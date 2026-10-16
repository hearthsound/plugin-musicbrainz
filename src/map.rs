//! Translation of MusicBrainz records into provider records.
//!
//! The functions here are pure: they take the types of [`crate::musicbrainz`]
//! and return the records of the `hearthsound:plugin@0.2.0` `types` interface,
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
use hearthsound_plugin::types::{
    Album, AlbumRef, ArtistCredit as ProviderArtistCredit, IdPair, Track as ProviderTrack,
};

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

/// Id namespace of a release's barcode (UPC or EAN).
pub const NS_UPC: &str = "upc";

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

/// Converts a release into an album with its tracks.
///
/// # Parameters
///
/// - `release`: the release to convert, as a lookup with `inc=recordings`
///   returned it.
///
/// # Returns
///
/// The album with the release id and title, the credited artists, the release
/// date, the primary type of its release group, the barcode as its UPC, the
/// release's genres (or the release group's when the release has none) and
/// every track of every medium, in medium then track order. Each track carries
/// the position of its medium as its disc number and points back at the album.
/// The images are empty: this plugin does not read artwork yet.
///
/// # Example
///
/// ```text
/// album(&release)
///   -> Album { id: "6360f948-...", title: "Music Has the Right to Children", upc: Some("744861029922"), tracks: [Track { disc_number: Some(1), track_number: Some(1), .. }], .. }
/// ```
pub fn album(release: &musicbrainz::Release) -> Album {
    let artists = artist_credits(&release.artist_credit);
    let reference = album_ref(release, &artists);

    let mut tracks = Vec::new();
    for medium in &release.media {
        let disc_number = narrow(medium.position.unwrap_or(0));
        for track in &medium.tracks {
            tracks.push(self::track(track, disc_number, &reference));
        }
    }

    Album {
        id: release.id.clone(),
        title: release.title.clone(),
        artists,
        release_date: non_empty(release.date.as_deref()),
        album_type: release
            .release_group
            .as_ref()
            .and_then(|group| group.primary_type.clone()),
        upc: non_empty(release.barcode.as_deref()),
        genres: genres(release),
        images: Vec::new(),
        tracks,
        ids: album_ids(release),
    }
}

/// Converts one track of a release medium into a provider track.
///
/// # Parameters
///
/// - `source`: the track as the web service returned it.
/// - `disc_number`: position of the medium the track sits on.
/// - `album`: the album reference the track points back at.
///
/// # Returns
///
/// The track with its MusicBrainz track id as its id, its title (or the
/// recording's title when the web service left the track title out), the
/// credited artists (falling back to the recording's when the response carried
/// none), the length on this medium as its duration, the `mb-recording` id and
/// one `isrc` pair per ISRC of its recording, and the first ISRC as its `isrc`
/// field. The explicit flag stays unset: MusicBrainz does not record one.
///
/// # Example
///
/// ```text
/// track(&medium.tracks[0], Some(1), &album_ref)
///   -> Track { id: "50b02d68-...", title: "Wildlife Analysis", disc_number: Some(1), track_number: Some(1), isrc: Some("GBBPW9800010"), .. }
/// ```
pub fn track(
    source: &musicbrainz::Track,
    disc_number: Option<u16>,
    album: &AlbumRef,
) -> ProviderTrack {
    let recording = source.recording.as_ref();

    let artists = if source.artist_credit.is_empty() {
        recording
            .map(|recording| artist_credits(&recording.artist_credit))
            .unwrap_or_default()
    } else {
        artist_credits(&source.artist_credit)
    };

    let mut ids = Vec::new();
    if let Some(recording) = recording {
        ids.push(id_pair(NS_RECORDING, &recording.id));
        ids.extend(recording.isrcs.iter().map(|isrc| id_pair(NS_ISRC, isrc)));
    }

    ProviderTrack {
        id: source.id.clone(),
        title: non_empty(source.title.as_deref())
            .or_else(|| recording.map(|recording| recording.title.clone()))
            .unwrap_or_default(),
        artists,
        album: Some(album.clone()),
        duration_ms: source
            .length
            .or_else(|| recording.and_then(|recording| recording.length)),
        disc_number,
        track_number: source.position.and_then(narrow),
        isrc: recording.and_then(|recording| recording.isrcs.first().cloned()),
        explicit: None,
        ids,
    }
}

