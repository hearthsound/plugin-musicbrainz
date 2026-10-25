//! The metadata operations, one function per web service endpoint group.
//!
//! Each function composes a path, calls [`crate::api`] and translates the
//! payload with [`crate::map`]. The functions are thin on purpose: the
//! interesting logic is the path building and the choice between the ISRC
//! lookup and the search, which are tested here directly, and the translation,
//! which is tested in [`crate::map`].

use hearthsound_plugin::metadata_artwork_provider::exports::hearthsound::plugin::metadata::{
    ArtistInfo, ArtistInfoRequest, RecordingHint, RecordingMatch,
};
use hearthsound_plugin::types::{Album, IdPair, ProviderError};

use crate::{api, map, musicbrainz, query, score, url};

/// Number of releases the release group browse asks for per page.
///
/// It is the web service's maximum page size, so a group with at most this many
/// official releases is read with one request.
const RELEASE_LIMIT: u32 = 100;

/// Number of browse pages the earliest-release scan reads at most.
///
/// A release group rarely holds more than one page of official releases, and
/// every further page is another request at the web service's 1 request/s
/// average. A group with more releases than this many pages hold is scanned on
/// its first pages only, and the scan logs that it stopped at the cap.
const EARLIEST_RELEASE_PAGES: u32 = 5;

/// Number of ISRC lookup hits whose releases are fetched one by one.
///
/// The web service omits the release list of a serialized ISRC lookup, so a hit
/// that needs its releases costs another request, and an ISRC assigned to many
/// recordings would fan out without bound. The first few hits carry the
/// identity evidence; the rest are read without their releases.
const ISRC_DETAIL_LIMIT: usize = 5;

/// Number of artists an artist search asks for.
///
/// A search for an artist name returns homages, cover bands and namesakes
/// before or after the artist itself, and the web service orders them by
/// relevance, so the first few hits are the only ones worth reading.
const ARTIST_SEARCH_LIMIT: u32 = 5;

/// Relevance an artist search hit must reach to name the requested artist.
///
/// The index reports `100` for an exact name match and lower values for a hit
/// it only considers similar, so a hit below this is a different artist with a
/// name close to the one the caller asked for.
const ARTIST_SEARCH_SCORE: f32 = 90.0;

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

