//! The artwork operations: one Cover Art Archive lookup per album subject.
//!
//! The plugin answers the artwork interface with the URLs of an album's front
//! cover renditions rather than with image bytes, because the server fetches
//! an image through the instance that named it. Which renditions a subject has
//! depends on what it carries: a release or a release group id names them
//! directly, and a barcode or an album title has to be looked up in the web
//! service first.
//!
//! A lookup result is cached in the instance's key-value store, because an
//! album grid asks for the same covers again and again while a search of the
//! index is the slowest request the plugin makes. The cache key is a digest of
//! the whole subject, so a result is reused for exactly the subject that
//! produced it, and a subject that names a release by id never reaches the
//! cache at all, because its renditions cost no request.

use sha2::{Digest, Sha256};

use hearthsound_plugin::metadata_artwork_provider::exports::hearthsound::plugin::artwork::ArtworkSubject;
use hearthsound_plugin::types::{EntityKind, Image, ProviderError};

use crate::{api, map, musicbrainz, query, url};

/// Prefix of every key the artwork cache writes.
const CACHE_KEY_PREFIX: &str = "artwork:v1:";

/// Separator between the parts of the text a cache key digests.
///
/// The unit separator cannot occur in a title, an artist name or an identifier,
/// so two different subjects never digest the same text.
const KEY_SEPARATOR: char = '\x1f';

/// How long a resolved lookup stays readable in the cache, in milliseconds.
///
/// The release or release group the web service answered with does not change,
/// so its key is kept for 30 days.
const FOUND_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

/// How long the absence of a lookup result stays readable, in milliseconds.
///
/// A release the index does not know yet may be indexed within days, so a key
/// that resolved to nothing is only kept for one.
const MISSING_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// Number of releases a barcode search asks for.
///
/// A barcode either names one release or none, but a reissue and its original
/// pressing can share one, and the first result is the one to draw.
const RELEASE_SEARCH_LIMIT: u32 = 5;

/// Number of release groups a release group search asks for.
const RELEASE_GROUP_SEARCH_LIMIT: u32 = 5;

/// Relevance a release group search hit must reach to belong to the subject.
///
/// The index reports `100` for a title and artist that match exactly and lower
/// values for a release group it only considers similar, so a hit below this is
/// another album with a name close to the one the subject carries.
const RELEASE_GROUP_SEARCH_SCORE: f32 = 90.0;

/// A lookup result the artwork cache stores.
///
/// The result carries the endpoint family along with the id, because a barcode
/// resolves to a release and a title resolves to a release group, and the two
/// are served under different Cover Art Archive paths.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "id")]
pub(crate) enum CachedLookup {
    /// The id of the release the lookup resolved.
    Release(String),
    /// The id of the release group the lookup resolved.
    ReleaseGroup(String),
    /// The lookup found nothing.
    Missing,
}

/// The key-value store an artwork lookup caches its results in.
///
/// The store is an interface rather than the host's key-value import so that
/// the lookups can be tested without one.
pub(crate) trait LookupCache {
    /// Reads the entry stored under `key`.
    ///
    /// # Parameters
    ///
    /// - `key`: the key to read.
    ///
    /// # Returns
    ///
    /// The entry, or `None` when the key is absent, expired, or holds a value
    /// that is not a lookup result.
    fn read(&self, key: &str) -> Option<CachedLookup>;

    /// Stores `lookup` under `key`.
    ///
    /// # Parameters
    ///
    /// - `key`: the key to write.
    /// - `lookup`: the lookup result to store.
    /// - `ttl_ms`: how long the entry stays readable, in milliseconds.
    fn write(&self, key: &str, lookup: &CachedLookup, ttl_ms: u64);
}

/// The instance's key-value store.
///
/// A cache is an optimization, so a store that refuses to read or write costs a
/// log line and another request rather than a failed call.
pub(crate) struct InstanceStore;

