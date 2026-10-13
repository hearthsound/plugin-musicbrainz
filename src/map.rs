//! Translation of MusicBrainz records into provider records.
//!
//! The functions here are pure: they take the types of [`crate::musicbrainz`]
//! and return the records of the `hearthsound:plugin@0.1.0` `types` interface,
//! so they are unit-tested on the host target against captured web service
//! responses.
//!
//! MusicBrainz issues its own ids and the server namespaces them per provider
//! instance, so a record's `id` field carries the MusicBrainz id as it is. The
//! canonical identifiers the server reconciles across providers - an ISRC, a
//! MusicBrainz recording, release, release group or artist id, and a release's
//! UPC - are reported in their own [`IdPair`] namespaces.

use hearthsound_plugin::metadata_provider::exports::hearthsound::plugin::metadata::{
    RecordingMatch, ReleaseSummary,
};
use hearthsound_plugin::types::{ArtistCredit as ProviderArtistCredit, IdPair};

use crate::musicbrainz;

/// Id namespace of an International Standard Recording Code.
pub const NS_ISRC: &str = "isrc";

/// Id namespace of a MusicBrainz recording.
pub const NS_RECORDING: &str = "mb-recording";

/// Id namespace of a MusicBrainz release.
pub const NS_RELEASE: &str = "mb-release";

/// Id namespace of a MusicBrainz release group.
pub const NS_RELEASE_GROUP: &str = "mb-release-group";

/// Id namespace of a MusicBrainz artist.
pub const NS_ARTIST: &str = "mb-artist";

/// Returns an id pair in one namespace.
///
/// # Parameters
///
/// - `namespace`: the namespace of the identifier.
/// - `value`: the identifier itself.
///
/// # Returns
///
/// The pair, ready to append to a record's `ids`.
///
/// # Example
///
/// ```text
/// id_pair(NS_RECORDING, "b061ba14-6b3b-4739-a043-dd4685357af4")
///   -> IdPair { namespace: "mb-recording", value: "b061ba14-6b3b-4739-a043-dd4685357af4" }
/// ```
pub fn id_pair(namespace: &str, value: &str) -> IdPair {
    IdPair {
        namespace: namespace.to_string(),
        value: value.to_string(),
    }
}

/// Converts an artist credit list from the web service.
///
/// # Parameters
///
/// - `credits`: the credits as the web service returned them.
///
/// # Returns
///
/// One credit per input credit, each named as credited and carrying the
/// `mb-artist` id when the credit resolved to an artist. A credit the web
/// service could not link, such as `[traditional]`, keeps its name and has no
/// ids.
///
/// # Example
///
/// ```text
/// artist_credits(&[ArtistCredit { name: "Boards of Canada", artist: Some(Artist { id: Some("69158f97-..."), name: "Boards of Canada" }) }])
///   -> [ArtistCredit { name: "Boards of Canada", ids: [("mb-artist", "69158f97-...")] }]
/// ```
pub fn artist_credits(credits: &[musicbrainz::ArtistCredit]) -> Vec<ProviderArtistCredit> {
    credits
        .iter()
        .map(|credit| ProviderArtistCredit {
            name: credit.name.clone(),
            ids: credit
                .artist
                .as_ref()
                .and_then(|artist| artist.id.as_deref())
                .map(|id| vec![id_pair(NS_ARTIST, id)])
                .unwrap_or_default(),
        })
        .collect()
}