/// Returns information on an artist.
///
/// The caller's `mb-artist` id, when it has one, names the artist directly.
/// Otherwise the caller's name is searched for, and only a hit that spells the
/// name the same way, ignoring case, and that the index considers a confident
/// match of it, is used; anything less is a namesake rather than the artist the
/// caller asked about.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `request`: the artist the caller wants information on.
///
/// # Returns
///
/// The artist's information, or `None` when the id or the name found no
/// artist. The web service has no biography, so the information carries the
/// `mb-artist` id and the artist's url relations.
///
/// # Errors
///
/// Returns the provider error of the request that failed. A name no search hit
/// answers is `None`, not an error.
pub(crate) fn artist_info(
    base_url: &str,
    request: &ArtistInfoRequest,
) -> Result<Option<ArtistInfo>, ProviderError> {
    let Some(mbid) = artist_mbid(base_url, request)? else {
        hearthsound_plugin::debug!("no MusicBrainz artist matches {:?}", request.name);
        return Ok(None);
    };

    let artist: musicbrainz::ArtistLookup = api::get_json(base_url, &artist_path(&mbid))?;
    hearthsound_plugin::debug!(
        "read artist {} with {} relations",
        artist.id,
        artist.relations.len()
    );

    Ok(Some(map::artist_info(&artist)))
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

    let read = details(&lookup.recordings, |id| {
        api::get_json::<musicbrainz::Recording>(base_url, &recording_path(id))
    })?;

    for id in &read.skipped {
        hearthsound_plugin::debug!(
            "recording {id} read without its releases: {ISRC_DETAIL_LIMIT} detail fetches already done"
        );
    }

    for (id, error) in &read.dropped {
        hearthsound_plugin::warn!("no releases for recording {id}: {error:?}");
    }

    let mut matches = Vec::with_capacity(lookup.recordings.len());

    for (recording, detail) in lookup.recordings.iter().zip(&read.found) {
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

/// What reading the release details of an ISRC lookup found.
///
/// The hits kept out of the read are reported rather than logged here, so that
/// the caller logs them once, next to the request they belong to.
#[derive(Debug, Default)]
struct DetailRead {
    /// The fetched recording of each hit of the lookup, in the order the web
    /// service returned the hits.
    found: Vec<Option<musicbrainz::Recording>>,
    /// Ids of the hits read without their releases because
    /// [`ISRC_DETAIL_LIMIT`] was reached.
    skipped: Vec<String>,
    /// Ids of the hits whose fetch failed with an error about that request
    /// alone, with that error.
    dropped: Vec<(String, ProviderError)>,
}

/// Reads the releases of the ISRC lookup hits that came without them.
///
/// The JSON serialization of an ISRC lookup carries no release list even when
/// `inc=releases` asks for one, so a hit that brought none costs one more
/// request to read its releases and its full ISRC set. At most
/// [`ISRC_DETAIL_LIMIT`] hits are fetched: an ISRC assigned to many recordings
/// would otherwise fan out without bound, and the first hits are the ones the
/// identity evidence rests on.
///
/// # Parameters
///
/// - `recordings`: the hits of an ISRC lookup, in the order the web service
///   returned them.
/// - `fetch`: reads one recording by its MusicBrainz id.
///
/// # Returns
///
/// The details read, one entry per hit and in the same order: a hit that
/// brought no releases and was within the bound carries its fetched recording,
/// and every other hit carries none, with the reason recorded in
/// [`DetailRead::skipped`] or [`DetailRead::dropped`].
///
/// # Errors
///
/// Returns the provider error of a fetch that failed for a transient reason -
/// the web service being unavailable or asking the caller to slow down.
/// Answering without that hit would silently truncate the match set the caller
/// resolves an identity from, so the failure is reported instead and the host
/// retries it.
fn details(
    recordings: &[musicbrainz::Recording],
    mut fetch: impl FnMut(&str) -> Result<musicbrainz::Recording, ProviderError>,
) -> Result<DetailRead, ProviderError> {
    let mut read = DetailRead {
        found: Vec::with_capacity(recordings.len()),
        ..DetailRead::default()
    };
    let mut fetched = 0;

    for recording in recordings {
        if !recording.releases.is_empty() {
            read.found.push(None);
            continue;
        }

        if fetched >= ISRC_DETAIL_LIMIT {
            read.skipped.push(recording.id.clone());
            read.found.push(None);
            continue;
        }

        fetched += 1;
        match fetch(&recording.id) {
            Ok(detail) => read.found.push(Some(detail)),
            Err(error) if transient(&error) => return Err(error),
            Err(error) => {
                read.dropped.push((recording.id.clone(), error));
                read.found.push(None);
            }
        }
    }

    Ok(read)
}

/// Whether a provider error means the web service is temporarily unusable.
///
/// # Parameters
///
/// - `error`: the error a request failed with.
///
/// # Returns
///
/// `true` for [`ProviderError::Unavailable`] and [`ProviderError::RateLimited`],
/// which are about the web service rather than the data the plugin asked for
/// and pass on their own; `false` for every other error, which concerns the
/// single request that failed.
fn transient(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::Unavailable(_) | ProviderError::RateLimited(_)
    )
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
/// The id of the group's earliest official release, as [`earliest_browsed`]
/// collects the group's pages and [`earliest`] picks the release.
///
/// # Errors
///
/// Returns [`ProviderError::NotFound`] when the group has no official release
/// and when the web service does not know the group, and the provider error of
/// the request otherwise.
fn earliest_official_release(base_url: &str, release_group: &str) -> Result<String, ProviderError> {
    let browsed = earliest_browsed(|offset| {
        api::get_json(base_url, &release_group_path(release_group, offset))
    })?;

    if browsed.capped {
        hearthsound_plugin::debug!(
            "release browse stopped at the page cap: {} pages, {} releases read",
            browsed.pages,
            browsed.releases
        );
    }

    browsed
        .earliest
        .map(|release| release.id)
        .ok_or(ProviderError::NotFound)
}

/// What reading the browse pages of a release group found.
#[derive(Debug, Default)]
struct Browsed {
    /// The earliest release of the pages read.
    earliest: Option<musicbrainz::ReleaseStub>,
    /// Number of pages read.
    pages: u32,
    /// Number of releases the pages held.
    releases: usize,
    /// Whether [`EARLIEST_RELEASE_PAGES`] stopped the scan of a group that had
    /// more releases.
    capped: bool,
}

/// Reads the browse pages of a release group and picks the earliest release.
///
/// The web service does not order a browse by date, so the earliest release is
/// the minimum over the whole group rather than over the first page. The group
/// is read page by page until the page count the browse reports is reached, an
/// empty page arrives, or [`EARLIEST_RELEASE_PAGES`] pages were read; a group
/// past that cap is scanned on its first pages and the result reports that the
/// cap stopped the scan.
///
/// # Parameters
///
/// - `fetch`: reads the browse page at an offset into the group.
///
/// # Returns
///
/// What the pages held: the earliest release as [`earliest`] picks it, or
/// `None` when the group has no release.
///
/// # Errors
///
/// Returns the provider error of the page that failed.
fn earliest_browsed(
    mut fetch: impl FnMut(u32) -> Result<musicbrainz::ReleaseBrowse, ProviderError>,
) -> Result<Browsed, ProviderError> {
    let mut collected: Vec<musicbrainz::ReleaseStub> = Vec::new();
    let mut offset = 0;
    let mut pages = 0;
    let mut complete = false;

    while !complete && pages < EARLIEST_RELEASE_PAGES {
        let browse = fetch(offset)?;
        pages += 1;

        let returned = browse.releases.len() as u32;
        complete = match browse.release_count {
            Some(count) => collected.len() as u32 + returned >= count || returned == 0,
            None => true,
        };
        offset = browse.release_offset.unwrap_or(offset) + returned;
        collected.extend(browse.releases);
    }

    Ok(Browsed {
        earliest: earliest(&collected).cloned(),
        pages,
        releases: collected.len(),
        capped: !complete,
    })
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

/// Resolves the MusicBrainz id of the artist a request names.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `request`: the artist the caller wants information on.
///
/// # Returns
///
/// The id of the caller's `mb-artist` pair when it carries one, the id of the
/// search hit [`searched_artist`] picks when it does not, and `None` when a
/// request without an id carries no name to search for or no hit qualifies.
///
/// # Errors
///
/// Returns the provider error of the search request.
fn artist_mbid(
    base_url: &str,
    request: &ArtistInfoRequest,
) -> Result<Option<String>, ProviderError> {
    if let Some(pair) = request
        .ids
        .iter()
        .find(|pair| pair.namespace == map::NS_ARTIST)
    {
        return Ok(Some(pair.value.clone()));
    }

    let name = request.name.trim();
    if name.is_empty() {
        return Ok(None);
    }

    let search: musicbrainz::ArtistSearch = api::get_json(base_url, &artist_search_path(name))?;
    hearthsound_plugin::debug!(
        "artist search matched {:?} artists, reading {}",
        search.count,
        search.artists.len()
    );

    Ok(searched_artist(&search.artists, name))
}

/// Picks the searched artist that names the requested one.
///
/// A search hit is the artist only when it spells the name the same way,
/// ignoring case, and when the index scored it at least
/// [`ARTIST_SEARCH_SCORE`]; every other hit is a namesake. Hits are read in the
/// order the web service returned them, which is by relevance, so the most
/// relevant qualifying hit wins.
///
/// # Parameters
///
/// - `artists`: the hits of an artist search.
/// - `name`: the artist name that was searched for.
///
/// # Returns
///
/// The id of the first qualifying hit, or `None` when no hit qualifies.
///
/// # Example
///
/// ```text
/// // hits: "Northlight Tribute" (100), "Northlight" (78), "northlight" (95)
/// searched_artist(&hits, "Northlight") -> Some("aaaa0003-3333-4333-8333-333333333333")
/// ```
fn searched_artist(artists: &[musicbrainz::ArtistStub], name: &str) -> Option<String> {
    artists
        .iter()
        .find(|artist| {
            artist.name.eq_ignore_ascii_case(name)
                && artist.score.unwrap_or(0.0) >= ARTIST_SEARCH_SCORE
        })
        .map(|artist| artist.id.clone())
}

/// Builds the path of an artist lookup.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz artist id.
///
/// # Returns
///
/// The path, with the url relations requested and the JSON format.
fn artist_path(mbid: &str) -> String {
    format!(
        "artist/{}?inc=url-rels&fmt=json",
        url::encode_component(mbid)
    )
}

/// Builds the path of an artist search.
///
/// # Parameters
///
/// - `name`: the artist name, unencoded.
///
/// # Returns
///
/// The path, with the query encoded and the result page bounded by
/// [`ARTIST_SEARCH_LIMIT`].
fn artist_search_path(name: &str) -> String {
    let text = format!("artist:\"{}\"", query::escape_lucene(name));

    format!(
        "artist?query={}&limit={}&fmt=json",
        url::encode_component(&text),
        ARTIST_SEARCH_LIMIT
    )
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

/// Builds the path of a page of a release group's official releases.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz release group id.
/// - `offset`: number of releases of the group to skip, which selects the page.
///
/// # Returns
///
/// The path, with the status filter, the page size and the offset.
fn release_group_path(mbid: &str, offset: u32) -> String {
    format!(
        "release?release-group={}&status=official&fmt=json&limit={}&offset={}",
        url::encode_component(mbid),
        RELEASE_LIMIT,
        offset
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

    /// Builds a release of a browse page.
    fn release(id: &str, date: Option<&str>) -> musicbrainz::ReleaseStub {
        musicbrainz::ReleaseStub {
            id: id.to_string(),
            title: "Album".to_string(),
            date: date.map(str::to_string),
            track_count: None,
            release_group: None,
        }
    }

    /// Builds a browse page of `ids`, all dated `date`.
    fn browse_page(
        ids: Vec<String>,
        date: &str,
        count: u32,
        offset: u32,
    ) -> musicbrainz::ReleaseBrowse {
        musicbrainz::ReleaseBrowse {
            releases: ids.iter().map(|id| release(id, Some(date))).collect(),
            release_count: Some(count),
            release_offset: Some(offset),
        }
    }

    /// Builds `count` release ids numbered from `prefix`.
    fn ids(prefix: &str, count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("{prefix}-{index}"))
            .collect()
    }

    /// Builds an ISRC lookup hit carrying `releases` releases.
    fn recording(id: &str, releases: usize) -> musicbrainz::Recording {
        musicbrainz::Recording {
            id: id.to_string(),
            title: "Roygbiv".to_string(),
            length: None,
            isrcs: Vec::new(),
            artist_credit: Vec::new(),
            releases: (0..releases)
                .map(|index| release(&format!("release-{index}"), None))
                .collect(),
            score: None,
        }
    }

    fn artist_search() -> musicbrainz::ArtistSearch {
        serde_json::from_str(include_str!("fixtures/artist_search.json")).unwrap()
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
    fn a_hit_that_brought_its_releases_costs_no_request() {
        let mut requests = 0;

        let read = details(&[recording("hit", 2)], |id| {
            requests += 1;
            Ok(recording(id, 2))
        })
        .unwrap();

        assert_eq!(requests, 0);
        assert_eq!(read.found.len(), 1);
        assert!(read.found[0].is_none());
        assert!(read.skipped.is_empty());
        assert!(read.dropped.is_empty());
    }

    #[test]
    fn at_most_five_recordings_are_fetched_for_their_releases() {
        let hits: Vec<musicbrainz::Recording> = (0..6)
            .map(|index| recording(&format!("hit-{index}"), 0))
            .collect();
        let mut requests = 0;

        let read = details(&hits, |id| {
            requests += 1;
            Ok(recording(id, 1))
        })
        .unwrap();

        assert_eq!(requests, ISRC_DETAIL_LIMIT);
        assert_eq!(read.found.len(), 6);
        assert!(read.found[..ISRC_DETAIL_LIMIT].iter().all(Option::is_some));
        assert!(read.found[ISRC_DETAIL_LIMIT..].iter().all(Option::is_none));
        assert_eq!(read.skipped, vec!["hit-5".to_string()]);
    }

    #[test]
    fn a_missing_detail_fetch_only_drops_that_recording() {
        let hits = [recording("unknown", 0), recording("known", 0)];

        let read = details(&hits, |id| {
            if id == "unknown" {
                return Err(ProviderError::NotFound);
            }
            Ok(recording(id, 1))
        })
        .unwrap();

        assert_eq!(read.found.len(), 2);
        assert!(read.found[0].is_none());
        assert_eq!(
            read.found[1].as_ref().map(|detail| detail.releases.len()),
            Some(1)
        );
        assert_eq!(read.dropped.len(), 1);
        assert_eq!(read.dropped[0].0, "unknown");
        assert!(matches!(&read.dropped[0].1, ProviderError::NotFound));
    }

    #[test]
    fn a_transient_detail_fetch_fails_the_isrc_lookup() {
        let hits = [recording("hit", 0)];

        let unavailable = details(&hits, |_| {
            Err(ProviderError::Unavailable("HTTP 500".to_string()))
        });
        assert!(matches!(unavailable, Err(ProviderError::Unavailable(_))));

        let limited = details(&hits, |_| Err(ProviderError::RateLimited(1_000)));
        assert!(matches!(limited, Err(ProviderError::RateLimited(1_000))));
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
            release_group_path("17d74d52-c92b-3b8d-9f87-218ab2d1c4a0", 0),
            "release?release-group=17d74d52-c92b-3b8d-9f87-218ab2d1c4a0&status=official&fmt=json&limit=100&offset=0"
        );
        assert_eq!(
            release_group_path("17d74d52-c92b-3b8d-9f87-218ab2d1c4a0", 100),
            "release?release-group=17d74d52-c92b-3b8d-9f87-218ab2d1c4a0&status=official&fmt=json&limit=100&offset=100"
        );
    }

    #[test]
    fn the_health_probe_reads_one_genre() {
        assert_eq!(health_path(), "genre/all?fmt=json&limit=1");
    }

    #[test]
    fn an_artist_lookup_asks_for_the_url_relations() {
        assert_eq!(
            artist_path("d1f2e3a4-5b6c-4d7e-8f90-1a2b3c4d5e6f"),
            "artist/d1f2e3a4-5b6c-4d7e-8f90-1a2b3c4d5e6f?inc=url-rels&fmt=json"
        );
    }

    #[test]
    fn an_artist_search_encodes_the_name_and_bounds_the_page() {
        assert_eq!(
            artist_search_path("Boards of Canada"),
            "artist?query=artist%3A%22Boards%20of%20Canada%22&limit=5&fmt=json"
        );
    }

    #[test]
    fn an_artist_named_by_id_is_not_searched_for() {
        let request = ArtistInfoRequest {
            name: "Northlight".to_string(),
            ids: vec![IdPair {
                namespace: map::NS_ARTIST.to_string(),
                value: "d1f2e3a4-5b6c-4d7e-8f90-1a2b3c4d5e6f".to_string(),
            }],
        };

        assert_eq!(
            artist_mbid("https://musicbrainz.invalid/ws/2/", &request).unwrap(),
            Some("d1f2e3a4-5b6c-4d7e-8f90-1a2b3c4d5e6f".to_string())
        );
    }

    #[test]
    fn a_request_without_an_id_or_a_name_takes_no_request() {
        let request = ArtistInfoRequest {
            name: "  ".to_string(),
            ids: Vec::new(),
        };

        assert_eq!(
            artist_mbid("https://musicbrainz.invalid/ws/2/", &request).unwrap(),
            None
        );
    }

    #[test]
    fn the_first_search_hit_that_names_the_artist_and_scores_well_wins() {
        let search = artist_search();
        let picked = searched_artist(&search.artists, "Northlight").unwrap();

        // The fixture's hits are "Northlight Tribute" (100), "Northlight" (78),
        // "northlight" ("95") and "Northlight" (91): only the third hit spells
        // the name the same way and reaches the score a hit must reach, so the
        // name case and the score both decide, and the first qualifying hit
        // wins.
        assert_eq!(picked, "aaaa0003-3333-4333-8333-333333333333");
    }

    #[test]
    fn no_search_hit_names_an_artist_the_index_does_not_know() {
        let search = artist_search();

        assert_eq!(searched_artist(&search.artists, "Northlight Quintet"), None);
        assert_eq!(searched_artist(&[], "Northlight"), None);
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
    fn a_group_that_fits_one_page_costs_one_request() {
        let mut requests = 0;

        let browsed = earliest_browsed(|offset| {
            requests += 1;
            Ok(browse_page(ids("release", 100), "2005-03-01", 100, offset))
        })
        .unwrap();

        assert_eq!(requests, 1);
        assert_eq!(browsed.pages, 1);
        assert_eq!(browsed.releases, 100);
        assert!(!browsed.capped);
        assert_eq!(
            browsed.earliest.as_ref().map(|release| release.id.as_str()),
            Some("release-0")
        );
    }

    #[test]
    fn the_earliest_release_is_picked_across_the_pages_of_a_group() {
        let mut requests = 0;

        // The first page holds releases from 2005 and the second a 1998 one
        // dated before them: a scan that stops after the first page dates the
        // group 2005.
        let browsed = earliest_browsed(|offset| {
            requests += 1;
            Ok(if offset == 0 {
                browse_page(ids("first", 100), "2005-03-01", 120, 0)
            } else {
                let mut second = browse_page(ids("second", 19), "2005-06-01", 120, 100);
                second
                    .releases
                    .push(release("earliest", Some("1998-04-20")));
                second
            })
        })
        .unwrap();

        assert_eq!(requests, 2);
        assert_eq!(browsed.releases, 120);
        assert!(!browsed.capped);
        assert_eq!(
            browsed.earliest.as_ref().map(|release| release.id.as_str()),
            Some("earliest")
        );
    }

    #[test]
    fn the_page_cap_stops_the_scan_of_a_group_with_more_releases() {
        let mut requests = 0;

        let browsed = earliest_browsed(|offset| {
            requests += 1;
            Ok(browse_page(
                ids(&format!("page-{offset}"), 100),
                "2005-03-01",
                1_000,
                offset,
            ))
        })
        .unwrap();

        assert_eq!(requests, EARLIEST_RELEASE_PAGES);
        assert_eq!(browsed.pages, EARLIEST_RELEASE_PAGES);
        assert_eq!(browsed.releases, 500);
        assert!(browsed.capped);
        assert_eq!(
            browsed.earliest.as_ref().map(|release| release.id.as_str()),
            Some("page-0-0")
        );
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
