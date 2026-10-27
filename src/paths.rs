//! Paths of the web service endpoints the plugin reads.
//!
//! Every function here composes one path - the part after the configured base
//! URL - with the inclusions the caller needs and the JSON format. Nothing here
//! touches the network, so the composition is unit-tested on the host target,
//! and the three layers (catalog, metadata, artwork) share one spelling of each
//! endpoint.
//!
//! Identifiers and query text are percent-encoded with
//! [`crate::url::encode_component`], so a value can never add a parameter or
//! change the endpoint it is pasted into.

use crate::{query, url};

/// Number of releases one browse page holds.
///
/// It is the web service's maximum page size, so a release group with at most
/// this many official releases is read with one request.
pub const BROWSE_LIMIT: u32 = 100;

/// Number of artists an artist search asks for.
///
/// A search for an artist name returns homages, cover bands and namesakes
/// before or after the artist itself, and the web service orders them by
/// relevance, so the first few hits are the only ones worth reading.
pub const ARTIST_SEARCH_LIMIT: u32 = 5;

/// Number of releases a barcode search asks for.
///
/// A barcode either names one release or none, but a reissue and its original
/// pressing can share one, and the first result is the one to read.
pub const BARCODE_SEARCH_LIMIT: u32 = 5;

/// Builds the path of an artist lookup.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz artist id.
///
/// # Returns
///
/// The path, with the url relations requested and the JSON format.
pub fn artist_path(mbid: &str) -> String {
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
/// The path, with the name searched as an artist term, the query encoded and
/// the result page bounded by [`ARTIST_SEARCH_LIMIT`].
pub fn artist_search_path(name: &str) -> String {
    let text = format!("artist:\"{}\"", query::escape_lucene(name));

    format!(
        "artist?query={}&limit={}&fmt=json",
        url::encode_component(&text),
        ARTIST_SEARCH_LIMIT
    )
}

/// Builds the path of an artist search with the caller's own page.
///
/// A catalog search carries the page the caller asked for, so it cannot use the
/// bounded page of [`artist_search_path`].
///
/// # Parameters
///
/// - `text`: the Lucene query, unencoded.
/// - `limit`: number of artists to ask for.
/// - `offset`: number of artists to skip.
///
/// # Returns
///
/// The path, with the query encoded and the page set.
pub fn artist_query_path(text: &str, limit: u32, offset: u32) -> String {
    format!(
        "artist?query={}&limit={}&offset={}&fmt=json",
        url::encode_component(text),
        limit,
        offset
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
pub fn isrc_path(isrc: &str) -> String {
    format!(
        "isrc/{}?inc=artist-credits+releases&fmt=json",
        url::encode_component(isrc)
    )
}

/// Builds the path of a recording lookup read for its releases.
///
/// The inclusions are the ones a recording needs beyond what an ISRC lookup
/// returns: its releases, their release groups, and its ISRCs.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz recording id.
///
/// # Returns
///
/// The path, with the requested inclusions and the JSON format.
pub fn recording_path(mbid: &str) -> String {
    format!(
        "recording/{}?inc=releases+release-groups+isrcs&fmt=json",
        url::encode_component(mbid)
    )
}

/// Builds the path of a recording lookup that also reads the recording's
/// credits.
///
/// A catalog track carries the artists credited on the recording itself, which
/// the identity lookup of [`recording_path`] does not ask for.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz recording id.
///
/// # Returns
///
/// The path, with the requested inclusions and the JSON format.
pub fn recording_credit_path(mbid: &str) -> String {
    format!(
        "recording/{}?inc=artist-credits+releases+isrcs&fmt=json",
        url::encode_component(mbid)
    )
}

/// Builds the path of a recording search with the caller's own page.
///
/// # Parameters
///
/// - `text`: the Lucene query, unencoded.
/// - `limit`: number of recordings to ask for.
/// - `offset`: number of recordings to skip.
///
/// # Returns
///
/// The path, with the query encoded and the page set.
pub fn recording_query_path(text: &str, limit: u32, offset: u32) -> String {
    format!(
        "recording?query={}&limit={}&offset={}&fmt=json",
        url::encode_component(text),
        limit,
        offset
    )
}

/// Builds the path of a recording search bounded by [`query::SEARCH_LIMIT`].
///
/// # Parameters
///
/// - `text`: the Lucene query, unencoded.
///
/// # Returns
///
/// The path, with the query encoded and the result page bounded.
pub fn recording_search_path(text: &str) -> String {
    format!(
        "recording?query={}&fmt=json&limit={}",
        url::encode_component(text),
        query::SEARCH_LIMIT
    )
}

/// Builds the path of a release lookup that materializes a whole album.
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
pub fn release_path(mbid: &str) -> String {
    format!(
        "release/{}?inc=recordings+artist-credits+isrcs+release-groups+genres&fmt=json",
        url::encode_component(mbid)
    )
}

/// Builds the path of a release lookup that reads only what an album reference
/// needs.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz release id.
///
/// # Returns
///
/// The path, with the artist credits, the release group and the media
/// requested, which are the parts of a release a coarse reference reports: who
/// made it, what kind of release it is, and how many tracks it holds.
pub fn release_ref_path(mbid: &str) -> String {
    format!(
        "release/{}?inc=artist-credits+release-groups+media&fmt=json",
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
pub fn release_group_path(mbid: &str, offset: u32) -> String {
    format!(
        "release?release-group={}&status=official&fmt=json&limit={}&offset={}",
        url::encode_component(mbid),
        BROWSE_LIMIT,
        offset
    )
}

/// Builds the path of a page of an artist's official album, EP and single
/// releases.
///
/// The browse asks for the artist credits, the release groups and the media of
/// each release, because an album reference reports its artists, its type, its
/// edition and its track count.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz artist id.
/// - `offset`: number of releases of the artist to skip, which selects the
///   page.
///
/// # Returns
///
/// The path, with the status and type filters, the page size and the offset.
pub fn artist_releases_path(mbid: &str, offset: u32) -> String {
    format!(
        "release?artist={}&status=official&type=album|ep|single&inc=release-groups+artist-credits+media&fmt=json&limit={}&offset={}",
        url::encode_component(mbid),
        BROWSE_LIMIT,
        offset
    )
}

/// Builds the path of a release search with the caller's own page.
///
/// # Parameters
///
/// - `text`: the Lucene query, unencoded.
/// - `limit`: number of releases to ask for.
/// - `offset`: number of releases to skip.
///
/// # Returns
///
/// The path, with the release groups and the artist credits requested, because
/// an album search deduplicates its hits by release group and reports the
/// artists of the release it keeps, and with the page set.
pub fn release_query_path(text: &str, limit: u32, offset: u32) -> String {
    format!(
        "release?query={}&inc=release-groups&fmt=json&limit={}&offset={}",
        url::encode_component(text),
        limit,
        offset
    )
}

/// Number of release groups a release group search asks for.
///
/// An album title and artist that match a group exactly rank above every other
/// hit, so a handful of hits is enough to find it or to see that it is absent.
pub const RELEASE_GROUP_SEARCH_LIMIT: u32 = 5;

/// Builds the path of a release group search.
///
/// # Parameters
///
/// - `text`: the Lucene query, unencoded.
///
/// # Returns
///
/// The path, with the query encoded and the result page bounded by
/// [`RELEASE_GROUP_SEARCH_LIMIT`].
pub fn release_group_search_path(text: &str) -> String {
    format!(
        "release-group?query={}&limit={}&fmt=json",
        url::encode_component(text),
        RELEASE_GROUP_SEARCH_LIMIT
    )
}

/// Builds the path of a release search for the release that carries a barcode.
///
/// # Parameters
///
/// - `text`: the Lucene query, unencoded.
///
/// # Returns
///
/// The path, with the query encoded and the result page bounded by
/// [`BARCODE_SEARCH_LIMIT`].
pub fn barcode_search_path(text: &str) -> String {
    format!(
        "release?query={}&limit={}&fmt=json",
        url::encode_component(text),
        BARCODE_SEARCH_LIMIT
    )
}

/// Builds the path of the genre listing the health probe reads.
///
/// # Returns
///
/// The path of the genre table, bounded to a single row.
pub fn health_path() -> String {
    "genre/all?fmt=json&limit=1".to_string()
}

/// Builds the path of the ListenBrainz popularity list of an artist.
///
/// # Parameters
///
/// - `mbid`: MusicBrainz artist id.
///
/// # Returns
///
/// The path under [`url::LISTENBRAINZ_BASE_URL`], with the JSON format.
pub fn listenbrainz_top_recordings_path(mbid: &str) -> String {
    format!(
        "1/popularity/top-recordings-for-artist/{}",
        url::encode_component(mbid)
    )
}

/// Builds the path of a Wikidata entity's JSON serialization.
///
/// # Parameters
///
/// - `qid`: Wikidata item id, for example `Q1000000`.
///
/// # Returns
///
/// The path under [`url::WIKIDATA_BASE_URL`].
pub fn wikidata_entity_path(qid: &str) -> String {
    format!(
        "wiki/Special:EntityData/{}.json",
        url::encode_component(qid)
    )
}

/// Builds the path of the Wikipedia REST summary of a page.
///
/// # Parameters
///
/// - `title`: page title as the edition spells it, with spaces.
///
/// # Returns
///
/// The path under the edition's base URL, with the title's spaces written as
/// underscores.
pub fn wikipedia_summary_path(title: &str) -> String {
    format!(
        "api/rest_v1/page/summary/{}",
        url::encode_component(&title.replace(' ', "_"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_catalog_recording_lookup_asks_for_the_credits() {
        assert_eq!(
            recording_credit_path("b061ba14-6b3b-4739-a043-dd4685357af4"),
            "recording/b061ba14-6b3b-4739-a043-dd4685357af4?inc=artist-credits+releases+isrcs&fmt=json"
        );
    }

    #[test]
    fn a_catalog_recording_search_carries_the_page() {
        assert_eq!(
            recording_query_path("recording:\"Roygbiv\"", 25, 50),
            "recording?query=recording%3A%22Roygbiv%22&limit=25&offset=50&fmt=json"
        );
    }

    #[test]
    fn a_search_encodes_the_query_and_bounds_the_page() {
        let text = query::recording_query("Roygbiv", &["Boards of Canada".to_string()], None, None)
            .unwrap();

        assert_eq!(
            recording_search_path(&text),
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
    fn a_release_reference_lookup_asks_for_the_credits_and_the_group() {
        assert_eq!(
            release_ref_path("6360f948-17d1-4135-a580-34dc200930d8"),
            "release/6360f948-17d1-4135-a580-34dc200930d8?inc=artist-credits+release-groups+media&fmt=json"
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
    fn an_artist_browse_filters_to_official_albums_eps_and_singles() {
        assert_eq!(
            artist_releases_path("69158f97-4c07-4c4e-baf8-4e4ab1ed666e", 0),
            "release?artist=69158f97-4c07-4c4e-baf8-4e4ab1ed666e&status=official&type=album|ep|single&inc=release-groups+artist-credits+media&fmt=json&limit=100&offset=0"
        );
    }

    #[test]
    fn an_album_search_carries_the_page_and_the_release_groups() {
        assert_eq!(
            release_query_path("Geogaddi", 10, 20),
            "release?query=Geogaddi&inc=release-groups&fmt=json&limit=10&offset=20"
        );
    }

    #[test]
    fn a_barcode_search_encodes_the_barcode_and_bounds_the_page() {
        assert_eq!(
            barcode_search_path("barcode:0602547508228"),
            "release?query=barcode%3A0602547508228&limit=5&fmt=json"
        );
    }

    #[test]
    fn a_release_group_search_encodes_the_query_and_bounds_the_page() {
        assert_eq!(
            release_group_search_path("releasegroup:\"Geogaddi\" AND artist:\"Boards of Canada\""),
            "release-group?query=releasegroup%3A%22Geogaddi%22%20AND%20artist%3A%22Boards%20of%20Canada%22&limit=5&fmt=json"
        );
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
        assert_eq!(
            artist_query_path("Boards of Canada", 20, 0),
            "artist?query=Boards%20of%20Canada&limit=20&offset=0&fmt=json"
        );
    }

    #[test]
    fn the_health_probe_reads_one_genre() {
        assert_eq!(health_path(), "genre/all?fmt=json&limit=1");
    }

    #[test]
    fn the_popularity_list_is_read_under_its_own_base_url() {
        assert_eq!(
            listenbrainz_top_recordings_path("69158f97-4c07-4c4e-baf8-4e4ab1ed666e"),
            "1/popularity/top-recordings-for-artist/69158f97-4c07-4c4e-baf8-4e4ab1ed666e"
        );
        assert_eq!(
            url::endpoint(
                url::LISTENBRAINZ_BASE_URL,
                &listenbrainz_top_recordings_path("69158f97-4c07-4c4e-baf8-4e4ab1ed666e")
            ),
            "https://api.listenbrainz.org/1/popularity/top-recordings-for-artist/69158f97-4c07-4c4e-baf8-4e4ab1ed666e"
        );
    }

    #[test]
    fn a_wikidata_entity_is_read_as_json() {
        assert_eq!(
            wikidata_entity_path("Q1000000"),
            "wiki/Special:EntityData/Q1000000.json"
        );
        assert_eq!(
            url::endpoint(url::WIKIDATA_BASE_URL, &wikidata_entity_path("Q1000000")),
            "https://www.wikidata.org/wiki/Special:EntityData/Q1000000.json"
        );
    }

    #[test]
    fn a_wikipedia_summary_writes_spaces_as_underscores() {
        assert_eq!(
            wikipedia_summary_path("Boards of Canada"),
            "api/rest_v1/page/summary/Boards_of_Canada"
        );
        assert_eq!(
            url::endpoint(
                &url::wikipedia_base_url("en"),
                &wikipedia_summary_path("Boards of Canada")
            ),
            "https://en.wikipedia.org/api/rest_v1/page/summary/Boards_of_Canada"
        );
    }
}
