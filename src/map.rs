//! Translation of MusicBrainz records into provider records.
//!
//! The functions here are pure: they take the types of [`crate::musicbrainz`]
//! and [`crate::listenbrainz`] and return the records of the
//! `hearthsound:plugin@0.5.0` `types` interface, so they are unit-tested on the
//! host target against captured web service responses.
//!
//! MusicBrainz issues its own ids and the server namespaces them per provider
//! instance, so a record's `id` field carries the MusicBrainz id as it is. The
//! canonical identifiers the server reconciles across providers - an ISRC, a
//! MusicBrainz recording, release, release group or artist id, and a release's
//! UPC - are reported in their own [`IdPair`] namespaces.
//!
//! Two conventions of the provider records are worth naming here. An album's
//! type is lowercase and follows the release group: its primary type when it
//! carries no secondary type, and the first secondary type otherwise, because a
//! compilation or a live album is what the listener needs to see. An album's
//! edition is the web service's own disambiguation of the pressing, when it has
//! one, which is the only way MusicBrainz names an edition.

use hearthsound_plugin::catalog_metadata_artwork_provider::exports::hearthsound::plugin::metadata::{
    ArtistInfo, ArtistLink, RecordingMatch, ReleaseSummary,
};
use hearthsound_plugin::types::{
    Album, AlbumRef, Artist, ArtistCredit as ProviderArtistCredit, IdPair, Image,
    Track as ProviderTrack,
};

use crate::{listenbrainz, musicbrainz, url};

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
/// date, the album type of its release group, the barcode as its UPC, the
/// release's genres (or the release group's when the release has none), the
/// web service's disambiguation of the pressing as its edition and every track
/// of every medium, in medium then track order. Each track carries the position
/// of its medium as its disc number and points back at the album. The images
/// are empty: this plugin reports cover art through the artwork layer, which
/// serves it from the Cover Art Archive.
///
/// # Example
///
/// ```text
/// album(&release)
///   -> Album { id: "6360f948-...", title: "Music Has the Right to Children", upc: Some("744861029922"), album_type: Some("album"), edition: None, .. }
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
        album_type: album_type(release.release_group.as_ref()),
        upc: non_empty(release.barcode.as_deref()),
        genres: genres(release),
        images: Vec::new(),
        tracks,
        ids: album_ids(release),
        edition: version(release.disambiguation.as_deref()),
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
/// The reference with the release id, title, artists, the year the release date
/// starts with, the album type of the release group, the release's
/// disambiguation as its edition, the track count its media add up to when the
/// response carried them, the ids of [`album_ids`] and no images.
///
/// # Example
///
/// ```text
/// album_ref(&release, &artists)
///   -> AlbumRef { id: "6360f948-...", title: "Music Has the Right to Children", year: Some(1998), track_count: Some(18), .. }
/// ```
fn album_ref(release: &musicbrainz::Release, artists: &[ProviderArtistCredit]) -> AlbumRef {
    AlbumRef {
        id: release.id.clone(),
        title: release.title.clone(),
        artists: artists.to_vec(),
        year: year(release.date.as_deref()),
        images: Vec::new(),
        album_type: album_type(release.release_group.as_ref()),
        edition: version(release.disambiguation.as_deref()),
        track_count: total_tracks(release),
        ids: album_ids(release),
    }
}

/// Builds the album reference of a release listing entry.
///
/// # Parameters
///
/// - `release`: the release as a browse or search listing returned it.
///
/// # Returns
///
/// The reference with the release id, title, artists, year, album type,
/// edition, the track count the listing reported, the ids of [`stub_ids`] and
/// no images.
///
/// # Example
///
/// ```text
/// release_ref_from_stub(&listing.releases[0])
///   -> AlbumRef { id: "5da8e677-...", title: "Geogaddi", album_type: Some("album"), year: Some(2002), .. }
/// ```
pub fn release_ref_from_stub(release: &musicbrainz::ReleaseStub) -> AlbumRef {
    AlbumRef {
        id: release.id.clone(),
        title: release.title.clone(),
        artists: artist_credits(&release.artist_credit),
        year: year(release.date.as_deref()),
        images: Vec::new(),
        album_type: album_type(release.release_group.as_ref()),
        edition: version(release.disambiguation.as_deref()),
        track_count: release
            .track_count
            .or_else(|| total_media_tracks(&release.media)),
        ids: stub_ids(release),
    }
}

/// Builds the album reference of a release lookup.
///
/// # Parameters
///
/// - `release`: the release as a lookup returned it.
///
/// # Returns
///
/// The reference of [`album_ref`], with the artists taken from the release's
/// own credits.
///
/// # Example
///
/// ```text
/// release_ref(&release)
///   -> AlbumRef { id: "6360f948-...", ids: [("mb-release", "6360f948-..."), ("mb-release-group", "17d74d52-...")], .. }
/// ```
pub fn release_ref(release: &musicbrainz::Release) -> AlbumRef {
    let artists = artist_credits(&release.artist_credit);

    album_ref(release, &artists)
}

