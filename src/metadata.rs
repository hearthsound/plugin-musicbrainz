//! The metadata operations, one function per web service endpoint group.
//!
//! Each function composes a path, calls [`crate::api`] and translates the
//! payload with [`crate::map`]. The functions are thin on purpose: the
//! interesting logic is the path building and the choice between the ISRC
//! lookup and the search, which are tested here directly, and the translation,
//! which is tested in [`crate::map`].

use hearthsound_plugin::metadata_provider::exports::hearthsound::plugin::metadata::{
    RecordingHint, RecordingMatch,
};
use hearthsound_plugin::types::{Album, IdPair, ProviderError};

use crate::{api, map, musicbrainz, query, score, url};

/// Number of releases the release group browse asks for.
///
/// It is the web service's maximum page size and comfortably above the number
/// of official releases a release group has, so one page resolves the group.
const RELEASE_LIMIT: u32 = 100;

/// Resolves a recording hint to scored MusicBrainz recordings.
///
/// The hint's ISRC, when it carries a usable one, is the strongest evidence and
/// is used first: MusicBrainz assigns an ISRC to one or a few recordings, and
/// their score starts at `1.0`. An ISRC the web service does not know, one that
/// is not shaped like an ISRC, and a hint without one all fall through to a
/// Lucene search of the recording index, whose hits start from the relevance
/// MusicBrainz reported.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `hint`: the recording hint.
///
/// # Returns
///
/// The matches, best score first. The list is empty when neither the ISRC nor
/// the search found anything, and when the hint has no title to search for; an
/// empty list is not an error, because a hint that names no recording is a
/// normal answer for the caller's identity resolution.
///
/// # Errors
///
/// Returns the provider error of the request that failed. A hit of the ISRC
/// lookup that cannot be enriched with its releases is reported with an empty
/// release list rather than as an error, because the recording id is still the
/// identity the caller asked for.
pub(crate) fn resolve_recording(
    base_url: &str,
    hint: &RecordingHint,
) -> Result<Vec<RecordingMatch>, ProviderError> {
    let isrc = hint
        .isrc
        .as_deref()
        .map(url::normalize_isrc)
        .filter(|isrc| url::is_valid_isrc(isrc));

    if let Some(isrc) = isrc
        && let Some(matches) = isrc_matches(base_url, &isrc, hint)?
        && !matches.is_empty()
    {
        return Ok(sorted(matches));
    }

    Ok(sorted(search_matches(base_url, hint)?))
}

/// Materializes a release as an album.
///
/// The caller's `mb-release` id, when it has one, names the release directly.
/// Otherwise an `mb-release-group` id is resolved to one of the group's
/// official releases - the earliest, because a group's first release is the one
/// a library wants when it materializes an album it knows only by group.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `ids`: the ids the caller supplied.
///
/// # Returns
///
/// The album of the release, with its tracks.
///
/// # Errors
///
/// Returns [`ProviderError::NotFound`] when `ids` carries neither an
/// `mb-release` nor an `mb-release-group` pair, when the release group has no
/// official release, and when the web service does not know the release; the
/// provider error of the request that failed otherwise.
pub(crate) fn get_release(base_url: &str, ids: &[IdPair]) -> Result<Album, ProviderError> {
    let release_id = match ids.iter().find(|pair| pair.namespace == map::NS_RELEASE) {
        Some(pair) => pair.value.clone(),
        None => {
            let Some(group) = ids
                .iter()
                .find(|pair| pair.namespace == map::NS_RELEASE_GROUP)
            else {
                hearthsound_plugin::debug!("no MusicBrainz release or release group id in {ids:?}");
                return Err(ProviderError::NotFound);
            };

            earliest_official_release(base_url, &group.value)?
        }
    };

    let release: musicbrainz::Release = api::get_json(base_url, &release_path(&release_id))?;
    hearthsound_plugin::debug!(
        "read release {} with {} media",
        release.id,
        release.media.len()
    );

    Ok(map::album(&release))
}

