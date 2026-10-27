//! The catalog operations: search, lookups, an artist's discography and its
//! most played tracks.
//!
//! Every function composes a path with [`crate::paths`], calls [`crate::api`]
//! and translates the payload with [`crate::map`]. The choice of what to ask
//! for is documented per function; a listing is reduced with
//! [`crate::select`], so the release a reference names is the same one every
//! layer reports.
//!
//! The ids the records carry are raw MusicBrainz ids, because the server
//! namespaces a provider's ids per instance and would otherwise namespace a
//! namespaced value. The canonical identifiers - `mb-release`,
//! `mb-release-group`, `mb-recording`, `mb-artist`, `isrc`, `upc` - travel in
//! the records' `ids`.

use std::collections::BTreeSet;

use hearthsound_plugin::catalog_metadata_artwork_provider::exports::hearthsound::plugin::catalog::{
    SearchQuery, SearchResults,
};
use hearthsound_plugin::types::{
    Album, AlbumRef, Artist, EntityKind, IdPair, ProviderError, Track,
};

use crate::cache::{self, Store};
use crate::{api, listenbrainz, map, metadata, musicbrainz, paths, query, select, url, wikimedia};

/// Number of browse pages a discography reads at most.
///
/// One page holds 100 releases, so ten pages hold more releases than any artist
/// has albums, EPs and singles together. The cap keeps a request for a huge
/// discography bounded; past it the answer is truncated rather than failed.
const DISCOGRAPHY_PAGES: u32 = 10;

/// Prefix of the cache key holding an artist's album count.
const ALBUM_COUNT_KEY_PREFIX: &str = "artist-albums:v1:";

/// Searches the MusicBrainz indexes.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `request`: the search text, the kinds to answer, and the page of each
///   kind.
///
/// # Returns
///
/// The hits of every requested kind, in the order the index ranked them. An
/// empty `kinds` list asks for every kind, and a zero limit asks for nothing;
/// neither sends a request. Search text that is empty or only whitespace has
/// nothing to match and answers no hits.
///
/// # Errors
///
/// Returns the provider error of the request that failed.
pub fn search(base_url: &str, request: &SearchQuery) -> Result<SearchResults, ProviderError> {
    let mut results = SearchResults {
        tracks: Vec::new(),
        albums: Vec::new(),
        artists: Vec::new(),
    };

    if request.limit == 0 {
        return Ok(results);
    }

    let Some(text) = query::search_text(&request.text) else {
        debug!("an empty search text matches nothing");

        return Ok(results);
    };

    if wants(&request.kinds, EntityKind::Track) {
        results.tracks = search_tracks(base_url, &text, request.limit, request.offset)?;
    }

    if wants(&request.kinds, EntityKind::Album) {
        results.albums = search_albums(base_url, &text, request.limit, request.offset)?;
    }

    if wants(&request.kinds, EntityKind::Artist) {
        results.artists = search_artists(base_url, &text, request.limit, request.offset)?;
    }

    Ok(results)
}

/// Returns whether a search asked for one kind.
///
/// # Parameters
///
/// - `kinds`: the kinds the caller asked for.
/// - `wanted`: the kind to test.
///
/// # Returns
///
/// `true` when the caller asked for that kind, and when it asked for no kind at
/// all, which means every kind.
fn wants(kinds: &[EntityKind], wanted: EntityKind) -> bool {
    kinds.is_empty() || kinds.contains(&wanted)
}