impl LookupCache for InstanceStore {
    fn read(&self, key: &str) -> Option<CachedLookup> {
        match hearthsound_plugin::kv::get_json(key) {
            Ok(Some(lookup)) => {
                hearthsound_plugin::debug!("artwork lookup of {key} came from the cache");

                Some(lookup)
            }
            Ok(None) => None,
            Err(error) => {
                hearthsound_plugin::warn!("artwork cache read of {key} failed: {error:?}");

                None
            }
        }
    }

    fn write(&self, key: &str, lookup: &CachedLookup, ttl_ms: u64) {
        if let Err(error) = hearthsound_plugin::kv::set_json(key, lookup, Some(ttl_ms)) {
            hearthsound_plugin::warn!("artwork cache write of {key} failed: {error:?}");
        }
    }
}

/// Finds the Cover Art Archive renditions of an artwork subject's picture.
///
/// Only an album subject is served: a release or release group id it carries
/// names the cover directly, a barcode is searched for, and a title is searched
/// for as a release group together with the album's first artist. An artist
/// subject, and every other kind, is answered with an empty list, because the
/// Cover Art Archive holds no portraits.
///
/// # Parameters
///
/// - `base_url`: base URL of the MusicBrainz web service.
/// - `artwork_base_url`: base URL of the Cover Art Archive.
/// - `cache`: the store a lookup result is cached in.
/// - `subject`: the album whose cover is wanted.
///
/// # Returns
///
/// The renditions of the cover, narrowest first, and an empty list when the
/// subject is not an album, carries nothing to look up, or names an album the
/// web service does not know. An empty list is not an error, because a provider
/// without a cover is a normal answer for the server's artwork layer.
///
/// # Errors
///
/// Returns the provider error of a request that failed. A failed request is not
/// cached, so the next call tries again.
pub(crate) fn find_artwork(
    base_url: &str,
    artwork_base_url: &str,
    cache: &dyn LookupCache,
    subject: &ArtworkSubject,
) -> Result<Vec<Image>, ProviderError> {
    if subject.kind != EntityKind::Album {
        return Ok(Vec::new());
    }

    if let Some(mbid) = id_of(subject, map::NS_RELEASE) {
        return Ok(map::release_images(artwork_base_url, mbid));
    }

    if let Some(mbid) = id_of(subject, map::NS_RELEASE_GROUP) {
        return Ok(map::release_group_images(artwork_base_url, mbid));
    }

    let key = cache_key(subject);

    if let Some(lookup) = cache.read(&key) {
        return Ok(images(artwork_base_url, &lookup));
    }

    let Some(lookup) = lookup(base_url, subject)? else {
        return Ok(Vec::new());
    };

    cache.write(&key, &lookup, ttl_ms(&lookup));

    Ok(images(artwork_base_url, &lookup))
}

/// Returns the value of a subject's identifier in one namespace.
///
/// # Parameters
///
/// - `subject`: the subject to read.
/// - `namespace`: the namespace of the identifier.
///
/// # Returns
///
/// The first value in that namespace, or `None` when the subject carries none.
fn id_of<'a>(subject: &'a ArtworkSubject, namespace: &str) -> Option<&'a str> {
    subject
        .ids
        .iter()
        .find(|pair| pair.namespace == namespace)
        .map(|pair| pair.value.as_str())
}

/// Builds the cache key of a subject.
///
/// The key is the prefix and the SHA-256 digest of the subject's kind, title,
/// artists and identifiers, in that order, each part followed by
/// [`KEY_SEPARATOR`], so a change of any part is a different key.
///
/// # Parameters
///
/// - `subject`: the subject to key.
///
/// # Returns
///
/// The key, as `artwork:v1:<64 lowercase hexadecimal digits>`.
fn cache_key(subject: &ArtworkSubject) -> String {
    let mut text = String::new();
    push_part(&mut text, kind_name(subject.kind));
    push_part(&mut text, &subject.title);

    for artist in &subject.artists {
        push_part(&mut text, artist);
    }

    for pair in &subject.ids {
        push_part(&mut text, &pair.namespace);
        push_part(&mut text, &pair.value);
    }

    format!("{CACHE_KEY_PREFIX}{}", sha256_hex(text.as_bytes()))
}

