//! The artwork operations: cover art for albums, portraits for artists.
//!
//! The plugin answers the artwork interface with the URLs of an image's
//! renditions rather than with image bytes, because the server fetches an image
//! through the instance that named it.
//!
//! An album's cover comes from the Cover Art Archive. Which renditions it has
//! depends on what the subject carries: a release or a release group id names
//! them directly, and a barcode or an album title has to be looked up in the web
//! service first. A lookup result is cached in the instance's key-value store,
//! because an album grid asks for the same covers again and again while a
//! search of the index is the slowest request the plugin makes.
//!
//! An artist's portrait comes from Wikimedia Commons: MusicBrainz links the
//! artist to Wikidata, Wikidata names the portrait with property `P18`, and
//! Commons serves that file at any width. The Wikidata answer is cached, so a
//! page that draws a portrait per album artist costs one request per artist.
//!
//! The cache key of an album subject is a digest of the whole subject, so a
//! result is reused for exactly the subject that produced it; a subject that
//! names a release or a release group by id never reaches the cache at all,
//! because its renditions cost no request.

use sha2::{Digest, Sha256};

use hearthsound_plugin::catalog_metadata_artwork_provider::exports::hearthsound::plugin::artwork::ArtworkSubject;
use hearthsound_plugin::types::{EntityKind, Image, ProviderError};

use crate::cache::{self, Cached, Store};
use crate::{api, map, musicbrainz, paths, query, wikimedia};

/// Prefix of every key the artwork cache writes.
const CACHE_KEY_PREFIX: &str = "artwork:v1:";

/// Separator between the parts of the text a cache key digests.
///
/// The unit separator cannot occur in a title, an artist name or an identifier,
/// so two different subjects never digest the same text.
const KEY_SEPARATOR: char = '\x1f';

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
pub enum Lookup {
    /// The id of the release the lookup resolved.
    Release(String),
    /// The id of the release group the lookup resolved.
    ReleaseGroup(String),
}

/// Finds the renditions of an artwork subject's picture.
///
/// An album subject is served from the Cover Art Archive: a release or release
/// group id it carries names the cover directly, a barcode is searched for, and
/// a title is searched for as a release group together with the album's first
/// artist. An artist subject is served from Wikimedia Commons through Wikidata,
/// named by the `mb-artist` id it carries or by the instance's own id for the
/// artist. Any other kind is answered with an empty list.
///
/// # Parameters
///
/// - `base_url`: base URL of the MusicBrainz web service.
/// - `artwork_base_url`: base URL of the Cover Art Archive.
/// - `store`: the cache a lookup result is stored in.
/// - `language`: the Wikipedia language whose sitelink is cached beside an
///   artist's portrait.
/// - `subject`: the entity whose picture is wanted.
///
/// # Returns
///
/// The renditions of the picture, narrowest first, and an empty list when the
/// subject carries nothing to look up, names an entity the services do not
/// know, or is neither an album nor an artist. An empty list is not an error,
/// because a provider without a picture is a normal answer for the server's
/// artwork layer.
///
/// # Errors
///
/// Returns the provider error of a request that failed. A failed lookup is not
/// cached, so the next call tries again.
pub fn find_artwork(
    base_url: &str,
    artwork_base_url: &str,
    store: &dyn Store,
    language: &str,
    subject: &ArtworkSubject,
) -> Result<Vec<Image>, ProviderError> {
    match subject.kind {
        EntityKind::Album => album_artwork(artwork_base_url, store, base_url, subject),
        EntityKind::Artist => artist_artwork(base_url, store, language, subject),
        EntityKind::Track | EntityKind::Playlist => Ok(Vec::new()),
    }
}