/// Converts a release the web service embedded in a recording into a release
/// summary.
///
/// # Parameters
///
/// - `release`: the release as the web service returned it.
///
/// # Returns
///
/// The summary with the `mb-release` id, the `mb-release-group` id when the
/// response carried the release group, the title, the date and the track count
/// when that endpoint reported one. A recording search result carries a track
/// count; a recording lookup with `inc=releases` does not, because its release
/// objects omit the field.
///
/// # Example
///
/// ```text
/// release_summary(&ReleaseStub { id: "52295596-...", title: "Boc Maxima", date: Some("1996"), track_count: None, release_group: None, .. })
///   -> ReleaseSummary { ids: [("mb-release", "52295596-...")], title: "Boc Maxima", date: Some("1996"), track_count: None }
/// ```
pub fn release_summary(release: &musicbrainz::ReleaseStub) -> ReleaseSummary {
    let mut ids = vec![id_pair(NS_RELEASE, &release.id)];
    if let Some(group) = release
        .release_group
        .as_ref()
        .and_then(|group| group.id.as_deref())
    {
        ids.push(id_pair(NS_RELEASE_GROUP, group));
    }

    ReleaseSummary {
        ids,
        title: release.title.clone(),
        date: non_empty(release.date.as_deref()),
        track_count: release.track_count,
    }
}

/// Converts a recording the web service returned into a recording match.
///
/// # Parameters
///
/// - `recording`: the recording to convert.
/// - `releases`: the releases to report for it.
/// - `isrcs`: the ISRCs to report for it, in order.
/// - `score`: the match score, in `0..=1`, as computed by
///   [`crate::score::match_score`].
///
/// # Returns
///
/// The match with the `mb-recording` id first, then one `isrc` pair per
/// reported ISRC, the title, the artist credits, the recording length as the
/// duration, the release summaries and the score.
///
/// # Example
///
/// ```text
/// recording_match(&recording, &recording.releases, &["GBBPW9800010".into()], 1.0)
///   -> RecordingMatch { ids: [("mb-recording", "b061ba14-..."), ("isrc", "GBBPW9800010")], title: "Wildlife Analysis", score: 1.0, .. }
/// ```
pub fn recording_match(
    recording: &musicbrainz::Recording,
    releases: &[musicbrainz::ReleaseStub],
    isrcs: &[String],
    score: f32,
) -> RecordingMatch {
    let mut ids = vec![id_pair(NS_RECORDING, &recording.id)];
    ids.extend(isrcs.iter().map(|isrc| id_pair(NS_ISRC, isrc)));

    RecordingMatch {
        ids,
        title: recording.title.clone(),
        artists: artist_credits(&recording.artist_credit),
        duration_ms: recording.length,
        releases: releases.iter().map(release_summary).collect(),
        score,
    }
}