/// Searches the recording index.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `text`: the encoded Lucene query.
/// - `limit`: number of tracks to answer.
/// - `offset`: number of tracks to skip.
///
/// # Returns
///
/// One track per hit, in the order the index ranked them, each pointing at the
/// first official release the hit listed.
///
/// # Errors
///
/// Returns the provider error of the request.
fn search_tracks(
    base_url: &str,
    text: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Track>, ProviderError> {
    let search: musicbrainz::RecordingSearch =
        api::get_json(base_url, &paths::recording_query_path(text, limit, offset))?;
    debug!(
        "recording search matched {:?} recordings, reading {}",
        search.count,
        search.recordings.len()
    );

    Ok(search
        .recordings
        .iter()
        .map(|recording| {
            map::recording_track(
                recording,
                select::first_official(&recording.releases),
                &recording.isrcs,
            )
        })
        .collect())
}

/// Searches the release index.
///
/// The index answers with every pressing of an album, so the hits are reduced
/// to one release per release group.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `text`: the encoded Lucene query.
/// - `limit`: number of albums to answer.
/// - `offset`: number of releases to skip.
///
/// # Returns
///
/// One reference per release group, in the order the index ranked the groups.
///
/// # Errors
///
/// Returns the provider error of the request.
fn search_albums(
    base_url: &str,
    text: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<AlbumRef>, ProviderError> {
    let search: musicbrainz::ReleaseSearch =
        api::get_json(base_url, &paths::release_query_path(text, limit, offset))?;
    debug!(
        "release search matched {:?} releases, reading {}",
        search.count,
        search.releases.len()
    );

    Ok(grouped_references(&search.releases))
}

/// Searches the artist index.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `text`: the encoded Lucene query.
/// - `limit`: number of artists to answer.
/// - `offset`: number of artists to skip.
///
/// # Returns
///
/// One artist per hit, in the order the index ranked them, without images:
/// naming a portrait costs a Wikidata request per artist, which a search does
/// not spend.
///
/// # Errors
///
/// Returns the provider error of the request.
fn search_artists(
    base_url: &str,
    text: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Artist>, ProviderError> {
    let search: musicbrainz::ArtistSearch =
        api::get_json(base_url, &paths::artist_query_path(text, limit, offset))?;
    debug!(
        "artist search matched {:?} artists, reading {}",
        search.count,
        search.artists.len()
    );

    Ok(search
        .artists
        .iter()
        .map(|artist| Artist {
            id: artist.id.clone(),
            name: artist.name.clone(),
            images: Vec::new(),
            ids: vec![map::id_pair(map::NS_ARTIST, &artist.id)],
            album_count: None,
        })
        .collect())
}

/// Reads one track.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `id`: MusicBrainz recording id.
///
/// # Returns
///
/// The track, pointing at the earliest official release of its release list.
///
/// # Errors
///
/// Returns [`ProviderError::NotFound`] when the web service does not know the
/// recording, and the provider error of the request otherwise.
pub fn get_track(base_url: &str, id: &str) -> Result<Track, ProviderError> {
    let recording: musicbrainz::Recording =
        api::get_json(base_url, &paths::recording_credit_path(id))?;
    debug!(
        "read recording {} with {} releases",
        recording.id,
        recording.releases.len()
    );

    Ok(map::recording_track(
        &recording,
        select::earliest_official(&recording.releases),
        &recording.isrcs,
    ))
}

/// Reads one album with its track list.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `store`: the cache a release body is read from and stored in.
/// - `id`: MusicBrainz release id.
///
/// # Returns
///
/// The album, with its tracks, its genres and its edition.
///
/// # Errors
///
/// Returns [`ProviderError::NotFound`] when the web service does not know the
/// release, and the provider error of the request otherwise.
pub fn get_album(base_url: &str, store: &dyn Store, id: &str) -> Result<Album, ProviderError> {
    let release: musicbrainz::Release =
        api::get_json_cached(store, base_url, &paths::release_path(id))?;
    debug!(
        "read release {} with {} media",
        release.id,
        release.media.len()
    );

    Ok(map::album(&release))
}

/// Reads one artist.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `store`: the cache holding the album count a discography listing stored.
/// - `language`: the Wikipedia language whose sitelink is cached beside the
///   portrait.
/// - `id`: MusicBrainz artist id.
///
/// # Returns
///
/// The artist with the portrait Wikidata names for it, when it names one, and
/// the album count an earlier `artist-albums` call counted. The count is not
/// read from the web service here: counting would list the whole discography,
/// which the caller asks for separately.
///
/// # Errors
///
/// Returns [`ProviderError::NotFound`] when the web service does not know the
/// artist, and the provider error of the request otherwise.
pub fn get_artist(
    base_url: &str,
    store: &dyn Store,
    language: &str,
    id: &str,
) -> Result<Artist, ProviderError> {
    let artist: musicbrainz::ArtistLookup =
        api::get_json_cached(store, base_url, &paths::artist_path(id))?;
    debug!(
        "read artist {} with {} relations",
        artist.id,
        artist.relations.len()
    );

    let images = wikimedia::portrait(store, language, &artist.relations)?
        .into_iter()
        .collect();
    let album_count =
        cache::get::<u32>(store, &album_count_key(&artist.id)).and_then(|c| c.value());

    Ok(map::artist(&artist, images, album_count))
}

/// Lists the albums, EPs and singles of an artist.
///
/// The web service can only page its releases, not its release groups, so the
/// pages are read until the requested slice is covered, the listing is
/// exhausted, or [`DISCOGRAPHY_PAGES`] pages were read. Only official releases
/// are listed, and each release group is represented by its earliest release.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `store`: the cache an exhausted listing stores its album count in.
/// - `id`: MusicBrainz artist id.
/// - `limit`: number of references to answer.
/// - `offset`: number of references to skip.
///
/// # Returns
///
/// One reference per release group, in the order the web service listed the
/// groups, sliced to the requested page. A zero limit answers nothing without a
/// request.
///
/// # Errors
///
/// Returns the provider error of the page that failed.
pub fn artist_albums(
    base_url: &str,
    store: &dyn Store,
    id: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<AlbumRef>, ProviderError> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let wanted = usize::try_from(offset.saturating_add(limit)).unwrap_or(usize::MAX);
    let mut releases: Vec<musicbrainz::ReleaseStub> = Vec::new();
    let mut page = 0;
    let mut exhausted = false;

    while !exhausted
        && page < DISCOGRAPHY_PAGES
        && select::official_by_release_group(&releases).len() < wanted
    {
        let browse: musicbrainz::ReleaseBrowse = api::get_json(
            base_url,
            &paths::artist_releases_path(id, page * paths::BROWSE_LIMIT),
        )?;
        page += 1;

        let returned = u32::try_from(browse.releases.len()).unwrap_or(u32::MAX);
        let offset_so_far = (page - 1) * paths::BROWSE_LIMIT;
        exhausted = returned == 0
            || returned < paths::BROWSE_LIMIT
            || browse
                .release_count
                .is_some_and(|count| offset_so_far + returned >= count);

        releases.extend(browse.releases);
    }

    let refs = grouped_references(&releases);

    if exhausted {
        let count = u32::try_from(refs.len()).unwrap_or(u32::MAX);
        cache::put(store, &album_count_key(id), &cache::Cached::Found(count));
    }

    let answered = refs
        .into_iter()
        .skip(usize::try_from(offset).unwrap_or(usize::MAX))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect();

    Ok(answered)
}

/// Reads the most played recordings of an artist.
///
/// ListenBrainz answers its popularity endpoint for anyone who asks, so the
/// plugin needs no account and no token: the list is the one ListenBrainz counts
/// for its listeners, whoever they are.
///
/// # Parameters
///
/// - `id`: MusicBrainz artist id.
/// - `limit`: number of tracks to answer.
///
/// # Returns
///
/// The tracks ListenBrainz ranks highest for the artist, in its order, sliced to
/// `limit`. A zero limit answers nothing without a request.
///
/// # Errors
///
/// Returns the provider error of the request. An entry that names another
/// artist beside the one asked about is dropped rather than misattributed; see
/// [`map::popularity_track`].
pub fn artist_top_tracks(id: &str, limit: u32) -> Result<Vec<Track>, ProviderError> {
    if limit == 0 {
        debug!("no popularity list: no room to answer one");

        return Ok(Vec::new());
    }

    let endpoint = url::endpoint(
        url::LISTENBRAINZ_BASE_URL,
        &paths::listenbrainz_top_recordings_path(id),
    );
    let entries: Vec<listenbrainz::RecordingPopularity> = api::send_json(api::request(&endpoint))?;
    debug!("popularity list held {} recordings", entries.len());

    Ok(entries
        .iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .filter_map(|entry| map::popularity_track(entry, id))
        .collect())
}

/// Resolves one ISRC to its tracks.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `isrc`: the ISRC as the caller supplied it.
///
/// # Returns
///
/// The tracks the code is assigned to, each pointing at the first official
/// release the lookup listed, and an empty list when the code is not shaped
/// like an ISRC or the web service does not know it.
///
/// # Errors
///
/// Returns the provider error of the request.
pub fn lookup_isrc(base_url: &str, isrc: &str) -> Result<Vec<Track>, ProviderError> {
    let isrc = url::normalize_isrc(isrc);

    if !url::is_valid_isrc(&isrc) {
        debug!("`{isrc}` is not an ISRC");

        return Ok(Vec::new());
    }

    let lookup: musicbrainz::IsrcLookup = match api::get_json(base_url, &paths::isrc_path(&isrc)) {
        Ok(lookup) => lookup,
        Err(ProviderError::NotFound) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    debug!(
        "ISRC {isrc} is assigned to {} recordings",
        lookup.recordings.len()
    );

    Ok(lookup
        .recordings
        .iter()
        .map(|recording| {
            let mut isrcs = recording.isrcs.clone();
            if !isrcs.iter().any(|value| value.eq_ignore_ascii_case(&isrc)) {
                isrcs.insert(0, isrc.clone());
            }

            map::recording_track(
                recording,
                select::first_official(&recording.releases),
                &isrcs,
            )
        })
        .collect())
}

/// Looks the albums up that the web service holds for the given canonical ids.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `store`: the cache release bodies are read from and stored in.
/// - `ids`: the canonical ids to resolve. `mb-release` names a release
///   directly, `mb-release-group` resolves to the earliest official release of
///   the group, and `upc` searches the release index for the barcode. An id in
///   a namespace this plugin does not know is skipped.
///
/// # Returns
///
/// One reference per album found, in the order of `ids`, without duplicates,
/// and an empty list when none of them matches.
///
/// # Errors
///
/// Returns the provider error of a request that failed for a reason other than
/// a missing album; a missing album is skipped, because the caller asked about
/// several ids at once.
pub fn lookup_album(
    base_url: &str,
    store: &dyn Store,
    ids: &[IdPair],
) -> Result<Vec<AlbumRef>, ProviderError> {
    let mut refs: Vec<AlbumRef> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for pair in ids {
        let reference = match album_for(base_url, store, pair)? {
            Some(reference) => reference,
            None => continue,
        };

        if seen.insert(reference.id.clone()) {
            refs.push(reference);
        }
    }

    debug!("lookup-album answered {} albums", refs.len());

    Ok(refs)
}

/// Resolves one canonical id to an album reference.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `store`: the cache release bodies are read from and stored in.
/// - `pair`: the canonical id to resolve.
///
/// # Returns
///
/// The reference, or `None` when the id is in another namespace, names
/// something the web service does not know, or carries an empty value.
///
/// # Errors
///
/// Returns the provider error of a request that failed for a reason other than
/// a missing album.
fn album_for(
    base_url: &str,
    store: &dyn Store,
    pair: &IdPair,
) -> Result<Option<AlbumRef>, ProviderError> {
    let value = pair.value.trim();

    if value.is_empty() {
        return Ok(None);
    }

    match pair.namespace.as_str() {
        map::NS_RELEASE => reference_of_release(base_url, store, value),
        map::NS_RELEASE_GROUP => {
            let release = match metadata::earliest_official_release(base_url, value) {
                Ok(release) => release,
                Err(ProviderError::NotFound) => return Ok(None),
                Err(error) => return Err(error),
            };

            reference_of_release(base_url, store, &release)
        }
        map::NS_UPC => by_barcode(base_url, value),
        _ => Ok(None),
    }
}

/// Reads the album reference of one release.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `store`: the cache the release body is read from and stored in.
/// - `mbid`: MusicBrainz release id.
///
/// # Returns
///
/// The reference, or `None` when the web service does not know the release.
///
/// # Errors
///
/// Returns the provider error of the request.
fn reference_of_release(
    base_url: &str,
    store: &dyn Store,
    mbid: &str,
) -> Result<Option<AlbumRef>, ProviderError> {
    match api::get_json_cached::<musicbrainz::Release>(store, base_url, &paths::release_path(mbid))
    {
        Ok(release) => Ok(Some(map::release_ref(&release))),
        Err(ProviderError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Searches the release index for the release that carries a barcode.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service.
/// - `upc`: the barcode of the release.
///
/// # Returns
///
/// The reference of the first release the index returned, or `None` when the
/// barcode is empty or the index returned none.
///
/// # Errors
///
/// Returns the provider error of the request.
fn by_barcode(base_url: &str, upc: &str) -> Result<Option<AlbumRef>, ProviderError> {
    let Some(text) = query::barcode_query(upc) else {
        return Ok(None);
    };

    let search: musicbrainz::ReleaseSearch =
        api::get_json(base_url, &paths::barcode_search_path(&text))?;

    Ok(barcode_reference(&search))
}

/// Picks the album reference of a barcode search.
///
/// # Parameters
///
/// - `search`: the response of a release search.
///
/// # Returns
///
/// The reference of the first release the index returned, or `None` when it
/// returned none. A reissue and its original pressing can share a barcode, and
/// the first hit is the one the index ranks highest.
fn barcode_reference(search: &musicbrainz::ReleaseSearch) -> Option<AlbumRef> {
    search.releases.first().map(map::release_ref_from_stub)
}

/// Reduces a release listing to one reference per release group.
///
/// # Parameters
///
/// - `releases`: the release listing entries read so far, in the order the web
///   service listed them.
///
/// # Returns
///
/// One reference per release group, represented by the group's earliest
/// official release, in first-appearance order. A release search answer and a
/// discography listing both carry every pressing of an album, and both report
/// the album once.
fn grouped_references(releases: &[musicbrainz::ReleaseStub]) -> Vec<AlbumRef> {
    select::official_by_release_group(releases)
        .into_iter()
        .map(map::release_ref_from_stub)
        .collect()
}

/// Builds the cache key holding an artist's album count.
///
/// # Parameters
///
/// - `id`: MusicBrainz artist id.
///
/// # Returns
///
/// The key, as `artist-albums:v1:<mbid>`.
fn album_count_key(id: &str) -> String {
    format!("{ALBUM_COUNT_KEY_PREFIX}{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a search query of one page.
    fn query_of(text: &str, kinds: Vec<EntityKind>, limit: u32, offset: u32) -> SearchQuery {
        SearchQuery {
            text: text.to_string(),
            kinds,
            limit,
            offset,
        }
    }

    fn pair(namespace: &str, value: &str) -> IdPair {
        IdPair {
            namespace: namespace.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn a_search_without_a_kind_or_a_page_takes_no_request() {
        let base_url = "https://musicbrainz.invalid/ws/2/";

        let nothing = search(base_url, &query_of("Northlight", Vec::new(), 0, 0)).unwrap();
        assert!(nothing.tracks.is_empty());
        assert!(nothing.albums.is_empty());
        assert!(nothing.artists.is_empty());

        let blank = search(base_url, &query_of("   ", Vec::new(), 10, 0)).unwrap();
        assert!(blank.tracks.is_empty());
        assert!(blank.albums.is_empty());
        assert!(blank.artists.is_empty());
    }

    #[test]
    fn an_empty_kind_list_asks_for_every_kind() {
        assert!(wants(&[], EntityKind::Track));
        assert!(wants(&[], EntityKind::Album));
        assert!(wants(&[], EntityKind::Artist));
        assert!(wants(&[EntityKind::Album], EntityKind::Album));
        assert!(!wants(&[EntityKind::Album], EntityKind::Track));
        assert!(!wants(&[EntityKind::Album], EntityKind::Artist));
    }

    #[test]
    fn a_zero_discography_limit_takes_no_request() {
        let store = crate::cache::InstanceStore;

        assert!(
            artist_albums(
                "https://musicbrainz.invalid/ws/2/",
                &store,
                "69158f97-4c07-4c4e-baf8-4e4ab1ed666e",
                0,
                0
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            artist_top_tracks("69158f97-4c07-4c4e-baf8-4e4ab1ed666e", 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_popularity_request_needs_no_credentials() {
        let entries: Vec<listenbrainz::RecordingPopularity> =
            serde_json::from_str(include_str!("fixtures/artist_popularity.json")).unwrap();
        assert_eq!(entries.len(), 2, "the fixture holds the popularity list");

        let endpoint = url::endpoint(
            url::LISTENBRAINZ_BASE_URL,
            &paths::listenbrainz_top_recordings_path("69158f97-4c07-4c4e-baf8-4e4ab1ed666e"),
        );
        let request = api::request(&endpoint);

        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            "https://api.listenbrainz.org/1/popularity/top-recordings-for-artist/69158f97-4c07-4c4e-baf8-4e4ab1ed666e"
        );
        assert!(
            request.headers.is_empty(),
            "the endpoint is public, so no header is sent"
        );
        assert_eq!(request.body, None);
        assert!(request.timeout_ms.is_some(), "the budget is set");
    }

    #[test]
    fn an_unusable_isrc_is_not_looked_up() {
        let base_url = "https://musicbrainz.invalid/ws/2/";

        // A hyphenated code is normalized into a usable one; a code that is
        // too short or empty is not looked up at all.
        assert!(lookup_isrc(base_url, "").unwrap().is_empty());
        assert!(lookup_isrc(base_url, "GBBPW980001").unwrap().is_empty());
        assert!(lookup_isrc(base_url, "   ").unwrap().is_empty());
    }

    #[test]
    fn an_id_in_another_namespace_resolves_to_nothing() {
        let base_url = "https://musicbrainz.invalid/ws/2/";

        let store = crate::cache::InstanceStore;

        assert!(
            album_for(base_url, &store, &pair(map::NS_ISRC, "GBBPW9800010"))
                .unwrap()
                .is_none()
        );
        assert!(
            album_for(base_url, &store, &pair(map::NS_RELEASE, "  "))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_discography_lists_one_reference_per_release_group() {
        let browse: musicbrainz::ReleaseBrowse =
            serde_json::from_str(include_str!("fixtures/artist_releases.json")).unwrap();
        let refs = grouped_references(&browse.releases);

        assert_eq!(refs.len(), 2, "two groups are listed, five releases");

        let first = &refs[0];
        assert_eq!(first.id, "3fdcf49c-e297-4c21-880d-dba4c40d98dd");
        assert_eq!(first.title, "Music Has the Right to Children");
        assert_eq!(first.album_type.as_deref(), Some("album"));
        assert_eq!(first.year, Some(1998));
        assert_eq!(first.track_count, Some(17));
        assert!(
            first
                .ids
                .iter()
                .any(|pair| pair.namespace == map::NS_RELEASE_GROUP
                    && pair.value == "17d74d52-c92b-3b8d-9f87-218ab2d1c4a0")
        );

        let second = &refs[1];
        assert_eq!(second.id, "5da8e677-bec2-471f-82ec-5b51133cd0c7");
        assert_eq!(second.title, "Geogaddi");
        assert_eq!(second.track_count, Some(24));
        assert!(
            second
                .ids
                .iter()
                .any(|pair| pair.namespace == map::NS_RELEASE_GROUP
                    && pair.value == "8183834a-6c8a-39ef-a7bc-a82f79b929e9")
        );
    }

    #[test]
    fn a_release_search_is_reduced_to_one_reference_per_group() {
        let search: musicbrainz::ReleaseSearch =
            serde_json::from_str(include_str!("fixtures/album_search.json")).unwrap();
        let refs = grouped_references(&search.releases);

        // The fixture holds two pressings of one group and one EP of another.
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].id, "3f4c1a2b-5d6e-4f70-8192-a3b4c5d6e7f8");
        assert_eq!(refs[0].album_type.as_deref(), Some("album"));
        assert_eq!(refs[0].year, Some(2001));
        assert_eq!(refs[1].id, "7c8d9e0f-1a2b-4c3d-8e4f-5a6b7c8d9e0f");
        assert_eq!(refs[1].album_type.as_deref(), Some("ep"));
        assert_eq!(refs[1].track_count, Some(4));
    }

    #[test]
    fn a_barcode_search_yields_the_reference_of_its_first_hit() {
        let search: musicbrainz::ReleaseSearch =
            serde_json::from_str(include_str!("fixtures/barcode_release_search.json")).unwrap();
        let reference = barcode_reference(&search).unwrap();

        assert_eq!(reference.id, "6360f948-17d1-4135-a580-34dc200930d8");
        assert_eq!(reference.title, "Music Has the Right to Children");
        assert!(
            reference
                .ids
                .iter()
                .any(|pair| pair.namespace == map::NS_UPC && pair.value == "744861029922")
        );
        assert!(
            barcode_reference(&musicbrainz::ReleaseSearch {
                count: Some(0),
                releases: Vec::new(),
            })
            .is_none()
        );
    }

    #[test]
    fn the_album_count_key_is_namespaced() {
        assert_eq!(
            album_count_key("69158f97-4c07-4c4e-baf8-4e4ab1ed666e"),
            "artist-albums:v1:69158f97-4c07-4c4e-baf8-4e4ab1ed666e"
        );
    }
}