/// Finds the Cover Art Archive renditions of an album's front cover.
///
/// # Parameters
///
/// - `artwork_base_url`: base URL of the Cover Art Archive.
/// - `store`: the cache a lookup result is stored in.
/// - `base_url`: base URL of the MusicBrainz web service.
/// - `subject`: the album whose cover is wanted.
///
/// # Returns
///
/// The renditions of the cover, narrowest first, and an empty list when the
/// subject carries nothing to look up or names an album the web service does
/// not know.
///
/// # Errors
///
/// Returns the provider error of a request that failed.
fn album_artwork(
    artwork_base_url: &str,
    store: &dyn Store,
    base_url: &str,
    subject: &ArtworkSubject,
) -> Result<Vec<Image>, ProviderError> {
    if let Some(mbid) = id_of(subject, map::NS_RELEASE) {
        return Ok(map::release_images(artwork_base_url, mbid));
    }

    if let Some(mbid) = id_of(subject, map::NS_RELEASE_GROUP) {
        return Ok(map::release_group_images(artwork_base_url, mbid));
    }

    let key = cache_key(subject);

    if let Some(cached) = cache::get::<Lookup>(store, &key) {
        return Ok(images(artwork_base_url, cached.value().as_ref()));
    }

    let Some(answer) = lookup(base_url, subject)? else {
        return Ok(Vec::new());
    };

    cache::put(store, &key, &answer);

    Ok(images(artwork_base_url, answer.value().as_ref()))
}