/// Collects the ids of a release listing entry.
///
/// # Parameters
///
/// - `release`: the release to collect ids for.
///
/// # Returns
///
/// The `mb-release` id, the `mb-release-group` id when the listing carried the
/// release group, and the `upc` pair when the release has a barcode.
///
/// # Example
///
/// ```text
/// stub_ids(&listing.releases[0])
///   -> [("mb-release", "5da8e677-..."), ("mb-release-group", "8183834a-...")]
/// ```
pub fn stub_ids(release: &musicbrainz::ReleaseStub) -> Vec<IdPair> {
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

/// Returns the album type a release group names.
///
/// The type is lowercase. A release group's secondary type wins over its
/// primary type, because a compilation, a live album or a soundtrack is what
/// the listener needs to see rather than the album, EP or single the group is
/// primarily. A primary type outside those three names is reported as `other`,
/// which is the vocabulary the server knows.
///
/// # Parameters
///
/// - `group`: the release group of the release, when the response carried one.
///
/// # Returns
///
/// The type, or `None` when the release group names neither a primary nor a
/// secondary type.
///
/// # Example
///
/// ```text
/// album_type(&album_group)              -> Some("album")
/// album_type(&compilation_group)        -> Some("compilation")
/// album_type(&single_with_no_group)     -> None
/// ```
pub fn album_type(group: Option<&musicbrainz::ReleaseGroup>) -> Option<String> {
    let group = group?;

    if let Some(secondary) = group.secondary_types.first() {
        return Some(secondary_type(secondary));
    }

    group
        .primary_type
        .as_deref()
        .map(|primary| match primary.to_ascii_lowercase().as_str() {
            "album" => "album".to_string(),
            "ep" => "ep".to_string(),
            "single" => "single".to_string(),
            _ => "other".to_string(),
        })
}

/// Maps one secondary release group type to the server's vocabulary.
///
/// # Parameters
///
/// - `secondary`: the type as the web service names it, for example
///   `Compilation` or `DJ-mix`.
///
/// # Returns
///
/// The lowercase type when it is one the server knows, and `other` for every
/// other secondary type, because a secondary type always outranks the primary
/// one.
fn secondary_type(secondary: &str) -> String {
    match secondary.to_ascii_lowercase().as_str() {
        known @ ("compilation" | "live" | "remix" | "soundtrack" | "dj-mix" | "demo"
        | "broadcast" | "other") => known.to_string(),
        _ => "other".to_string(),
    }
}

/// Returns the track count the media of a release add up to.
///
/// # Parameters
///
/// - `release`: the release to read.
///
/// # Returns
///
/// The sum of the track counts its media report, or `None` when the response
/// carried no media.
fn total_tracks(release: &musicbrainz::Release) -> Option<u32> {
    total_media_tracks(&release.media)
}

/// Returns the track count a list of media adds up to.
///
/// # Parameters
///
/// - `media`: the media to count; a medium that reports no count contributes
///   the tracks it lists.
///
/// # Returns
///
/// The sum, or `None` when there are no media to count.
fn total_media_tracks(media: &[musicbrainz::Medium]) -> Option<u32> {
    if media.is_empty() {
        return None;
    }

    Some(
        media
            .iter()
            .map(|medium| {
                medium
                    .track_count
                    .unwrap_or(u32::try_from(medium.tracks.len()).unwrap_or(u32::MAX))
            })
            .sum(),
    )
}

/// Returns the edition a release's disambiguation names.
///
/// # Parameters
///
/// - `disambiguation`: the web service's disambiguation comment.
///
/// # Returns
///
/// The comment, or `None` when the web service has none for the release.
///
/// # Example
///
/// ```text
/// version(Some("2011 remaster")) -> Some("2011 remaster".to_string())
/// version(Some(""))              -> None
/// version(None)                  -> None
/// ```
fn version(disambiguation: Option<&str>) -> Option<String> {
    non_empty(disambiguation)
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
/// The web service serializes a value it does not know as an empty string,
/// which the provider records carry as an absent value instead.
///
/// # Parameters
///
/// - `value`: the value the web service returned, if any.
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
pub fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(str::to_string)
}

/// Returns the link kind of a MusicBrainz url relation type.
///
/// The web service names a link type in its own vocabulary; the server knows
/// the kinds documented on the `artist-link` record. Every type this plugin
/// does not map - `youtube`, `social network`, `image` and the rest - is
/// dropped rather than reported under an invented kind.
///
/// # Parameters
///
/// - `kind`: the relation type as the web service named it.
///
/// # Returns
///
/// The link kind, or `None` when the relation type has no kind here.
///
/// # Example
///
/// ```text
/// link_kind("official homepage") -> Some("homepage")
/// link_kind("last.fm")           -> Some("lastfm")
/// link_kind("youtube")           -> None
/// ```
pub fn link_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "official homepage" => Some("homepage"),
        "wikipedia" => Some("wikipedia"),
        "wikidata" => Some("wikidata"),
        "last.fm" => Some("lastfm"),
        "discogs" => Some("discogs"),
        "allmusic" => Some("allmusic"),
        "bandcamp" => Some("bandcamp"),
        _ => None,
    }
}

