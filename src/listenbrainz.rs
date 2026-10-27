//! Serde types for the ListenBrainz popularity endpoints.
//!
//! ListenBrainz ranks the recordings of an artist by how often its listeners
//! played them, which is the closest thing the public services have to the
//! artist's top tracks. The endpoint answers with an array of entries rather
//! than an object, and every field but the recording's own is optional because
//! an entry may come without the release it was played from.
//!
//! Only the parts this plugin reads are typed; the cover art identifiers and
//! the colour a client may draw are ignored.

use serde::Deserialize;

/// One recording of an artist's popularity list.
#[derive(Debug, Clone, Deserialize)]
pub struct RecordingPopularity {
    /// MusicBrainz ids of the artists credited on the recording.
    #[serde(default, rename = "artist_mbids")]
    pub artist_mbids: Vec<String>,
    /// Name of the artist the list belongs to.
    #[serde(default)]
    pub artist_name: String,
    /// MusicBrainz id of the recording.
    pub recording_mbid: String,
    /// Title of the recording.
    pub recording_name: String,
    /// MusicBrainz id of the release the recording is played from, when
    /// ListenBrainz named one.
    #[serde(default)]
    pub release_mbid: Option<String>,
    /// Title of that release, when ListenBrainz named one.
    #[serde(default)]
    pub release_name: Option<String>,
    /// Length of the recording in milliseconds, when ListenBrainz knows it.
    #[serde(default)]
    pub length: Option<u32>,
    /// Number of times the recording was played, across all listeners.
    #[serde(default)]
    pub total_listen_count: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn popularity() -> Vec<RecordingPopularity> {
        serde_json::from_str(include_str!("fixtures/artist_popularity.json")).unwrap()
    }

    #[test]
    fn reads_the_entries_of_a_popularity_list() {
        let entries = popularity();

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].recording_mbid,
            "13dd61c7-ce73-4e97-9f0c-9f0e53144411"
        );
        assert_eq!(entries[0].recording_name, "Closer");
        assert_eq!(entries[0].artist_name, "Nine Inch Nails");
        assert_eq!(
            entries[0].artist_mbids,
            vec!["b7ffd2af-418f-4be2-bdd1-22f8b48613da".to_string()]
        );
        assert_eq!(
            entries[0].release_mbid.as_deref(),
            Some("ba8701ba-dc7c-4bca-9c83-846ee8c3d576")
        );
        assert_eq!(
            entries[0].release_name.as_deref(),
            Some("The Downward Spiral")
        );
        assert_eq!(entries[0].length, Some(373_133));
        assert_eq!(entries[0].total_listen_count, Some(1_380_798));
    }

    #[test]
    fn an_entry_without_a_release_keeps_no_release() {
        let entries = popularity();

        assert_eq!(entries[1].recording_name, "The Hand That Feeds");
        assert_eq!(entries[1].release_mbid, None);
        assert_eq!(entries[1].release_name, None);
    }
}