/// Finds the portrait of an artist.
///
/// # Parameters
///
/// - `base_url`: base URL of the MusicBrainz web service.
/// - `store`: the cache Wikidata answers are read from.
/// - `language`: the Wikipedia language whose sitelink is cached.
/// - `subject`: the artist whose portrait is wanted.
///
/// # Returns
///
/// The Wikimedia Commons rendition of the portrait, and an empty list when the
/// subject names no artist the web service knows, names an artist without a
/// Wikidata item, or names one whose item has no image.
///
/// # Errors
///
/// Returns the provider error of a request that failed.
fn artist_artwork(
    base_url: &str,
    store: &dyn Store,
    language: &str,
    subject: &ArtworkSubject,
) -> Result<Vec<Image>, ProviderError> {
    let Some(mbid) = id_of(subject, map::NS_ARTIST).or(subject.provider_id.as_deref()) else {
        return Ok(Vec::new());
    };

    let artist: musicbrainz::ArtistLookup = match api::get_json(base_url, &paths::artist_path(mbid))
    {
        Ok(artist) => artist,
        Err(ProviderError::NotFound) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    Ok(wikimedia::portrait(store, language, &artist.relations)?
        .into_iter()
        .collect())
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
/// The answer to cache, with [`Cached::Missing`] for a lookup that found
/// nothing, or `None` when the subject carries neither a barcode nor a title
/// and there is nothing to search for.
///
/// # Errors
///
/// Returns the provider error of the request that failed.
fn lookup(
    base_url: &str,
    subject: &ArtworkSubject,
) -> Result<Option<Cached<Lookup>>, ProviderError> {
    if let Some(upc) = id_of(subject, map::NS_UPC) {
        return release_by_barcode(base_url, upc);
    }

    let title = subject.title.trim();
    if title.is_empty() {
        return Ok(None);
    }

    release_group_by_name(base_url, title, &subject.artists)
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
/// The id of the first release the index returned, [`Cached::Missing`] when it
/// returned none, and `None` when there is no barcode to search for.
///
/// # Errors
///
/// Returns the provider error of the request.
fn release_by_barcode(base_url: &str, upc: &str) -> Result<Option<Cached<Lookup>>, ProviderError> {
    let Some(text) = query::barcode_query(upc) else {
        return Ok(None);
    };

    let search: musicbrainz::ReleaseSearch =
        api::get_json(base_url, &paths::barcode_search_path(&text))?;
    debug!(
        "barcode search matched {:?} releases, reading {}",
        search.count,
        search.releases.len()
    );

    Ok(Some(selected_release(&search)))
}

/// Picks the lookup result of a barcode search.
///
/// # Parameters
///
/// - `search`: the response of a release search.
///
/// # Returns
///
/// The first release the index returned, or [`Cached::Missing`] when it
/// returned none. A barcode a reissue shares with its original pressing names
/// both, and the first hit is the one the index ranks highest.
fn selected_release(search: &musicbrainz::ReleaseSearch) -> Cached<Lookup> {
    match search.releases.first() {
        Some(release) => Cached::Found(Lookup::Release(release.id.clone())),
        None => Cached::Missing,
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
/// The most relevant release group the index scored at least
/// [`RELEASE_GROUP_SEARCH_SCORE`], [`Cached::Missing`] when no hit reaches that
/// score, and `None` when there is no title to search for.
///
/// # Errors
///
/// Returns the provider error of the request.
fn release_group_by_name(
    base_url: &str,
    title: &str,
    artists: &[String],
) -> Result<Option<Cached<Lookup>>, ProviderError> {
    let Some(text) = query::release_group_query(title, artists) else {
        return Ok(None);
    };

    let search: musicbrainz::ReleaseGroupSearch =
        api::get_json(base_url, &paths::release_group_search_path(&text))?;
    debug!(
        "release group search matched {:?} groups, reading {}",
        search.count,
        search.release_groups.len()
    );

    Ok(Some(selected_group(&search)))
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
/// [`Cached::Missing`] when no hit does. Hits are read in the order the index
/// returned them, which is by relevance, so the most relevant qualifying hit
/// wins.
fn selected_group(search: &musicbrainz::ReleaseGroupSearch) -> Cached<Lookup> {
    match search
        .release_groups
        .iter()
        .find(|group| group.score.unwrap_or(0.0) >= RELEASE_GROUP_SEARCH_SCORE)
    {
        Some(group) => Cached::Found(Lookup::ReleaseGroup(group.id.clone())),
        None => Cached::Missing,
    }
}

/// Builds the renditions of a resolved lookup.
///
/// # Parameters
///
/// - `artwork_base_url`: base URL of the Cover Art Archive.
/// - `lookup`: the lookup result, when one was resolved.
///
/// # Returns
///
/// The renditions of the resolved release or release group, and an empty list
/// for an absent lookup.
fn images(artwork_base_url: &str, lookup: Option<&Lookup>) -> Vec<Image> {
    match lookup {
        Some(Lookup::Release(mbid)) => map::release_images(artwork_base_url, mbid),
        Some(Lookup::ReleaseGroup(mbid)) => map::release_group_images(artwork_base_url, mbid),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;
    use hearthsound_plugin::types::IdPair;

    /// A store that keeps its entries in memory.
    struct MemoryStore {
        /// The entries the store holds.
        entries: RefCell<HashMap<String, String>>,
        /// The writes the store saw, in order.
        writes: RefCell<Vec<(String, String, u64)>>,
    }

    impl MemoryStore {
        /// Creates an empty store.
        fn empty() -> MemoryStore {
            MemoryStore {
                entries: RefCell::new(HashMap::new()),
                writes: RefCell::new(Vec::new()),
            }
        }

        /// Creates a store holding one cached lookup.
        ///
        /// # Parameters
        ///
        /// - `key`: the key of the entry.
        /// - `answer`: the entry.
        fn holding(key: &str, answer: &Cached<Lookup>) -> MemoryStore {
            let store = MemoryStore::empty();
            cache::put(&store, key, answer);

            store
        }
    }

    impl Store for MemoryStore {
        fn read(&self, key: &str) -> Option<String> {
            self.entries.borrow().get(key).cloned()
        }

        fn write(&self, key: &str, value: &str, ttl_ms: u64) {
            self.entries
                .borrow_mut()
                .insert(key.to_string(), value.to_string());
            self.writes
                .borrow_mut()
                .push((key.to_string(), value.to_string(), ttl_ms));
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
        let store = MemoryStore::empty();

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            "en",
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
        let store = MemoryStore::empty();

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            "en",
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
        let store = MemoryStore::holding(
            &cache_key(&subject),
            &Cached::Found(Lookup::Release(
                "6360f948-17d1-4135-a580-34dc200930d8".to_string(),
            )),
        );

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            "en",
            &subject,
        )
        .unwrap();

        assert_eq!(
            images_of(&found)[0].0,
            "https://coverartarchive.org/release/6360f948-17d1-4135-a580-34dc200930d8/front-250"
        );
        assert_eq!(store.writes.borrow().len(), 1);
    }

    #[test]
    fn a_cached_absence_answers_no_images() {
        let subject = album_subject();
        let store = MemoryStore::holding(&cache_key(&subject), &Cached::<Lookup>::Missing);

        let found = find_artwork(
            "https://musicbrainz.invalid/ws/2/",
            "https://coverartarchive.org",
            &store,
            "en",
            &subject,
        )
        .unwrap();

        assert!(found.is_empty());
        assert_eq!(store.writes.borrow()[0].2, cache::MISSING_TTL_MS);
    }

    #[test]
    fn a_track_or_playlist_subject_answers_no_images() {
        let store = MemoryStore::empty();

        for kind in [EntityKind::Track, EntityKind::Playlist] {
            let subject = ArtworkSubject {
                kind,
                provider_id: Some("abc".to_string()),
                ids: Vec::new(),
                title: "Roygbiv".to_string(),
                artists: Vec::new(),
            };

            assert!(
                find_artwork(
                    "https://musicbrainz.invalid/ws/2/",
                    "https://coverartarchive.org",
                    &store,
                    "en",
                    &subject,
                )
                .unwrap()
                .is_empty()
            );
        }
    }

    #[test]
    fn an_artist_without_an_id_answers_no_images() {
        let subject = ArtworkSubject {
            kind: EntityKind::Artist,
            provider_id: None,
            ids: Vec::new(),
            title: "Northlight".to_string(),
            artists: Vec::new(),
        };
        let store = MemoryStore::empty();

        assert!(
            find_artwork(
                "https://musicbrainz.invalid/ws/2/",
                "https://coverartarchive.org",
                &store,
                "en",
                &subject,
            )
            .unwrap()
            .is_empty()
        );
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
    fn a_cached_lookup_round_trips_through_the_store() {
        for lookup in [
            Lookup::Release("6360f948-17d1-4135-a580-34dc200930d8".to_string()),
            Lookup::ReleaseGroup("17d74d52-c92b-3b8d-9f87-218ab2d1c4a0".to_string()),
        ] {
            let store = MemoryStore::empty();
            cache::put(&store, "k", &Cached::Found(lookup.clone()));
            let read = cache::get::<Lookup>(&store, "k").unwrap().value();

            assert_eq!(
                images("https://coverartarchive.org", read.as_ref()).len(),
                images("https://coverartarchive.org", Some(&lookup)).len()
            );
        }
    }

    #[test]
    fn a_barcode_search_reads_the_first_release() {
        let search: musicbrainz::ReleaseSearch =
            serde_json::from_str(include_str!("fixtures/release_search.json")).unwrap();

        match selected_release(&search).value() {
            Some(Lookup::Release(id)) => {
                assert_eq!(id, "6360f948-17d1-4135-a580-34dc200930d8");
            }
            _ => panic!("expected the fixture to hold a release"),
        }
    }

    #[test]
    fn a_barcode_search_without_a_hit_resolves_to_nothing() {
        let search = musicbrainz::ReleaseSearch {
            count: Some(0),
            releases: Vec::new(),
        };

        assert!(selected_release(&search).value().is_none());
    }

    #[test]
    fn the_first_confident_release_group_wins() {
        let search: musicbrainz::ReleaseGroupSearch =
            serde_json::from_str(include_str!("fixtures/release_group_search.json")).unwrap();

        // The fixture's hits score 71, "95" and 91, so the first hit is not
        // relevant enough and the second one wins.
        match selected_group(&search).value() {
            Some(Lookup::ReleaseGroup(id)) => {
                assert_eq!(id, "17d74d52-c92b-3b8d-9f87-218ab2d1c4a0");
            }
            _ => panic!("expected a release group"),
        }
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

        assert!(selected_group(&search).value().is_none());
    }
}