/// Appends one part and its separator to the text a cache key digests.
///
/// # Parameters
///
/// - `text`: the text to append to.
/// - `part`: the part to append.
fn push_part(text: &mut String, part: &str) {
    text.push_str(part);
    text.push(KEY_SEPARATOR);
}

/// Names an entity kind the way a cache key spells it.
///
/// # Parameters
///
/// - `kind`: the kind to name.
///
/// # Returns
///
/// The lower-case name of the kind.
fn kind_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Track => "track",
        EntityKind::Album => "album",
        EntityKind::Artist => "artist",
        EntityKind::Playlist => "playlist",
    }
}

/// Returns the SHA-256 digest of `bytes` as lowercase hexadecimal.
///
/// # Parameters
///
/// - `bytes`: the bytes to digest.
///
/// # Returns
///
/// The digest, as 64 hexadecimal digits.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Resolves the album a subject names.
///
/// A barcode the subject carries is looked up as a release; every other subject
/// is looked up as a release group by its title and its first artist.
///
/// # Parameters
///
/// - `base_url`: base URL of the MusicBrainz web service.
/// - `subject`: the subject to resolve.
///
/// # Returns
///
/// The lookup result, or `None` when the subject carries neither a barcode nor
/// a title and there is nothing to search for.
///
/// # Errors
///
/// Returns the provider error of the request that failed.
fn lookup(base_url: &str, subject: &ArtworkSubject) -> Result<Option<CachedLookup>, ProviderError> {
    if let Some(upc) = id_of(subject, map::NS_UPC) {
        return release_by_barcode(base_url, upc).map(Some);
    }

    let title = subject.title.trim();
    if title.is_empty() {
        return Ok(None);
    }

    release_group_by_name(base_url, title, &subject.artists).map(Some)
}

/// Looks a release up by its barcode.
///
/// # Parameters
///
/// - `base_url`: base URL of the MusicBrainz web service.
/// - `upc`: the barcode of the release.
///
/// # Returns
///
/// The id of the first release the index returned, or [`CachedLookup::Missing`]
/// when it returned none and when there is no barcode to search for.
///
/// # Errors
///
/// Returns the provider error of the request.
fn release_by_barcode(base_url: &str, upc: &str) -> Result<CachedLookup, ProviderError> {
    let Some(text) = query::barcode_query(upc) else {
        return Ok(CachedLookup::Missing);
    };

    let search: musicbrainz::ReleaseSearch = api::get_json(base_url, &release_search_path(&text))?;
    hearthsound_plugin::debug!(
        "barcode search matched {:?} releases, reading {}",
        search.count,
        search.releases.len()
    );

    Ok(release_lookup(&search))
}

/// Picks the lookup result of a barcode search.
///
/// # Parameters
///
/// - `search`: the response of a release search.
///
/// # Returns
///
/// The id of the first release of the page, or [`CachedLookup::Missing`] when
/// the page is empty.
fn release_lookup(search: &musicbrainz::ReleaseSearch) -> CachedLookup {
    match search.releases.first() {
        Some(release) => CachedLookup::Release(release.id.clone()),
        None => CachedLookup::Missing,
    }
}

/// Searches the release group index for an album.
///
/// # Parameters
///
/// - `base_url`: base URL of the MusicBrainz web service.
/// - `title`: the album title.
/// - `artists`: the album's artists, in order.
///
/// # Returns
///
/// The id of the most relevant release group the index scored at least
/// [`RELEASE_GROUP_SEARCH_SCORE`], or [`CachedLookup::Missing`] when no hit
/// reaches the score and when there is no title to search for.
///
/// # Errors
///
/// Returns the provider error of the request.
fn release_group_by_name(
    base_url: &str,
    title: &str,
    artists: &[String],
) -> Result<CachedLookup, ProviderError> {
    let Some(text) = query::release_group_query(title, artists) else {
        return Ok(CachedLookup::Missing);
    };

    let search: musicbrainz::ReleaseGroupSearch =
        api::get_json(base_url, &release_group_search_path(&text))?;
    hearthsound_plugin::debug!(
        "release group search matched {:?} groups, reading {}",
        search.count,
        search.release_groups.len()
    );

    Ok(release_group_lookup(&search))
}

