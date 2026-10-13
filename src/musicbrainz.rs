//! Serde types for the MusicBrainz web service responses.
//!
//! The types cover the parts of the `JSON` serialization this plugin reads and
//! ignore the rest: the web service embeds aliases, life spans, areas and other
//! detail in almost every entity, and a plugin that resolved a recording does
//! not need any of it.
//!
//! The field names follow the hyphenated JSON keys with `serde` renames, so the
//! structs read like the [web service
//! documentation](https://musicbrainz.org/doc/MusicBrainz_API). Every list is
//! `#[serde(default)]` because the web service omits an empty list rather than
//! sending `[]`.

use serde::Deserialize;

/// An artist as the web service embeds it in an artist credit.
#[derive(Debug, Clone, Deserialize)]
pub struct Artist {
    /// MusicBrainz artist id, when the credit could be resolved to one.
    ///
    /// It is optional because the web service leaves it out for credits it
    /// cannot link, such as `[traditional]` or an unlinked guest.
    pub id: Option<String>,
    /// Artist name.
    pub name: String,
}

/// One credited artist of a recording, release or track.
#[derive(Debug, Clone, Deserialize)]
pub struct ArtistCredit {
    /// Name as credited, which includes the artist's name and any join phrase
    /// the web service folded into it.
    pub name: String,
    /// The artist behind the credit, when the web service resolved one.
    pub artist: Option<Artist>,
}

/// A genre as the web service embeds it in an entity.
#[derive(Debug, Clone, Deserialize)]
pub struct Genre {
    /// Genre name, for example `electronic` or `idm`.
    pub name: String,
}

/// A release group as the web service embeds it in a release.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseGroup {
    /// MusicBrainz release group id.
    pub id: Option<String>,
    /// Primary type of the release group, for example `Album` or `Single`.
    #[serde(rename = "primary-type")]
    pub primary_type: Option<String>,
    /// Genres the release group is tagged with, which stand in for a release
    /// that carries none of its own.
    #[serde(default)]
    pub genres: Vec<Genre>,
}

/// A release as the web service embeds it in a recording.
///
/// The same shape carries a release summary in a recording search result, in a
/// recording lookup with `inc=releases`, and in a release browse listing; the
/// fields a given endpoint omits are `None` or empty.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseStub {
    /// MusicBrainz release id.
    pub id: String,
    /// Release title.
    pub title: String,
    /// Release date, as complete as the web service knows it (`1998-08-20`,
    /// `1998-08` or `1998`).
    pub date: Option<String>,
    /// Number of tracks on the release.
    #[serde(rename = "track-count")]
    pub track_count: Option<u32>,
    /// The release group this release belongs to.
    #[serde(rename = "release-group")]
    pub release_group: Option<ReleaseGroup>,
}

/// A recording as the web service returns it.
///
/// A recording search result, an ISRC lookup result, a recording lookup and the
/// recording embedded in a release track all deserialize into this type; the
/// fields an endpoint does not return stay `None` or empty.
#[derive(Debug, Clone, Deserialize)]
pub struct Recording {
    /// MusicBrainz recording id.
    pub id: String,
    /// Recording title.
    pub title: String,
    /// Recording length in milliseconds, when the web service knows it.
    ///
    /// Note that the `length` of the media track that carries a recording is a
    /// different measurement, and is what a release's track list reports.
    pub length: Option<u32>,
    /// International Standard Recording Codes assigned to the recording.
    #[serde(default)]
    pub isrcs: Vec<String>,
    /// The artists credited on the recording.
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<ArtistCredit>,
    /// The releases the recording appears on, as far as the endpoint returned
    /// them.
    #[serde(default)]
    pub releases: Vec<ReleaseStub>,
    /// Relevance the search index reported for this recording, in `0..=100`.
    ///
    /// Only a search result carries it, and the web service has serialized it
    /// both as a number and as a string over its history, so both are accepted.
    #[serde(default, deserialize_with = "deserialize_score")]
    pub score: Option<f32>,
}

/// Deserializes a search score that may be a JSON number or a JSON string.
///
/// # Parameters
///
/// - `deserializer`: the deserializer positioned on the `score` value.
///
/// # Returns
///
/// The score as a number, or `None` for a missing, null or unparseable value.
///
/// # Errors
///
/// Returns the deserializer's error when the value is not JSON at all.
fn deserialize_score<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;

    Ok(match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(number)) => number.as_f64().map(|score| score as f32),
        Some(serde_json::Value::String(score)) => score.parse::<f32>().ok(),
        Some(_) => None,
    })
}

/// The response of an ISRC lookup.
#[derive(Debug, Clone, Deserialize)]
pub struct IsrcLookup {
    /// The ISRC that was looked up, echoed by the web service.
    pub isrc: Option<String>,
    /// The recordings assigned that ISRC.
    #[serde(default)]
    pub recordings: Vec<Recording>,
}

/// The response of a recording search.
#[derive(Debug, Clone, Deserialize)]
pub struct RecordingSearch {
    /// Total number of recordings the index matched, which may exceed the
    /// number returned.
    pub count: Option<u32>,
    /// The matching recordings, best match first.
    #[serde(default)]
    pub recordings: Vec<Recording>,
}

/// One track of a release medium.
///
/// The web service omits `title` when the track is named after its recording,
/// and the recording itself is only embedded when the request asked for
/// `inc=recordings`.
#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    /// MusicBrainz id of the track, which identifies it on this release.
    pub id: String,
    /// Track title as it appears on the release.
    pub title: Option<String>,
    /// Position of the track on its medium, starting at 1.
    pub position: Option<u32>,
    /// Length of the track on this medium, in milliseconds.
    pub length: Option<u32>,
    /// The artists credited on the track.
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<ArtistCredit>,
    /// The recording the track plays.
    pub recording: Option<Recording>,
}

/// One medium of a release, for example a CD or one side of a record.
#[derive(Debug, Clone, Deserialize)]
pub struct Medium {
    /// Position of the medium in the release, starting at 1.
    pub position: Option<u32>,
    /// The medium's tracks.
    #[serde(default)]
    pub tracks: Vec<Track>,
}

/// A release as a release lookup returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    /// MusicBrainz release id.
    pub id: String,
    /// Release title.
    pub title: String,
    /// Release date, as complete as the web service knows it.
    pub date: Option<String>,
    /// Barcode of the release, which the web service leaves empty when it does
    /// not know one.
    pub barcode: Option<String>,
    /// The artists credited on the release.
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<ArtistCredit>,
    /// Genres the release itself is tagged with.
    #[serde(default)]
    pub genres: Vec<Genre>,
    /// The release group this release belongs to.
    #[serde(rename = "release-group")]
    pub release_group: Option<ReleaseGroup>,
    /// The media of the release, in order.
    #[serde(default)]
    pub media: Vec<Medium>,
}

/// The response of a release browse.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseBrowse {
    /// The releases of the browsed release group, in the order the web service
    /// chose.
    #[serde(default)]
    pub releases: Vec<ReleaseStub>,
}

/// The response of the genre listing the health probe reads.
#[derive(Debug, Clone, Deserialize)]
pub struct GenrePage {
    /// The genres the web service returned; the probe only asks for one.
    #[serde(default)]
    pub genres: Vec<Genre>,
}