/// Converts an artist's relations into provider links.
///
/// Every relation whose type is one of the kinds [`link_kind`] maps and whose
/// target is a non-empty URL becomes one link, in the order the web service
/// returned the relations, so an artist's links read in the order the web
/// service chose.
///
/// # Parameters
///
/// - `relations`: the relations of an artist lookup.
///
/// # Returns
///
/// The links, one per mapped relation.
///
/// # Example
///
/// ```text
/// artist_links(&[ArtistRelation { kind: "wikipedia".to_string(), url: Some(RelationUrl { resource: "https://en.wikipedia.org/wiki/Northlight".to_string() }) }])
///   -> [ArtistLink { kind: "wikipedia", url: "https://en.wikipedia.org/wiki/Northlight" }]
/// ```
pub fn artist_links(relations: &[musicbrainz::ArtistRelation]) -> Vec<ArtistLink> {
    relations
        .iter()
        .filter_map(|relation| {
            let kind = link_kind(&relation.kind)?;
            let resource = relation.url.as_ref()?.resource.as_str();

            if resource.is_empty() {
                None
            } else {
                Some(ArtistLink {
                    kind: kind.to_string(),
                    url: resource.to_string(),
                })
            }
        })
        .collect()
}

/// Converts an artist lookup into artist information.
///
/// The web service has no biography of its own, so the biography and the
/// Wikipedia page it was read from come from Wikidata and Wikipedia, and the
/// caller hands them in.
///
/// # Parameters
///
/// - `artist`: the artist as an artist lookup returned it.
/// - `biography`: the biography text, when one was read.
/// - `links`: the artist's links, in the order they should be reported.
///
/// # Returns
///
/// The artist information with the `mb-artist` id and the links it was given.
///
/// # Example
///
/// ```text
/// artist_info(&lookup, None, artist_links(&lookup.relations))
///   -> ArtistInfo { ids: [("mb-artist", "d1f2e3a4-...")], biography: None, links: [...] }
/// ```
pub fn artist_info(
    artist: &musicbrainz::ArtistLookup,
    biography: Option<String>,
    links: Vec<ArtistLink>,
) -> ArtistInfo {
    ArtistInfo {
        ids: vec![id_pair(NS_ARTIST, &artist.id)],
        biography: non_empty(biography.as_deref()),
        links,
    }
}

/// Appends a link unless the list already names the same page.
///
/// # Parameters
///
/// - `links`: the links to extend.
/// - `kind`: the link kind of the entry.
/// - `url`: the url of the entry.
///
/// A page a url relation already named is not listed twice, so the Wikipedia
/// page a biography was read from does not appear beside the one MusicBrainz
/// links to.
///
/// # Example
///
/// ```text
/// let mut links = vec![ArtistLink { kind: "wikipedia".into(), url: "https://en.wikipedia.org/wiki/Northlight".into() }];
/// push_link(&mut links, "wikipedia", "https://en.wikipedia.org/wiki/Northlight");
/// // links still holds one entry
/// ```
pub fn push_link(links: &mut Vec<ArtistLink>, kind: &str, url: &str) {
    if url.is_empty() || links.iter().any(|link| link.url == url) {
        return;
    }

    links.push(ArtistLink {
        kind: kind.to_string(),
        url: url.to_string(),
    });
}

/// Converts an artist lookup into a catalog artist.
///
/// # Parameters
///
/// - `artist`: the artist as an artist lookup returned it.
/// - `images`: the renditions of the artist's portrait, narrowest first.
/// - `album_count`: the number of albums the provider reported for the artist,
///   when it reported one.
///
/// # Returns
///
/// The artist with the `mb-artist` id.
///
/// # Example
///
/// ```text
/// artist(&lookup, vec![portrait], None)
///   -> Artist { id: "d1f2e3a4-...", name: "Northlight", album_count: None, ids: [("mb-artist", "d1f2e3a4-...")] }
/// ```
pub fn artist(
    artist: &musicbrainz::ArtistLookup,
    images: Vec<Image>,
    album_count: Option<u32>,
) -> Artist {
    Artist {
        id: artist.id.clone(),
        name: artist.name.clone(),
        images,
        ids: vec![id_pair(NS_ARTIST, &artist.id)],
        album_count,
    }
}

/// Width of the Wikimedia Commons rendition of an artist portrait.
///
/// The server stores every rendition it can get and serves the size a page
/// asks for from it, so the plugin declares the width a portrait page draws.
const PORTRAIT_WIDTH: u32 = 1200;