/// Probes the web service with the smallest request it serves.
///
/// `GET /genre/all?limit=1` reads one row of the genre table, so the probe
/// costs the service almost nothing while still proving that the configured
/// base URL answers with the JSON this plugin expects.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
///
/// # Returns
///
/// `Ok(())` when the web service answered.
///
/// # Errors
///
/// Returns the provider error of the request.
pub(crate) fn health(base_url: &str) -> Result<(), ProviderError> {
    let _page: musicbrainz::GenrePage = api::get_json(base_url, &health_path())?;

    Ok(())
}

/// Resolves an ISRC to its recordings.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `isrc`: the normalized ISRC to look up.
/// - `hint`: the hint the ISRC came from, for scoring.
///
/// # Returns
///
/// The recordings the ISRC is assigned to, or `None` when MusicBrainz does not
/// know the code and the caller should search instead.
///
/// # Errors
///
/// Returns the provider error of a request that failed for any other reason.
fn isrc_matches(
    base_url: &str,
    isrc: &str,
    hint: &RecordingHint,
) -> Result<Option<Vec<RecordingMatch>>, ProviderError> {
    let lookup: musicbrainz::IsrcLookup = match api::get_json(base_url, &isrc_path(isrc)) {
        Ok(lookup) => lookup,
        Err(ProviderError::NotFound) => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut matches = Vec::with_capacity(lookup.recordings.len());

    for recording in &lookup.recordings {
        // The JSON serialization of an ISRC lookup carries no release list even
        // when `inc=releases` asks for one, so a hit that brought none costs
        // one more request to read its releases and its full ISRC set.
        let detail = if recording.releases.is_empty() {
            match api::get_json::<musicbrainz::Recording>(base_url, &recording_path(&recording.id))
            {
                Ok(detail) => Some(detail),
                Err(error) => {
                    hearthsound_plugin::warn!(
                        "no releases for recording {}: {error:?}",
                        recording.id
                    );
                    None
                }
            }
        } else {
            None
        };

        let releases = detail
            .as_ref()
            .map_or(recording.releases.as_slice(), |detail| {
                detail.releases.as_slice()
            });

        let mut isrcs = vec![isrc.to_string()];
        for value in detail
            .as_ref()
            .map_or(recording.isrcs.as_slice(), |detail| detail.isrcs.as_slice())
        {
            if !isrcs.iter().any(|seen| seen == value) {
                isrcs.push(value.clone());
            }
        }

        let adjusted = score::match_score(
            1.0,
            Some(&hint.title),
            hint.duration_ms,
            &recording.title,
            recording.length,
        );

        matches.push(map::recording_match(recording, releases, &isrcs, adjusted));
    }

    Ok(Some(matches))
}

/// Searches the recording index for a hint and scores the hits.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `hint`: the hint to search for.
///
/// # Returns
///
/// The matches in the order the web service returned them, which is by
/// relevance. A hint without a title is not searchable and yields no matches.
///
/// # Errors
///
/// Returns the provider error of the request.
fn search_matches(
    base_url: &str,
    hint: &RecordingHint,
) -> Result<Vec<RecordingMatch>, ProviderError> {
    let Some(text) = query::recording_query(
        &hint.title,
        &hint.artists,
        hint.album.as_deref(),
        hint.duration_ms,
    ) else {
        hearthsound_plugin::debug!("recording hint without a title is not searchable");
        return Ok(Vec::new());
    };

    let search: musicbrainz::RecordingSearch = api::get_json(base_url, &search_path(&text))?;
    hearthsound_plugin::debug!(
        "recording search matched {:?} recordings, reading {}",
        search.count,
        search.recordings.len()
    );

    Ok(search
        .recordings
        .iter()
        .map(|recording| {
            let relevance = recording.score.unwrap_or(0.0) / 100.0;
            let adjusted = score::match_score(
                relevance,
                Some(&hint.title),
                hint.duration_ms,
                &recording.title,
                recording.length,
            );

            map::recording_match(recording, &recording.releases, &[], adjusted)
        })
        .collect())
}

/// Resolves a release group to its earliest official release.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `release_group`: MusicBrainz id of the release group.
///
/// # Returns
///
/// The id of the group's earliest official release, as [`earliest`] picks it.
///
/// # Errors
///
/// Returns [`ProviderError::NotFound`] when the group has no official release
/// and when the web service does not know the group, and the provider error of
/// the request otherwise.
fn earliest_official_release(base_url: &str, release_group: &str) -> Result<String, ProviderError> {
    let browse: musicbrainz::ReleaseBrowse =
        api::get_json(base_url, &release_group_path(release_group))?;

    earliest(&browse.releases)
        .map(|release| release.id.clone())
        .ok_or(ProviderError::NotFound)
}

/// Picks the earliest release of a browse result.
///
/// The release of a group that came first is the one the web service lists
/// first, so a date only becomes the deciding factor when there is one. A
/// release the web service left undated sorts after every dated one, because an
/// unknown date cannot be shown to be early.
///
/// # Parameters
///
/// - `releases`: the releases of a browse page.
///
/// # Returns
///
/// The earliest release, or `None` when the page is empty.
///
/// # Example
///
/// ```text
/// earliest(&[dated("2002"), dated("1998-04-20"), dated("1998-09-22")])
///   -> the release dated "1998-04-20"
/// ```
fn earliest(releases: &[musicbrainz::ReleaseStub]) -> Option<&musicbrainz::ReleaseStub> {
    releases
        .iter()
        .min_by_key(|release| date_key(release.date.as_deref()))
}

/// Builds the sort key of a release date.
///
/// # Parameters
///
/// - `date`: the release date, in the partial form the web service uses.
///
/// # Returns
///
/// A pair that orders partial ISO dates chronologically and pushes an absent or
/// empty date behind every real one.
fn date_key(date: Option<&str>) -> (bool, &str) {
    match date {
        Some(date) if !date.is_empty() => (false, date),
        _ => (true, ""),
    }
}

/// Sorts matches by score, best first.
///
/// # Parameters
///
/// - `matches`: the matches to sort.
///
/// # Returns
///
/// The same matches, ordered by descending score. Two matches with the same
/// score keep the order the web service returned them in.
fn sorted(mut matches: Vec<RecordingMatch>) -> Vec<RecordingMatch> {
    matches.sort_by(|left, right| right.score.total_cmp(&left.score));
    matches
}

/// Builds the path of an ISRC lookup.
///
/// The web service keys recordings by the bare 12-character code.
///
/// # Parameters
///
/// - `isrc`: the normalized ISRC.
///
/// # Returns
///
/// The path, with the requested inclusions and the JSON format.
fn isrc_path(isrc: &str) -> String {
    format!(
        "isrc/{}?inc=artist-credits+releases&fmt=json",
        url::encode_component(isrc)
    )
}

/// Builds the path of a recording lookup.
///
/// The inclusions are the ones a recording hit needs beyond what an ISRC lookup
/// returns: its releases, their release groups, and its ISRCs.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz recording id.
///
/// # Returns
///
/// The path, with the requested inclusions and the JSON format.
fn recording_path(mbid: &str) -> String {
    format!(
        "recording/{}?inc=releases+release-groups+isrcs&fmt=json",
        url::encode_component(mbid)
    )
}

/// Builds the path of a recording search.
///
/// # Parameters
///
/// - `text`: the Lucene query, unencoded.
///
/// # Returns
///
/// The path, with the query encoded and the result page bounded by
/// [`query::SEARCH_LIMIT`].
fn search_path(text: &str) -> String {
    format!(
        "recording?query={}&fmt=json&limit={}",
        url::encode_component(text),
        query::SEARCH_LIMIT
    )
}

/// Builds the path of a release lookup.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz release id.
///
/// # Returns
///
/// The path, with the inclusions a materialized album needs: the recordings of
/// every track with their ISRCs and artist credits, and the release group with
/// the genres of both entities.
fn release_path(mbid: &str) -> String {
    format!(
        "release/{}?inc=recordings+artist-credits+isrcs+release-groups+genres&fmt=json",
        url::encode_component(mbid)
    )
}

/// Builds the path of a release group's official releases.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz release group id.
///
/// # Returns
///
/// The path, with the status filter and the page size.
fn release_group_path(mbid: &str) -> String {
    format!(
        "release?release-group={}&status=official&fmt=json&limit={}",
        url::encode_component(mbid),
        RELEASE_LIMIT
    )
}

/// Builds the path of the health probe.
///
/// # Returns
///
/// The path of the genre listing, bounded to a single row.
fn health_path() -> String {
    "genre/all?fmt=json&limit=1".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn releases_of_a_group() -> Vec<musicbrainz::ReleaseStub> {
        let browse: musicbrainz::ReleaseBrowse =
            serde_json::from_str(include_str!("fixtures/releases_by_group.json")).unwrap();

        browse.releases
    }

    #[test]
    fn an_isrc_lookup_asks_for_the_credits_and_releases() {
        assert_eq!(
            isrc_path("GBBPW9800010"),
            "isrc/GBBPW9800010?inc=artist-credits+releases&fmt=json"
        );
    }

    #[test]
    fn a_recording_lookup_asks_for_releases_and_isrcs() {
        assert_eq!(
            recording_path("b061ba14-6b3b-4739-a043-dd4685357af4"),
            "recording/b061ba14-6b3b-4739-a043-dd4685357af4?inc=releases+release-groups+isrcs&fmt=json"
        );
    }

    #[test]
    fn a_search_encodes_the_query_and_bounds_the_page() {
        let text = query::recording_query("Roygbiv", &["Boards of Canada".to_string()], None, None)
            .unwrap();

        assert_eq!(
            search_path(&text),
            "recording?query=recording%3A%22Roygbiv%22%20AND%20artist%3A%22Boards%20of%20Canada%22&fmt=json&limit=10"
        );
    }

    #[test]
    fn a_release_lookup_asks_for_recordings_credits_isrcs_and_genres() {
        assert_eq!(
            release_path("6360f948-17d1-4135-a580-34dc200930d8"),
            "release/6360f948-17d1-4135-a580-34dc200930d8?inc=recordings+artist-credits+isrcs+release-groups+genres&fmt=json"
        );
    }

    #[test]
    fn a_release_group_browse_filters_to_official_releases() {
        assert_eq!(
            release_group_path("17d74d52-c92b-3b8d-9f87-218ab2d1c4a0"),
            "release?release-group=17d74d52-c92b-3b8d-9f87-218ab2d1c4a0&status=official&fmt=json&limit=100"
        );
    }

    #[test]
    fn the_health_probe_reads_one_genre() {
        assert_eq!(health_path(), "genre/all?fmt=json&limit=1");
    }

    #[test]
    fn the_earliest_release_of_a_group_wins() {
        let releases = releases_of_a_group();
        let picked = earliest(&releases).unwrap();

        // The fixture's official releases start with 1998-09-22, 1998-04-20 and
        // 2002-02-13.
        assert_eq!(picked.id, "1a8207d9-75fa-44d7-9a83-04801b0619f3");
        assert_eq!(picked.date.as_deref(), Some("1998-04-20"));
    }

    #[test]
    fn an_undated_release_sorts_after_a_dated_one() {
        let undated = musicbrainz::ReleaseStub {
            id: "0f598088-e9e6-4733-aee5-d3c3316a1f3b".to_string(),
            title: "The Wall".to_string(),
            date: None,
            track_count: None,
            release_group: None,
        };
        let empty = musicbrainz::ReleaseStub {
            date: Some(String::new()),
            ..undated.clone()
        };
        let dated = musicbrainz::ReleaseStub {
            id: "9e6886e7-b1bb-484d-b152-fe7d90c3e973".to_string(),
            date: Some("1979".to_string()),
            ..undated.clone()
        };

        let page = [undated, empty, dated];
        let picked = earliest(&page).unwrap();

        assert_eq!(picked.id, "9e6886e7-b1bb-484d-b152-fe7d90c3e973");
        assert_eq!(earliest(&[]).map(|release| &release.id), None);
    }

    #[test]
    fn matches_are_sorted_by_descending_score() {
        let matches = vec![
            match_with_score(0.5),
            match_with_score(1.0),
            match_with_score(0.9),
        ];

        let sorted: Vec<f32> = sorted(matches).iter().map(|m| m.score).collect();

        assert_eq!(sorted, vec![1.0, 0.9, 0.5]);
    }

    /// Builds a match carrying `score` and nothing else of interest.
    fn match_with_score(score: f32) -> RecordingMatch {
        RecordingMatch {
            ids: Vec::new(),
            title: "Roygbiv".to_string(),
            artists: Vec::new(),
            duration_ms: None,
            releases: Vec::new(),
            score,
        }
    }
}