/// Builds the album reference of a release.
///
/// # Parameters
///
/// - `release`: the release to reference.
/// - `artists`: the release's already converted artist credits.
///
/// # Returns
///
/// The reference with the release id, title, artists and the year the release
/// date starts with. Its images are empty.
///
/// # Example
///
/// ```text
/// album_ref(&release, &artists)
///   -> AlbumRef { id: "6360f948-...", title: "Music Has the Right to Children", year: Some(1998), images: [] }
/// ```
fn album_ref(release: &musicbrainz::Release, artists: &[ProviderArtistCredit]) -> AlbumRef {
    AlbumRef {
        id: release.id.clone(),
        title: release.title.clone(),
        artists: artists.to_vec(),
        year: year(release.date.as_deref()),
        images: Vec::new(),
    }
}

/// Collects the ids of a release.
///
/// # Parameters
///
/// - `release`: the release to collect ids for.
///
/// # Returns
///
/// The `mb-release` id, the `mb-release-group` id when the response carried
/// the release group, and the `upc` pair when the release has a barcode.
fn album_ids(release: &musicbrainz::Release) -> Vec<IdPair> {
    let mut ids = vec![id_pair(NS_RELEASE, &release.id)];
    if let Some(group) = release
        .release_group
        .as_ref()
        .and_then(|group| group.id.as_deref())
    {
        ids.push(id_pair(NS_RELEASE_GROUP, group));
    }
    if let Some(upc) = non_empty(release.barcode.as_deref()) {
        ids.push(id_pair(NS_UPC, &upc));
    }
    ids
}

/// Returns the genres of a release.
///
/// # Parameters
///
/// - `release`: the release to read.
///
/// # Returns
///
/// The names of the release's own genres, or the names of its release group's
/// genres when the release carries none, in the order the web service returned
/// them.
fn genres(release: &musicbrainz::Release) -> Vec<String> {
    let names = |genres: &[musicbrainz::Genre]| {
        genres
            .iter()
            .map(|genre| genre.name.clone())
            .collect::<Vec<String>>()
    };

    if !release.genres.is_empty() {
        return names(&release.genres);
    }

    release
        .release_group
        .as_ref()
        .map(|group| names(&group.genres))
        .unwrap_or_default()
}

/// Reads the leading year of a release date.
///
/// # Parameters
///
/// - `date`: the release date, in the partial form the web service uses.
///
/// # Returns
///
/// The first four characters parsed as a year, or `None` when the date is
/// absent or does not start with four digits.
///
/// # Example
///
/// ```text
/// year(Some("1998-08-20")) -> Some(1998)
/// year(Some("2006"))       -> Some(2006)
/// year(None)               -> None
/// ```
fn year(date: Option<&str>) -> Option<u16> {
    date?.get(..4)?.parse::<u16>().ok()
}