/// Hosts the recorded redirect chain of a portrait passes through.
///
/// Recorded with `curl -sIL` on the URL [`commons_portrait`] builds, whose
/// path is `/wiki/Special:FilePath/<file>?width=1200` on
/// `commons.wikimedia.org`: Commons answers `302` to its own redirect
/// endpoint, which answers `301` to `thumb.wikimedia.org`, which serves the
/// resized file. The thumbnail host is a host of its own rather than a
/// subdomain of the Commons host, so a manifest that allows only
/// `commons.wikimedia.org` cannot fetch a single portrait; the manifest names
/// both, and the unit test of the crate holds them together.
pub const PORTRAIT_REDIRECT_HOSTS: &[&str] = &["commons.wikimedia.org", "thumb.wikimedia.org"];

/// Hosts the recorded redirect chain of a cover passes through.
///
/// Recorded with `curl -sIL` on the URL [`release_images`] builds, whose path
/// is `/release/<mbid>/front-250` on `coverartarchive.org`: the archive answers
/// `307` to `archive.org`, which answers `302` to the storage node that serves
/// the file (`dn710906.ca.archive.org` for the release recorded here, a
/// different node per item). The manifest allows `archive.org` and its
/// subdomains, which covers every node the redirect can name.
pub const COVER_REDIRECT_HOSTS: &[&str] = &[
    "coverartarchive.org",
    "archive.org",
    "dn710906.ca.archive.org",
];

/// Builds the Wikimedia Commons rendition of an artist's portrait.
///
/// # Parameters
///
/// - `file`: the file name Wikidata names, for example
///   `Boards of Canada 1999.jpg`.
///
/// # Returns
///
/// The image, at [`PORTRAIT_WIDTH`] pixels wide.
///
/// # Example
///
/// ```text
/// commons_portrait("Northlight.jpg")
///   -> Image { url: "https://commons.wikimedia.org/wiki/Special:FilePath/Northlight.jpg?width=1200", width: Some(1200), height: None }
/// ```
pub fn commons_portrait(file: &str) -> Image {
    Image {
        url: format!(
            "{}/wiki/Special:FilePath/{}?width={PORTRAIT_WIDTH}",
            url::COMMONS_BASE_URL,
            url::encode_component(file)
        ),
        width: Some(PORTRAIT_WIDTH),
        height: None,
    }
}

/// Converts a recording into a catalog track.
///
/// # Parameters
///
/// - `recording`: the recording as a search, lookup or ISRC result returned
///   it.
/// - `release`: the release the track's album reference is built from, when the
///   response named one.
/// - `isrcs`: the ISRCs to report for the recording, in order.
///
/// # Returns
///
/// The track with the recording as its id, its title, the credited artists, the
/// album reference of `release`, the recording's length as its duration, the
/// `mb-recording` id, one `isrc` pair per reported ISRC and the first reported
/// ISRC as its `isrc` field. Neither a disc nor a track number is reported,
/// because neither is read from the listing; the explicit flag stays unset,
/// because MusicBrainz does not record one.
///
/// # Example
///
/// ```text
/// recording_track(&recording, Some(&recording.releases[0]), &recording.isrcs)
///   -> Track { id: "b061ba14-...", title: "Wildlife Analysis", album: Some(AlbumRef { .. }), isrc: Some("GBBPW9800010"), .. }
/// ```
pub fn recording_track(
    recording: &musicbrainz::Recording,
    release: Option<&musicbrainz::ReleaseStub>,
    isrcs: &[String],
) -> ProviderTrack {
    let artists = artist_credits(&recording.artist_credit);
    let mut ids = vec![id_pair(NS_RECORDING, &recording.id)];
    ids.extend(isrcs.iter().map(|isrc| id_pair(NS_ISRC, isrc)));

    ProviderTrack {
        id: recording.id.clone(),
        title: recording.title.clone(),
        artists: artists.clone(),
        album: release.map(|release| AlbumRef {
            artists,
            ..release_ref_from_stub(release)
        }),
        duration_ms: recording.length,
        disc_number: None,
        track_number: None,
        isrc: isrcs.first().cloned(),
        explicit: None,
        ids,
    }
}