/// Picks the lookup result of a release group search.
///
/// # Parameters
///
/// - `search`: the response of a release group search.
///
/// # Returns
///
/// The id of the first hit that reaches [`RELEASE_GROUP_SEARCH_SCORE`], or
/// [`CachedLookup::Missing`] when no hit does. Hits are read in the order the
/// index returned them, which is by relevance, so the most relevant qualifying
/// hit wins.
fn release_group_lookup(search: &musicbrainz::ReleaseGroupSearch) -> CachedLookup {
    match search
        .release_groups
        .iter()
        .find(|group| group.score.unwrap_or(0.0) >= RELEASE_GROUP_SEARCH_SCORE)
    {
        Some(group) => CachedLookup::ReleaseGroup(group.id.clone()),
        None => CachedLookup::Missing,
    }
}

/// Builds the renditions of a cached lookup result.
///
/// # Parameters
///
/// - `artwork_base_url`: base URL of the Cover Art Archive.
/// - `lookup`: the cached lookup result.
///
/// # Returns
///
/// The renditions of the resolved release or release group, and an empty list
/// for [`CachedLookup::Missing`].
fn images(artwork_base_url: &str, lookup: &CachedLookup) -> Vec<Image> {
    match lookup {
        CachedLookup::Release(mbid) => map::release_images(artwork_base_url, mbid),
        CachedLookup::ReleaseGroup(mbid) => map::release_group_images(artwork_base_url, mbid),
        CachedLookup::Missing => Vec::new(),
    }
}

/// Returns how long a lookup result stays readable in the cache.
///
/// # Parameters
///
/// - `lookup`: the lookup result to store.
///
/// # Returns
///
/// [`FOUND_TTL_MS`] for a resolved id and [`MISSING_TTL_MS`] for the marker
/// that nothing was resolved.
fn ttl_ms(lookup: &CachedLookup) -> u64 {
    match lookup {
        CachedLookup::Missing => MISSING_TTL_MS,
        CachedLookup::Release(_) | CachedLookup::ReleaseGroup(_) => FOUND_TTL_MS,
    }
}