/// Narrows a web service position to the width the provider records use.
///
/// # Parameters
///
/// - `value`: the position.
///
/// # Returns
///
/// The position as a `u16`, or `None` when it does not fit, which no real
/// release or medium position does.
fn narrow(value: u32) -> Option<u16> {
    u16::try_from(value).ok()
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

    fn release_fixture() -> musicbrainz::Release {
        serde_json::from_str(include_str!("fixtures/release.json")).unwrap()
    }

    fn multi_medium_release() -> musicbrainz::Release {
        serde_json::from_str(include_str!("fixtures/release_multi.json")).unwrap()
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

    #[test]
    fn maps_a_release_with_its_tracks() {
        let album = album(&release_fixture());

        assert_eq!(album.id, "6360f948-17d1-4135-a580-34dc200930d8");
        assert_eq!(album.title, "Music Has the Right to Children");
        assert_eq!(album.release_date.as_deref(), Some("1998-08-20"));
        assert_eq!(album.album_type.as_deref(), Some("Album"));
        assert_eq!(album.upc.as_deref(), Some("744861029922"));
        assert_eq!(album.genres, vec!["electronic"]);
        assert!(album.images.is_empty());
        assert_eq!(
            id_pairs(&album.ids),
            vec![
                ("mb-release", "6360f948-17d1-4135-a580-34dc200930d8"),
                ("mb-release-group", "17d74d52-c92b-3b8d-9f87-218ab2d1c4a0"),
                ("upc", "744861029922"),
            ]
        );
        assert_eq!(album.artists.len(), 1);
        assert_eq!(album.artists[0].name, "Boards of Canada");

        let track = &album.tracks[0];
        assert_eq!(track.id, "50b02d68-e39a-331b-bac6-ece2bb3c59c0");
        assert_eq!(track.title, "Wildlife Analysis");
        assert_eq!(track.duration_ms, Some(75_626));
        assert_eq!(track.disc_number, Some(1));
        assert_eq!(track.track_number, Some(1));
        assert_eq!(track.isrc.as_deref(), Some("GBBPW9800010"));
        assert_eq!(track.explicit, None);
        assert_eq!(
            id_pairs(&track.ids),
            vec![
                ("mb-recording", "b061ba14-6b3b-4739-a043-dd4685357af4"),
                ("isrc", "GBBPW9800010"),
            ]
        );

        let reference = track.album.as_ref().unwrap();
        assert_eq!(reference.id, "6360f948-17d1-4135-a580-34dc200930d8");
        assert_eq!(reference.title, "Music Has the Right to Children");
        assert_eq!(reference.year, Some(1998));
        assert!(reference.images.is_empty());
    }

    #[test]
    fn maps_every_medium_of_a_release() {
        let album = album(&multi_medium_release());

        assert_eq!(album.title, "The Cornerstone Player 044");
        assert_eq!(album.album_type.as_deref(), Some("Album"));
        assert_eq!(album.release_date.as_deref(), Some("2003"));
        // The web service reports an unknown barcode as an empty string.
        assert_eq!(album.upc, None);
        assert!(album.genres.is_empty());
        assert_eq!(
            id_pairs(&album.ids),
            vec![
                ("mb-release", "ebcf935f-f49c-42e3-84e4-1007201bdca8"),
                ("mb-release-group", "406e886a-9cb3-4f02-b0fe-52328a3208ef"),
            ]
        );

        assert_eq!(album.tracks.len(), 4);
        let disc_numbers: Vec<Option<u16>> = album.tracks.iter().map(|t| t.disc_number).collect();
        assert_eq!(disc_numbers, vec![Some(1), Some(1), Some(2), Some(2)]);
        assert_eq!(
            album
                .tracks
                .iter()
                .map(|t| t.track_number)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(1), Some(2)]
        );
        assert_eq!(album.tracks[2].title, "To All the Critics (Preamble)");
        assert_eq!(album.tracks[2].isrc, None);
        assert_eq!(
            id_pairs(&album.tracks[2].ids),
            vec![("mb-recording", "1f0ce4ab-e00a-4a8d-9d23-00c963897198")]
        );
    }

    #[test]
    fn a_release_date_gives_the_reference_year() {
        assert_eq!(year(Some("1998-08-20")), Some(1998));
        assert_eq!(year(Some("2006")), Some(2006));
        assert_eq!(year(Some("")), None);
        assert_eq!(year(None), None);
        assert_eq!(year(Some("19")), None);
        assert_eq!(year(Some("whenever")), None);
    }

    #[test]
    fn a_track_without_a_title_falls_back_to_its_recording() {
        let recording = isrc_lookup().recordings[0].clone();
        let source = musicbrainz::Track {
            id: "50b02d68-e39a-331b-bac6-ece2bb3c59c0".to_string(),
            title: Some(String::new()),
            position: Some(1),
            length: None,
            artist_credit: Vec::new(),
            recording: Some(recording),
        };
        let reference = AlbumRef {
            id: "6360f948-17d1-4135-a580-34dc200930d8".to_string(),
            title: "Music Has the Right to Children".to_string(),
            artists: Vec::new(),
            year: None,
            images: Vec::new(),
        };

        let track = track(&source, Some(1), &reference);

        assert_eq!(track.title, "Wildlife Analysis");
        assert_eq!(track.duration_ms, Some(77_426));
        assert_eq!(track.artists.len(), 1);
        assert_eq!(track.artists[0].name, "Boards of Canada");
    }

    #[test]
    fn a_track_without_a_recording_keeps_what_it_has() {
        let source = musicbrainz::Track {
            id: "9e3839f8-94e2-4afd-90bb-b940971923f0".to_string(),
            title: Some("Roygbiv".to_string()),
            position: Some(11),
            length: Some(139_253),
            artist_credit: vec![musicbrainz::ArtistCredit {
                name: "Boards of Canada".to_string(),
                artist: None,
            }],
            recording: None,
        };
        let reference = AlbumRef {
            id: "ebcf935f-f49c-42e3-84e4-1007201bdca8".to_string(),
            title: "The Cornerstone Player 044".to_string(),
            artists: Vec::new(),
            year: Some(2003),
            images: Vec::new(),
        };

        let track = track(&source, Some(2), &reference);

        assert_eq!(track.title, "Roygbiv");
        assert_eq!(track.duration_ms, Some(139_253));
        assert_eq!(track.track_number, Some(11));
        assert_eq!(track.isrc, None);
        assert!(track.ids.is_empty());
        assert_eq!(track.artists[0].name, "Boards of Canada");
        assert!(track.artists[0].ids.is_empty());
    }
}