/// Converts an entry of an artist's popularity list into a catalog track.
///
/// # Parameters
///
/// - `entry`: the entry as ListenBrainz returned it.
///
/// # Returns
///
/// The track with the recording as its id, the recording's title, the artist the
/// list names, the recording's length as its duration, the `mb-recording` id
/// and, when ListenBrainz named the release the recording is played from, the
/// reference of that release. A track without a release stays without an album,
/// because ListenBrainz does not know every release a recording appears on.
///
/// # Example
///
/// ```text
/// popularity_track(&entry)
///   -> Track { id: "13dd61c7-...", title: "Closer", album: Some(AlbumRef { id: "ba8701ba-...", .. }), .. }
/// ```
pub fn popularity_track(entry: &listenbrainz::RecordingPopularity) -> ProviderTrack {
    let artists = vec![ProviderArtistCredit {
        name: entry.artist_name.clone(),
        ids: entry
            .artist_mbids
            .iter()
            .map(|mbid| id_pair(NS_ARTIST, mbid))
            .collect(),
    }];

    let album = match (&entry.release_mbid, &entry.release_name) {
        (Some(mbid), Some(title)) if !mbid.is_empty() && !title.is_empty() => Some(AlbumRef {
            id: mbid.clone(),
            title: title.clone(),
            artists: artists.clone(),
            year: None,
            images: Vec::new(),
            album_type: None,
            edition: None,
            track_count: None,
            ids: vec![id_pair(NS_RELEASE, mbid)],
        }),
        _ => None,
    };

    ProviderTrack {
        id: entry.recording_mbid.clone(),
        title: entry.recording_name.clone(),
        artists,
        album,
        duration_ms: entry.length,
        disc_number: None,
        track_number: None,
        isrc: None,
        explicit: None,
        ids: vec![id_pair(NS_RECORDING, &entry.recording_mbid)],
    }
}

/// Widths of the Cover Art Archive renditions of a front cover.
///
/// The archive serves each of them from the same path with the width as its
/// suffix, and a server that wants a cover picks the smallest rendition that
/// covers the place it draws it in.
const COVER_WIDTHS: [u32; 3] = [250, 500, 1200];

/// Builds the Cover Art Archive renditions of a release's front cover.
///
/// # Parameters
///
/// - `base_url`: base URL of the Cover Art Archive, with or without a trailing
///   slash.
/// - `mbid`: MusicBrainz release id.
///
/// # Returns
///
/// One image per width of [`COVER_WIDTHS`], narrowest first.
///
/// # Example
///
/// ```text
/// release_images("https://coverartarchive.org/", "6360f948-17d1-4135-a580-34dc200930d8")
///   -> [Image { url: "https://coverartarchive.org/release/6360f948-.../front-250", width: Some(250), height: None }, ...]
/// ```
pub fn release_images(base_url: &str, mbid: &str) -> Vec<Image> {
    cover_images(base_url, &format!("release/{mbid}"))
}

/// Builds the Cover Art Archive renditions of a release group's front cover.
///
/// The archive serves a release group's cover at a path of its own, which is
/// the cover of the group's releases rather than of one pressing.
///
/// # Parameters
///
/// - `base_url`: base URL of the Cover Art Archive, with or without a trailing
///   slash.
/// - `mbid`: MusicBrainz release group id.
///
/// # Returns
///
/// One image per width of [`COVER_WIDTHS`], narrowest first.
///
/// # Example
///
/// ```text
/// release_group_images("https://coverartarchive.org/", "17d74d52-c92b-3b8d-9f87-218ab2d1c4a0")
///   -> [Image { url: "https://coverartarchive.org/release-group/17d74d52-.../front-250", width: Some(250), height: None }, ...]
/// ```
pub fn release_group_images(base_url: &str, mbid: &str) -> Vec<Image> {
    cover_images(base_url, &format!("release-group/{mbid}"))
}