/// Returns a copy of an optional string that is neither absent nor empty.
///
/// The web service serializes a value it does not know as an empty string
/// rather than omitting it, which for a record field means "no value".
///
/// # Parameters
///
/// - `value`: the value as the web service returned it.
///
/// # Returns
///
/// `None` when the value is absent or empty, otherwise the value.
///
/// # Example
///
/// ```text
/// non_empty(Some("1998-08-20")) -> Some("1998-08-20".to_string())
/// non_empty(Some(""))           -> None
/// non_empty(None)               -> None
/// ```
fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders id pairs as tuples, because the generated `IdPair` is not
    /// comparable.
    fn id_pairs(pairs: &[IdPair]) -> Vec<(&str, &str)> {
        pairs
            .iter()
            .map(|pair| (pair.namespace.as_str(), pair.value.as_str()))
            .collect()
    }

    fn isrc_lookup() -> musicbrainz::IsrcLookup {
        serde_json::from_str(include_str!("fixtures/isrc_lookup.json")).unwrap()
    }

    fn search() -> musicbrainz::RecordingSearch {
        serde_json::from_str(include_str!("fixtures/search_recording.json")).unwrap()
    }

    fn recording_lookup() -> musicbrainz::Recording {
        serde_json::from_str(include_str!("fixtures/recording_releases.json")).unwrap()
    }

    #[test]
    fn maps_an_isrc_lookup_recording() {
        let lookup = isrc_lookup();
        let recording = &lookup.recordings[0];
        let match_ = recording_match(
            recording,
            &recording.releases,
            &["GBBPW9800010".to_string()],
            1.0,
        );

        assert_eq!(
            id_pairs(&match_.ids),
            vec![
                ("mb-recording", "b061ba14-6b3b-4739-a043-dd4685357af4"),
                ("isrc", "GBBPW9800010"),
            ]
        );
        assert_eq!(match_.title, "Wildlife Analysis");
        assert_eq!(match_.duration_ms, Some(77_426));
        assert_eq!(match_.artists.len(), 1);
        assert_eq!(match_.artists[0].name, "Boards of Canada");
        assert_eq!(
            id_pairs(&match_.artists[0].ids),
            vec![("mb-artist", "69158f97-4c07-4c4e-baf8-4e4ab1ed666e")]
        );
        assert!(match_.releases.is_empty());
        assert_eq!(match_.score, 1.0);
    }

    #[test]
    fn maps_a_search_result_with_its_releases() {
        let search = search();
        let recording = &search.recordings[0];
        let match_ = recording_match(recording, &recording.releases, &[], 1.0);

        assert_eq!(match_.title, "Roygbiv");
        assert_eq!(match_.duration_ms, Some(139_253));
        assert_eq!(match_.releases.len(), 1);

        let release = &match_.releases[0];
        assert_eq!(release.title, "The Cornerstone Player 044");
        assert_eq!(release.date.as_deref(), Some("2003"));
        assert_eq!(release.track_count, Some(67));
        assert_eq!(
            id_pairs(&release.ids),
            vec![
                ("mb-release", "ebcf935f-f49c-42e3-84e4-1007201bdca8"),
                ("mb-release-group", "406e886a-9cb3-4f02-b0fe-52328a3208ef"),
            ]
        );
    }

    #[test]
    fn a_search_result_without_a_length_keeps_no_duration() {
        let search = search();
        let match_ = recording_match(&search.recordings[1], &[], &[], 1.0);

        assert_eq!(match_.title, "roygbiv");
        assert_eq!(match_.duration_ms, None);
    }

    #[test]
    fn maps_a_recording_lookup_with_its_releases() {
        let recording = recording_lookup();
        let match_ = recording_match(&recording, &recording.releases, &recording.isrcs, 1.0);

        assert_eq!(match_.releases.len(), 2);
        assert_eq!(match_.releases[0].title, "Boc Maxima");
        assert_eq!(match_.releases[0].date.as_deref(), Some("1996"));
        assert_eq!(match_.releases[0].track_count, None);
        assert_eq!(
            id_pairs(&match_.releases[0].ids),
            vec![
                ("mb-release", "52295596-c761-4871-bd4e-1821862e4288"),
                ("mb-release-group", "1b066b55-5144-3302-aa37-3bbd289729e0"),
            ]
        );
        assert_eq!(match_.releases[1].date.as_deref(), Some("1998-04-20"));
    }

    #[test]
    fn keeps_the_reported_score() {
        let search = search();
        let match_ = recording_match(&search.recordings[2], &[], &[], 0.87);

        assert_eq!(match_.score, 0.87);
    }

    #[test]
    fn an_unresolved_credit_keeps_its_name_without_ids() {
        let credit = musicbrainz::ArtistCredit {
            name: "[traditional]".to_string(),
            artist: None,
        };

        let mapped = artist_credits(&[credit]);

        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].name, "[traditional]");
        assert!(mapped[0].ids.is_empty());
    }

    #[test]
    fn a_release_without_a_group_carries_only_the_release_id() {
        let release = musicbrainz::ReleaseStub {
            id: "52295596-c761-4871-bd4e-1821862e4288".to_string(),
            title: "Boc Maxima".to_string(),
            date: Some(String::new()),
            status: Some("Official".to_string()),
            track_count: Some(30),
            release_group: None,
        };

        let summary = release_summary(&release);

        assert_eq!(
            id_pairs(&summary.ids),
            vec![("mb-release", "52295596-c761-4871-bd4e-1821862e4288")]
        );
        assert_eq!(summary.date, None);
        assert_eq!(summary.track_count, Some(30));
    }
}