/// Builds the path of a release search.
///
/// # Parameters
///
/// - `text`: the Lucene query, unencoded.
///
/// # Returns
///
/// The path, with the query encoded and the result page bounded by
/// [`RELEASE_SEARCH_LIMIT`].
fn release_search_path(text: &str) -> String {
    format!(
        "release?query={}&limit={}&fmt=json",
        url::encode_component(text),
        RELEASE_SEARCH_LIMIT
    )
}

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
fn release_group_search_path(text: &str) -> String {
    format!(
        "release-group?query={}&limit={}&fmt=json",
        url::encode_component(text),
        RELEASE_GROUP_SEARCH_LIMIT
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;
    use hearthsound_plugin::types::IdPair;

    /// A key-value store that keeps its entries in memory.
    struct StubStore {
        /// The entries the store holds.
        entries: RefCell<HashMap<String, CachedLookup>>,
        /// The writes the store saw, in order.
        writes: RefCell<Vec<(String, CachedLookup, u64)>>,
    }

    impl StubStore {
        /// Creates an empty store.
        fn empty() -> StubStore {
            StubStore {
                entries: RefCell::new(HashMap::new()),
                writes: RefCell::new(Vec::new()),
            }
        }

        /// Creates a store holding one entry.
        ///
        /// # Parameters
        ///
        /// - `key`: the key of the entry.
        /// - `lookup`: the entry.
        fn holding(key: &str, lookup: CachedLookup) -> StubStore {
            let store = StubStore::empty();
            store.entries.borrow_mut().insert(key.to_string(), lookup);

            store
        }
    }

    impl LookupCache for StubStore {
        fn read(&self, key: &str) -> Option<CachedLookup> {
            self.entries.borrow().get(key).cloned()
        }

        fn write(&self, key: &str, lookup: &CachedLookup, ttl_ms: u64) {
            self.entries
                .borrow_mut()
                .insert(key.to_string(), lookup.clone());
            self.writes
                .borrow_mut()
                .push((key.to_string(), lookup.clone(), ttl_ms));
        }
    }

    /// The album the tests look a cover up for.
    fn album_subject() -> ArtworkSubject {
        ArtworkSubject {
            kind: EntityKind::Album,
            provider_id: None,
            ids: vec![IdPair {
                namespace: map::NS_UPC.to_string(),
                value: "0602547508228".to_string(),
            }],
            title: "Music Has the Right to Children".to_string(),
            artists: vec!["Boards of Canada".to_string()],
        }
    }

    /// Renders images as url and width pairs, because the generated `Image` is
    /// not comparable.
    fn images_of(images: &[Image]) -> Vec<(&str, Option<u32>)> {
        images
            .iter()
            .map(|image| (image.url.as_str(), image.width))
            .collect()
    }

    #[test]
    fn a_release_id_names_the_release_renditions() {
        let mut subject = album_subject();
        subject.ids = vec![IdPair {
            namespace: map::NS_RELEASE.to_string(),
            value: "6360f948-17d1-4135-a580-34dc200930d8".to_string(),
        }];
        let store = StubStore::empty();

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            &subject,
        )
        .unwrap();

        assert_eq!(
            images_of(&found)[0].0,
            "https://coverartarchive.org/release/6360f948-17d1-4135-a580-34dc200930d8/front-250"
        );
        assert_eq!(found.len(), 3);
        assert!(store.writes.borrow().is_empty());
    }

    #[test]
    fn a_release_group_id_names_the_release_group_renditions() {
        let mut subject = album_subject();
        subject.ids = vec![IdPair {
            namespace: map::NS_RELEASE_GROUP.to_string(),
            value: "17d74d52-c92b-3b8d-9f87-218ab2d1c4a0".to_string(),
        }];
        let store = StubStore::empty();

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            &subject,
        )
        .unwrap();

        assert_eq!(
            images_of(&found)[0].0,
            "https://coverartarchive.org/release-group/17d74d52-c92b-3b8d-9f87-218ab2d1c4a0/front-250"
        );
        assert!(store.writes.borrow().is_empty());
    }

    #[test]
    fn a_cached_release_is_answered_without_a_lookup() {
        let subject = album_subject();
        let store = StubStore::holding(
            &cache_key(&subject),
            CachedLookup::Release("6360f948-17d1-4135-a580-34dc200930d8".to_string()),
        );

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            &subject,
        )
        .unwrap();

        assert_eq!(
            images_of(&found)[0].0,
            "https://coverartarchive.org/release/6360f948-17d1-4135-a580-34dc200930d8/front-250"
        );
        assert!(store.writes.borrow().is_empty());
    }

    #[test]
    fn a_cached_absence_answers_no_images() {
        let subject = album_subject();
        let store = StubStore::holding(&cache_key(&subject), CachedLookup::Missing);

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            &subject,
        )
        .unwrap();

        assert!(found.is_empty());
        assert!(store.writes.borrow().is_empty());
    }

    #[test]
    fn an_artist_subject_answers_no_images() {
        let subject = ArtworkSubject {
            kind: EntityKind::Artist,
            provider_id: None,
            ids: vec![IdPair {
                namespace: map::NS_ARTIST.to_string(),
                value: "d1f2e3a4-5b6c-4d7e-8f90-1a2b3c4d5e6f".to_string(),
            }],
            title: "Northlight".to_string(),
            artists: Vec::new(),
        };
        let store = StubStore::empty();

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            &subject,
        )
        .unwrap();

        assert!(found.is_empty());
        assert!(store.writes.borrow().is_empty());
    }

    #[test]
    fn a_subject_without_a_barcode_or_a_title_is_not_searchable() {
        let subject = ArtworkSubject {
            kind: EntityKind::Album,
            provider_id: None,
            ids: Vec::new(),
            title: "   ".to_string(),
            artists: Vec::new(),
        };

        assert!(
            lookup("https://musicbrainz.invalid/ws/2/", &subject)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_key_digests_the_whole_subject() {
        let subject = album_subject();
        let key = cache_key(&subject);

        assert_eq!(
            key,
            "artwork:v1:464cd5c504d391de0735f49d56e528ab16f5e22d80f716c84b8e663ba883316c"
        );
        assert_eq!(cache_key(&subject), key);

        let mut retitled = album_subject();
        retitled.title = "Geogaddi".to_string();
        let mut recredited = album_subject();
        recredited.artists = vec!["Autechre".to_string()];
        let mut reidentified = album_subject();
        reidentified.ids = vec![IdPair {
            namespace: map::NS_UPC.to_string(),
            value: "602547508228".to_string(),
        }];

        assert_ne!(cache_key(&retitled), key);
        assert_ne!(cache_key(&recredited), key);
        assert_ne!(cache_key(&reidentified), key);
    }

    #[test]
    fn a_resolved_lookup_is_cached_for_thirty_days() {
        assert_eq!(
            ttl_ms(&CachedLookup::Release("6360f948".to_string())),
            2_592_000_000
        );
        assert_eq!(
            ttl_ms(&CachedLookup::ReleaseGroup("17d74d52".to_string())),
            2_592_000_000
        );
        assert_eq!(ttl_ms(&CachedLookup::Missing), 86_400_000);
    }

    #[test]
    fn a_cached_lookup_round_trips_through_json() {
        for lookup in [
            CachedLookup::Release("6360f948-17d1-4135-a580-34dc200930d8".to_string()),
            CachedLookup::ReleaseGroup("17d74d52-c92b-3b8d-9f87-218ab2d1c4a0".to_string()),
            CachedLookup::Missing,
        ] {
            let json = serde_json::to_string(&lookup).unwrap();
            let back: CachedLookup = serde_json::from_str(&json).unwrap();

            assert_eq!(ttl_ms(&back), ttl_ms(&lookup));
            assert_eq!(
                images("https://coverartarchive.org", &back).len(),
                images("https://coverartarchive.org", &lookup).len()
            );
        }
    }

    #[test]
    fn a_barcode_search_reads_the_first_release() {
        let search: musicbrainz::ReleaseSearch =
            serde_json::from_str(include_str!("fixtures/release_search.json")).unwrap();

        assert!(matches!(
            release_lookup(&search),
            CachedLookup::Release(id) if id == "6360f948-17d1-4135-a580-34dc200930d8"
        ));
    }

    #[test]
    fn a_barcode_search_without_a_hit_resolves_to_nothing() {
        let search = musicbrainz::ReleaseSearch {
            count: Some(0),
            releases: Vec::new(),
        };

        assert!(matches!(release_lookup(&search), CachedLookup::Missing));
    }

    #[test]
    fn the_first_confident_release_group_wins() {
        let search: musicbrainz::ReleaseGroupSearch =
            serde_json::from_str(include_str!("fixtures/release_group_search.json")).unwrap();

        // The fixture's hits score 71, "95" and 91, so the first hit is not
        // relevant enough and the second one wins.
        assert!(matches!(
            release_group_lookup(&search),
            CachedLookup::ReleaseGroup(id) if id == "17d74d52-c92b-3b8d-9f87-218ab2d1c4a0"
        ));
    }

    #[test]
    fn a_release_group_search_without_a_confident_hit_resolves_to_nothing() {
        let search = musicbrainz::ReleaseGroupSearch {
            count: Some(1),
            release_groups: vec![musicbrainz::ReleaseGroupStub {
                id: "11111111-1111-4111-8111-111111111111".to_string(),
                title: "Geogaddi".to_string(),
                score: Some(89.0),
            }],
        };

        assert!(matches!(
            release_group_lookup(&search),
            CachedLookup::Missing
        ));
    }

    #[test]
    fn a_barcode_search_encodes_the_barcode_and_bounds_the_page() {
        assert_eq!(
            release_search_path("barcode:0602547508228"),
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
}