/// Builds the renditions of one Cover Art Archive front cover.
///
/// # Parameters
///
/// - `base_url`: base URL of the Cover Art Archive.
/// - `path`: the archive path of the cover, without a leading or trailing
///   slash.
///
/// # Returns
///
/// One image per width of [`COVER_WIDTHS`], narrowest first. The archive
/// reports no height for a rendition it builds on demand, so the height is
/// left unknown.
fn cover_images(base_url: &str, path: &str) -> Vec<Image> {
    COVER_WIDTHS
        .iter()
        .map(|width| Image {
            url: url::endpoint(base_url, &format!("{path}/front-{width}")),
            width: Some(*width),
            height: None,
        })
        .collect()
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
            status: Some("Official".to_string()),
            barcode: None,
            disambiguation: None,
            track_count: Some(30),
            artist_credit: Vec::new(),
            media: Vec::new(),
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
        assert_eq!(album.album_type.as_deref(), Some("album"));
        assert_eq!(album.edition, None);
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
        // The release group is a compilation, and a secondary type wins over
        // the group's primary type.
        assert_eq!(album.album_type.as_deref(), Some("compilation"));
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
            album_type: Some("album".to_string()),
            edition: None,
            track_count: Some(18),
            ids: Vec::new(),
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
            album_type: Some("album".to_string()),
            edition: None,
            track_count: Some(4),
            ids: Vec::new(),
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

    fn artist_lookup() -> musicbrainz::ArtistLookup {
        serde_json::from_str(include_str!("fixtures/artist_lookup.json")).unwrap()
    }

    /// Renders links as tuples, because the generated `ArtistLink` is not
    /// comparable.
    fn links(links: &[ArtistLink]) -> Vec<(&str, &str)> {
        links
            .iter()
            .map(|link| (link.kind.as_str(), link.url.as_str()))
            .collect()
    }

    #[test]
    fn maps_an_artist_lookup_into_artist_information() {
        let artist = artist_lookup();
        let info = artist_info(&artist, None, artist_links(&artist.relations));

        assert_eq!(
            id_pairs(&info.ids),
            vec![("mb-artist", "d1f2e3a4-5b6c-4d7e-8f90-1a2b3c4d5e6f")]
        );
        assert_eq!(info.biography, None);
        assert_eq!(
            links(&info.links),
            vec![
                ("homepage", "https://northlight.example/"),
                ("wikipedia", "https://en.wikipedia.org/wiki/Northlight"),
                ("wikidata", "https://www.wikidata.org/wiki/Q1000000"),
                ("lastfm", "https://www.last.fm/music/Northlight"),
                ("discogs", "https://www.discogs.com/artist/1000000"),
                (
                    "allmusic",
                    "https://www.allmusic.com/artist/northlight-mn0000000000"
                ),
                ("bandcamp", "https://northlight.bandcamp.com/"),
            ]
        );
    }

    #[test]
    fn it_links_the_wikipedia_page_a_biography_came_from() {
        let artist = artist_lookup();
        let mut links = artist_links(&artist.relations);

        push_link(
            &mut links,
            "wikipedia",
            "https://en.wikipedia.org/wiki/Northlight",
        );
        push_link(
            &mut links,
            "wikipedia",
            "https://en.wikipedia.org/wiki/Northlight",
        );
        push_link(&mut links, "wikipedia", "");

        // The page a url relation already named is not listed twice, and an
        // empty url carries no link.
        assert_eq!(
            links
                .iter()
                .filter(|link| link.url == "https://en.wikipedia.org/wiki/Northlight")
                .count(),
            1
        );
        assert!(
            links.iter().all(|link| !link.url.is_empty()),
            "an empty url carries no link"
        );
    }

    #[test]
    fn artist_information_carries_the_biography_it_was_given() {
        let artist = artist_lookup();
        let info = artist_info(
            &artist,
            Some("Northlight are a Scottish electronic music duo.".to_string()),
            artist_links(&artist.relations),
        );

        assert_eq!(
            info.biography.as_deref(),
            Some("Northlight are a Scottish electronic music duo.")
        );
        assert_eq!(info.links.len(), 7);
    }

    #[test]
    fn an_artist_record_carries_its_ids_its_portrait_and_its_album_count() {
        let lookup = artist_lookup();
        let mapped = self::artist(
            &lookup,
            vec![commons_portrait("Northlight live 1999.jpg")],
            Some(9),
        );

        assert_eq!(mapped.id, "d1f2e3a4-5b6c-4d7e-8f90-1a2b3c4d5e6f");
        assert_eq!(mapped.name, "Northlight");
        assert_eq!(mapped.album_count, Some(9));
        assert_eq!(mapped.images.len(), 1);
        assert_eq!(mapped.images[0].width, Some(1200));
        assert_eq!(
            id_pairs(&mapped.ids),
            vec![("mb-artist", "d1f2e3a4-5b6c-4d7e-8f90-1a2b3c4d5e6f")]
        );
    }

    #[test]
    fn a_release_listing_entry_becomes_a_reference_with_its_type_and_ids() {
        let browse: musicbrainz::ReleaseBrowse =
            serde_json::from_str(include_str!("fixtures/artist_releases.json")).unwrap();
        let reference = release_ref_from_stub(&browse.releases[0]);

        assert_eq!(reference.id, "3fdcf49c-e297-4c21-880d-dba4c40d98dd");
        assert_eq!(reference.title, "Music Has the Right to Children");
        assert_eq!(reference.year, Some(1998));
        assert_eq!(reference.album_type.as_deref(), Some("album"));
        assert_eq!(reference.edition, None);
        // The release's two media hold ten and seven tracks.
        assert_eq!(reference.track_count, Some(17));
        assert_eq!(reference.artists[0].name, "Boards of Canada");
        assert_eq!(
            id_pairs(&reference.ids),
            vec![
                ("mb-release", "3fdcf49c-e297-4c21-880d-dba4c40d98dd"),
                ("mb-release-group", "17d74d52-c92b-3b8d-9f87-218ab2d1c4a0"),
                ("upc", "5021603055117"),
            ]
        );
    }

    #[test]
    fn a_reference_reports_the_edition_a_release_names() {
        let release: musicbrainz::Release =
            serde_json::from_str(include_str!("fixtures/release_ref.json")).unwrap();
        let reference = release_ref(&release);

        assert_eq!(reference.id, "1d7d2318-8ac2-34d5-bd5b-ca7aad59e4ba");
        assert_eq!(reference.album_type.as_deref(), Some("album"));
        assert_eq!(reference.edition.as_deref(), Some("limited edition"));
        assert_eq!(reference.track_count, Some(23));
        assert_eq!(
            id_pairs(&reference.ids),
            vec![
                ("mb-release", "1d7d2318-8ac2-34d5-bd5b-ca7aad59e4ba"),
                ("mb-release-group", "8183834a-6c8a-39ef-a7bc-a82f79b929e9"),
                ("upc", "801061810122"),
            ]
        );
    }

    #[test]
    fn a_barcode_search_hit_becomes_a_reference_carrying_its_upc() {
        let search: musicbrainz::ReleaseSearch =
            serde_json::from_str(include_str!("fixtures/barcode_release_search.json")).unwrap();
        let reference = release_ref_from_stub(&search.releases[0]);

        assert_eq!(reference.id, "6360f948-17d1-4135-a580-34dc200930d8");
        assert_eq!(reference.track_count, Some(18));
        assert!(id_pairs(&reference.ids).contains(&("upc", "744861029922")));
    }

    #[test]
    fn the_album_type_follows_the_release_group() {
        let group = |primary: Option<&str>, secondary: &[&str]| musicbrainz::ReleaseGroup {
            id: Some("8183834a-6c8a-39ef-a7bc-a82f79b929e9".to_string()),
            primary_type: primary.map(str::to_string),
            secondary_types: secondary.iter().map(|value| value.to_string()).collect(),
            genres: Vec::new(),
        };

        assert_eq!(
            album_type(Some(&group(Some("Album"), &[]))).as_deref(),
            Some("album")
        );
        assert_eq!(
            album_type(Some(&group(Some("EP"), &[]))).as_deref(),
            Some("ep")
        );
        assert_eq!(
            album_type(Some(&group(Some("Single"), &[]))).as_deref(),
            Some("single")
        );
        assert_eq!(
            album_type(Some(&group(Some("Album"), &["Compilation", "Demo"]))).as_deref(),
            Some("compilation")
        );
        assert_eq!(
            album_type(Some(&group(Some("Album"), &["DJ-mix"]))).as_deref(),
            Some("dj-mix")
        );
        assert_eq!(
            album_type(Some(&group(Some("Single"), &["Soundtrack"]))).as_deref(),
            Some("soundtrack")
        );
        assert_eq!(
            album_type(Some(&group(Some("Broadcast"), &[]))).as_deref(),
            Some("other")
        );
        assert_eq!(
            album_type(Some(&group(Some("Album"), &["Mixtape/Street"]))).as_deref(),
            Some("other")
        );
        assert_eq!(album_type(Some(&group(None, &[]))), None);
        assert_eq!(album_type(None), None);
    }

    #[test]
    fn a_recording_becomes_a_track_pointing_at_its_release() {
        let recording = recording_lookup();
        let album = crate::select::earliest_official(&recording.releases)
            .unwrap()
            .clone();
        let track = recording_track(&recording, Some(&album), &recording.isrcs);

        assert_eq!(track.id, recording.id);
        assert_eq!(track.title, "Wildlife Analysis");
        assert_eq!(track.duration_ms, Some(77_426));
        assert_eq!(track.disc_number, None);
        assert_eq!(track.track_number, None);
        assert_eq!(track.isrc.as_deref(), Some("GBBPW9800010"));
        assert_eq!(
            id_pairs(&track.ids),
            vec![
                ("mb-recording", "b061ba14-6b3b-4739-a043-dd4685357af4"),
                ("isrc", "GBBPW9800010"),
            ]
        );

        let reference = track.album.as_ref().unwrap();
        assert_eq!(reference.title, "Boc Maxima");
        assert_eq!(reference.year, Some(1996));
        assert!(id_pairs(&reference.ids).contains(&("mb-release", &reference.id)));
    }

    #[test]
    fn a_recording_whose_releases_are_all_promotions_names_no_album() {
        let search = search();
        let recording = &search.recordings[0];
        let track = recording_track(
            recording,
            crate::select::first_official(&recording.releases),
            &[],
        );

        assert_eq!(track.title, "Roygbiv");
        assert!(track.album.is_none());
    }

    #[test]
    fn an_isrc_adds_its_pairs_to_the_track() {
        let lookup = isrc_lookup();
        let recording = &lookup.recordings[0];
        let track = recording_track(recording, None, &["GBBPW9800010".to_string()]);

        assert_eq!(track.isrc.as_deref(), Some("GBBPW9800010"));
        assert_eq!(
            id_pairs(&track.ids),
            vec![
                ("mb-recording", "b061ba14-6b3b-4739-a043-dd4685357af4"),
                ("isrc", "GBBPW9800010"),
            ]
        );
        assert!(track.album.is_none());
    }

    #[test]
    fn a_popularity_entry_becomes_a_track_with_the_release_it_names() {
        let entries: Vec<listenbrainz::RecordingPopularity> =
            serde_json::from_str(include_str!("fixtures/artist_popularity.json")).unwrap();

        let first = popularity_track(&entries[0]);
        assert_eq!(first.id, "13dd61c7-ce73-4e97-9f0c-9f0e53144411");
        assert_eq!(first.title, "Closer");
        assert_eq!(first.duration_ms, Some(373_133));
        assert_eq!(first.artists[0].name, "Nine Inch Nails");
        assert_eq!(
            id_pairs(&first.artists[0].ids),
            vec![("mb-artist", "b7ffd2af-418f-4be2-bdd1-22f8b48613da")]
        );
        let album = first.album.as_ref().unwrap();
        assert_eq!(album.id, "ba8701ba-dc7c-4bca-9c83-846ee8c3d576");
        assert_eq!(album.title, "The Downward Spiral");
        assert_eq!(
            id_pairs(&album.ids),
            vec![("mb-release", album.id.as_str())]
        );

        let second = popularity_track(&entries[1]);
        assert!(second.album.is_none());
        assert_eq!(second.isrc, None);
    }

    #[test]
    fn relation_types_without_a_link_kind_are_dropped() {
        // The fixture's artist also has a social network page, an image and a
        // band membership; none of the three is a link the server knows.
        let artist = artist_lookup();

        assert_eq!(artist.relations.len(), 10);
        assert_eq!(link_kind("social network"), None);
        assert_eq!(link_kind("image"), None);
        assert_eq!(link_kind("member of band"), None);
    }

    #[test]
    fn a_relation_without_a_usable_url_carries_no_link() {
        let relations = vec![
            musicbrainz::ArtistRelation {
                kind: "official homepage".to_string(),
                url: None,
            },
            musicbrainz::ArtistRelation {
                kind: "wikipedia".to_string(),
                url: Some(musicbrainz::RelationUrl {
                    resource: String::new(),
                }),
            },
            musicbrainz::ArtistRelation {
                kind: "last.fm".to_string(),
                url: Some(musicbrainz::RelationUrl {
                    resource: "https://www.last.fm/music/Northlight".to_string(),
                }),
            },
        ];

        assert_eq!(
            links(&artist_links(&relations)),
            vec![("lastfm", "https://www.last.fm/music/Northlight")]
        );
    }

    #[test]
    fn every_mapped_relation_type_has_a_link_kind() {
        assert_eq!(link_kind("official homepage"), Some("homepage"));
        assert_eq!(link_kind("wikipedia"), Some("wikipedia"));
        assert_eq!(link_kind("wikidata"), Some("wikidata"));
        assert_eq!(link_kind("last.fm"), Some("lastfm"));
        assert_eq!(link_kind("discogs"), Some("discogs"));
        assert_eq!(link_kind("allmusic"), Some("allmusic"));
        assert_eq!(link_kind("bandcamp"), Some("bandcamp"));
        assert_eq!(link_kind("youtube"), None);
        assert_eq!(link_kind(""), None);
    }

    /// Renders images as url and width pairs, because the generated `Image` is
    /// not comparable.
    fn images(images: &[Image]) -> Vec<(&str, Option<u32>)> {
        images
            .iter()
            .map(|image| (image.url.as_str(), image.width))
            .collect()
    }

    #[test]
    fn a_release_cover_has_one_rendition_per_width() {
        let built = release_images(
            "https://coverartarchive.org/",
            "6360f948-17d1-4135-a580-34dc200930d8",
        );

        assert_eq!(
            images(&built),
            vec![
                (
                    "https://coverartarchive.org/release/6360f948-17d1-4135-a580-34dc200930d8/front-250",
                    Some(250)
                ),
                (
                    "https://coverartarchive.org/release/6360f948-17d1-4135-a580-34dc200930d8/front-500",
                    Some(500)
                ),
                (
                    "https://coverartarchive.org/release/6360f948-17d1-4135-a580-34dc200930d8/front-1200",
                    Some(1200)
                ),
            ]
        );
    }

    #[test]
    fn a_release_group_cover_is_served_under_its_own_path() {
        let built = release_group_images(
            "https://coverartarchive.org",
            "17d74d52-c92b-3b8d-9f87-218ab2d1c4a0",
        );

        assert_eq!(
            images(&built),
            vec![
                (
                    "https://coverartarchive.org/release-group/17d74d52-c92b-3b8d-9f87-218ab2d1c4a0/front-250",
                    Some(250)
                ),
                (
                    "https://coverartarchive.org/release-group/17d74d52-c92b-3b8d-9f87-218ab2d1c4a0/front-500",
                    Some(500)
                ),
                (
                    "https://coverartarchive.org/release-group/17d74d52-c92b-3b8d-9f87-218ab2d1c4a0/front-1200",
                    Some(1200)
                ),
            ]
        );
    }

    #[test]
    fn a_cover_base_url_of_a_mirror_is_used_as_it_is() {
        let built = release_images(
            " https://archive.example/cover-art// ",
            "6360f948-17d1-4135-a580-34dc200930d8",
        );

        assert_eq!(
            images(&built)[0].0,
            "https://archive.example/cover-art/release/6360f948-17d1-4135-a580-34dc200930d8/front-250"
        );
    }
}
