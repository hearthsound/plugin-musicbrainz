//! Wikidata and Wikipedia lookups: an artist's portrait and its biography.
//!
//! MusicBrainz links an artist to Wikidata through a url relation, and Wikidata
//! names the two things this plugin needs: the artist's portrait, the image of
//! property `P18` on Wikimedia Commons, and the Wikipedia page about the
//! artist, through the sitelink of a Wikipedia edition. That page's REST
//! summary carries the first paragraph, which is the biography the server
//! stores.
//!
//! Both answers are cached in the instance's key-value store: a Wikidata entity
//! grows to hundreds of kilobytes and a page summary is a request of its own,
//! while the answers change as rarely as the artist does. The cache key of an
//! entity carries the Wikipedia language, because only the sitelinks of the
//! editions the description may be read from are kept.
//!
//! The parsing and the url reading are pure and unit-tested on the host target;
//! the two operations that reach the network are thin wrappers around
//! [`crate::api`].

use std::collections::BTreeMap;

use hearthsound_plugin::types::{Image, ProviderError};

use crate::cache::{self, Cached, Store};
use crate::musicbrainz::ArtistRelation;
use crate::{api, map, paths, url};

/// Prefix of every Wikidata cache key.
const WIKIDATA_KEY_PREFIX: &str = "wikidata:v1:";

/// Prefix of every Wikipedia cache key.
const WIKIPEDIA_KEY_PREFIX: &str = "wikipedia:v1:";

/// The sitelink of one Wikipedia edition on a Wikidata entity.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Sitelink {
    /// Language code of the edition, for example `en`.
    pub language: String,
    /// Page title as the edition spells it, with spaces.
    pub title: String,
    /// Url of the page.
    pub url: String,
}

/// What the plugin keeps of a Wikidata entity.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EntitySummary {
    /// File name of the entity's image on Wikimedia Commons, from property
    /// `P18`.
    #[serde(default)]
    pub photo: Option<String>,
    /// Sitelink of the Wikipedia edition whose language was asked for.
    #[serde(default)]
    pub sitelink: Option<Sitelink>,
    /// Sitelink of the English edition, when another edition was asked for.
    #[serde(default)]
    pub fallback: Option<Sitelink>,
}

/// What the plugin keeps of a Wikipedia page.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PageSummary {
    /// First paragraph of the page, when the edition has one.
    #[serde(default)]
    pub extract: Option<String>,
}

/// The biography read for an artist.
#[derive(Debug, Clone)]
pub struct Biography {
    /// First paragraph of the page, when the edition has one.
    pub extract: Option<String>,
    /// Url of the page it was read from.
    pub url: String,
}

/// The response of a Wikidata entity data request.
#[derive(Debug, Clone, serde::Deserialize)]
struct EntityData {
    /// The requested entity, keyed by its item id.
    #[serde(default)]
    entities: BTreeMap<String, Entity>,
}

/// One Wikidata entity.
#[derive(Debug, Clone, serde::Deserialize)]
struct Entity {
    /// The entity's statements, keyed by property id.
    #[serde(default)]
    claims: BTreeMap<String, Vec<Statement>>,
    /// The entity's sitelinks, keyed by wiki, for example `enwiki`.
    #[serde(default)]
    sitelinks: BTreeMap<String, RawSitelink>,
}

/// One statement of a Wikidata entity.
#[derive(Debug, Clone, serde::Deserialize)]
struct Statement {
    /// Rank of the statement, which marks a deprecated value.
    #[serde(default)]
    rank: Option<String>,
    /// The statement's main value.
    #[serde(default)]
    mainsnak: Option<Snak>,
}

/// The value of one statement.
#[derive(Debug, Clone, serde::Deserialize)]
struct Snak {
    /// The value the statement carries.
    #[serde(default)]
    datavalue: Option<DataValue>,
}

/// A value of one datatype.
#[derive(Debug, Clone, serde::Deserialize)]
struct DataValue {
    /// The value itself, a string for a Commons file name.
    #[serde(default)]
    value: Option<serde_json::Value>,
}

/// A sitelink as Wikidata serializes it.
#[derive(Debug, Clone, serde::Deserialize)]
struct RawSitelink {
    /// Page title as the edition spells it.
    title: String,
    /// Url of the page, which Wikidata reports for every sitelink.
    #[serde(default)]
    url: Option<String>,
}

/// The response of a Wikipedia page summary request.
#[derive(Debug, Clone, serde::Deserialize)]
struct PageResponse {
    /// First paragraph of the page, in plain text.
    #[serde(default)]
    extract: Option<String>,
}

/// Returns the Wikidata id an artist's url relations name.
///
/// # Parameters
///
/// - `relations`: the relations of an artist lookup.
///
/// # Returns
///
/// The item id of the `wikidata` relation, or `None` when the artist has no
/// Wikidata relation or its url names no item.
///
/// # Example
///
/// ```text
/// wikidata_id(&relations) -> Some("Q1000000")
/// ```
pub fn wikidata_id(relations: &[ArtistRelation]) -> Option<String> {
    let relation = relations
        .iter()
        .find(|relation| relation.kind == "wikidata")?;
    let resource = relation.url.as_ref()?.resource.as_str();

    item_id(resource)
}

/// Reads the item id out of a Wikidata url.
///
/// # Parameters
///
/// - `resource`: the url of the relation, for example
///   `https://www.wikidata.org/wiki/Q1000000`.
///
/// # Returns
///
/// The last path segment when it is shaped like an item id, and `None`
/// otherwise.
fn item_id(resource: &str) -> Option<String> {
    let segment = resource
        .trim_end_matches('/')
        .rsplit('/')
        .next()?
        .split(['?', '#'])
        .next()?;
    let digits = segment.strip_prefix('Q')?;

    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("Q{digits}"))
    } else {
        None
    }
}

/// Returns the Wikipedia page an artist's url relations name.
///
/// # Parameters
///
/// - `relations`: the relations of an artist lookup.
///
/// # Returns
///
/// The language, the page title and the page url of the `wikipedia` relation,
/// or `None` when the artist has no Wikipedia relation or its url is not a
/// Wikipedia page.
///
/// # Example
///
/// ```text
/// wikipedia_page(&relations)
///   -> Some(("en".to_string(), "Northlight".to_string(), "https://en.wikipedia.org/wiki/Northlight".to_string()))
/// ```
pub fn wikipedia_page(relations: &[ArtistRelation]) -> Option<(String, String, String)> {
    let relation = relations
        .iter()
        .find(|relation| relation.kind == "wikipedia")?;
    let resource = relation.url.as_ref()?.resource.as_str();
    let (language, title) = wikipedia_url(resource)?;

    Some((language, title, resource.to_string()))
}

/// Reads the edition and the page title out of a Wikipedia url.
///
/// # Parameters
///
/// - `resource`: the url of the relation, for example
///   `https://en.wikipedia.org/wiki/Northlight`.
///
/// # Returns
///
/// The edition's language and the page title with spaces, or `None` when the
/// url is not a page of a Wikipedia edition.
fn wikipedia_url(resource: &str) -> Option<(String, String)> {
    let rest = resource
        .strip_prefix("https://")
        .or_else(|| resource.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let language = host.strip_suffix(".wikipedia.org").filter(|language| {
        !language.is_empty()
            && language
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })?;
    let encoded = path.strip_prefix("wiki/")?;
    let encoded = encoded.split(['?', '#']).next().unwrap_or(encoded);
    let title = url::percent_decode(encoded).replace('_', " ");

    if title.is_empty() {
        return None;
    }

    Some((language.to_string(), title))
}

/// Reads the Wikidata entity an artist's relations name.
///
/// # Parameters
///
/// - `store`: the cache to read and write.
/// - `language`: the Wikipedia language whose sitelink is kept.
/// - `qid`: the Wikidata item id.
///
/// # Returns
///
/// What the entity says, or `None` when Wikidata does not know the item. The
/// answer is cached for [`cache::FOUND_TTL_MS`], and a missing item for
/// [`cache::MISSING_TTL_MS`].
///
/// # Errors
///
/// Returns the provider error of a request that failed for a reason other than
/// a missing item.
pub fn entity(
    store: &dyn Store,
    language: &str,
    qid: &str,
) -> Result<Option<EntitySummary>, ProviderError> {
    let key = entity_key(language, qid);

    if let Some(cached) = cache::get::<EntitySummary>(store, &key) {
        return Ok(cached.value());
    }

    let endpoint = url::endpoint(url::WIKIDATA_BASE_URL, &paths::wikidata_entity_path(qid));
    let data: EntityData = match api::get_json_at(&endpoint) {
        Ok(data) => data,
        Err(ProviderError::NotFound) => {
            cache::put(store, &key, &Cached::<EntitySummary>::Missing);

            return Ok(None);
        }
        Err(error) => return Err(error),
    };

    let summary = entity_summary(&data, language, qid);

    let Some(summary) = summary else {
        debug!("wikidata answered without the entity {qid}");
        cache::put(store, &key, &Cached::<EntitySummary>::Missing);

        return Ok(None);
    };

    cache::put(store, &key, &Cached::Found(summary.clone()));

    Ok(Some(summary))
}

/// Reads the summary of one Wikipedia page.
///
/// # Parameters
///
/// - `store`: the cache to read and write.
/// - `language`: the edition's language code.
/// - `title`: the page title, with spaces.
///
/// # Returns
///
/// The page's summary, or `None` when the edition has no such page. The answer
/// is cached for [`cache::FOUND_TTL_MS`], and a missing page for
/// [`cache::MISSING_TTL_MS`].
///
/// # Errors
///
/// Returns the provider error of a request that failed for a reason other than
/// a missing page.
pub fn page_summary(
    store: &dyn Store,
    language: &str,
    title: &str,
) -> Result<Option<PageSummary>, ProviderError> {
    let key = page_key(language, title);

    if let Some(cached) = cache::get::<PageSummary>(store, &key) {
        return Ok(cached.value());
    }

    let endpoint = url::endpoint(
        &url::wikipedia_base_url(language),
        &paths::wikipedia_summary_path(title),
    );

    match api::get_json_at::<PageResponse>(&endpoint) {
        Ok(response) => {
            let summary = PageSummary {
                extract: map::non_empty(response.extract.as_deref()),
            };
            cache::put(store, &key, &Cached::Found(summary.clone()));

            Ok(Some(summary))
        }
        Err(ProviderError::NotFound) => {
            cache::put(store, &key, &Cached::<PageSummary>::Missing);

            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Finds the portrait of an artist.
///
/// # Parameters
///
/// - `store`: the cache to read and write.
/// - `language`: the Wikipedia language whose sitelink is cached.
/// - `relations`: the relations of the artist's web service lookup.
///
/// # Returns
///
/// The Wikimedia Commons rendition of the artist's portrait, or `None` when the
/// artist has no Wikidata relation or Wikidata names no image for it.
///
/// # Errors
///
/// Returns the provider error of a request that failed.
pub fn portrait(
    store: &dyn Store,
    language: &str,
    relations: &[ArtistRelation],
) -> Result<Option<Image>, ProviderError> {
    let Some(qid) = wikidata_id(relations) else {
        return Ok(None);
    };
    let Some(entity) = entity(store, language, &qid)? else {
        return Ok(None);
    };

    Ok(entity.photo.map(|file| map::commons_portrait(&file)))
}

/// Reads the biography of an artist.
///
/// The page is the sitelink of the requested Wikipedia language, then the one
/// of the English edition, then the `wikipedia` url relation of the artist. The
/// biography is the first paragraph of that page.
///
/// # Parameters
///
/// - `store`: the cache to read and write.
/// - `language`: the Wikipedia language to read the biography from.
/// - `relations`: the relations of the artist's web service lookup.
///
/// # Returns
///
/// The biography and the page it was read from, or `None` when the artist has
/// no page in reach and when the page has no summary.
///
/// # Errors
///
/// Returns the provider error of a request that failed.
pub fn biography(
    store: &dyn Store,
    language: &str,
    relations: &[ArtistRelation],
) -> Result<Option<Biography>, ProviderError> {
    let mut page = None;

    if let Some(qid) = wikidata_id(relations)
        && let Some(entity) = entity(store, language, &qid)?
    {
        page = entity
            .sitelink
            .or(entity.fallback)
            .map(|link| (link.language, link.title, link.url));
    }

    if page.is_none() {
        page = wikipedia_page(relations);
    }

    let Some((language, title, url)) = page else {
        return Ok(None);
    };
    let Some(summary) = page_summary(store, &language, &title)? else {
        debug!("no Wikipedia page for {title} in {language}");

        return Ok(None);
    };

    Ok(Some(Biography {
        extract: summary.extract,
        url,
    }))
}

/// Builds what the plugin keeps of one Wikidata response.
///
/// # Parameters
///
/// - `data`: the response of a Wikidata entity data request.
/// - `language`: the Wikipedia language whose sitelink is kept.
/// - `qid`: the item id that was requested.
///
/// # Returns
///
/// The summary, or `None` when the response does not hold the requested item.
fn entity_summary(data: &EntityData, language: &str, qid: &str) -> Option<EntitySummary> {
    let entity = data.entities.get(qid)?;

    Some(EntitySummary {
        photo: photo(&entity.claims),
        sitelink: sitelink(&entity.sitelinks, &model_key(language), language),
        fallback: sitelink(&entity.sitelinks, "enwiki", "en").filter(|_| language != "en"),
    })
}

/// Returns the Commons file name a `P18` claim names.
///
/// # Parameters
///
/// - `claims`: the statements of the entity, keyed by property id.
///
/// # Returns
///
/// The file name of the first statement that is not deprecated and carries a
/// non-empty string.
fn photo(claims: &BTreeMap<String, Vec<Statement>>) -> Option<String> {
    claims
        .get("P18")?
        .iter()
        .filter(|statement| statement.rank.as_deref() != Some("deprecated"))
        .filter_map(|statement| {
            statement
                .mainsnak
                .as_ref()?
                .datavalue
                .as_ref()?
                .value
                .as_ref()?
                .as_str()
        })
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

/// Returns the sitelink of one wiki.
///
/// # Parameters
///
/// - `sitelinks`: the entity's sitelinks, keyed by wiki.
/// - `key`: the wiki, for example `enwiki`.
/// - `language`: the language code of that wiki.
///
/// # Returns
///
/// The sitelink, or `None` when the entity has none for that wiki or its title
/// is empty. A sitelink without a url gets the one of its page title.
fn sitelink(
    sitelinks: &BTreeMap<String, RawSitelink>,
    key: &str,
    language: &str,
) -> Option<Sitelink> {
    let link = sitelinks.get(key)?;
    let title = link.title.trim();

    if title.is_empty() {
        return None;
    }

    let url = match link.url.as_deref() {
        Some(url) if !url.is_empty() => url.to_string(),
        _ => url::wikipedia_page_url(language, title),
    };

    Some(Sitelink {
        language: language.to_string(),
        title: title.to_string(),
        url,
    })
}

/// Returns the wiki key of one Wikipedia edition.
///
/// # Parameters
///
/// - `language`: the edition's language code.
///
/// # Returns
///
/// The key the edition's sitelinks are stored under.
fn model_key(language: &str) -> String {
    format!("{language}wiki")
}

/// Builds the cache key of a Wikidata entity.
///
/// # Parameters
///
/// - `language`: the Wikipedia language whose sitelink is kept.
/// - `qid`: the Wikidata item id.
///
/// # Returns
///
/// The key, as `wikidata:v1:<qid>:<language>`.
fn entity_key(language: &str, qid: &str) -> String {
    format!("{WIKIDATA_KEY_PREFIX}{qid}:{language}")
}

/// Builds the cache key of a Wikipedia page.
///
/// # Parameters
///
/// - `language`: the edition's language code.
/// - `title`: the page title.
///
/// # Returns
///
/// The key, as `wikipedia:v1:<language>:<title>`.
fn page_key(language: &str, title: &str) -> String {
    format!("{WIKIPEDIA_KEY_PREFIX}{language}:{title}")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    /// A store that keeps its entries in memory.
    struct MemoryStore {
        /// The entries the store holds.
        entries: RefCell<HashMap<String, String>>,
    }

    impl MemoryStore {
        /// Creates an empty store.
        fn empty() -> MemoryStore {
            MemoryStore {
                entries: RefCell::new(HashMap::new()),
            }
        }
    }

    impl Store for MemoryStore {
        fn read(&self, key: &str) -> Option<String> {
            self.entries.borrow().get(key).cloned()
        }

        fn write(&self, key: &str, value: &str, _ttl_ms: u64) {
            self.entries
                .borrow_mut()
                .insert(key.to_string(), value.to_string());
        }
    }

    fn artist_lookup() -> crate::musicbrainz::ArtistLookup {
        serde_json::from_str(include_str!("fixtures/artist_lookup.json")).unwrap()
    }

    fn relations() -> Vec<ArtistRelation> {
        artist_lookup().relations
    }

    fn entity_data() -> EntityData {
        serde_json::from_str(include_str!("fixtures/wikidata_entity.json")).unwrap()
    }

    fn page_response() -> PageResponse {
        serde_json::from_str(include_str!("fixtures/wikipedia_summary.json")).unwrap()
    }

    /// Seeds a store with the cached answers of the fixtures.
    fn seeded_store() -> MemoryStore {
        let store = MemoryStore::empty();
        let entity = entity_summary(&entity_data(), "en", "Q1000000").unwrap();
        let page = PageSummary {
            extract: map::non_empty(page_response().extract.as_deref()),
        };

        crate::cache::put(
            &store,
            &entity_key("en", "Q1000000"),
            &Cached::Found(entity),
        );
        crate::cache::put(&store, &page_key("en", "Northlight"), &Cached::Found(page));

        store
    }

    #[test]
    fn a_wikidata_entity_names_its_portrait_and_its_sitelinks() {
        let summary = entity_summary(&entity_data(), "en", "Q1000000").unwrap();

        assert_eq!(summary.photo.as_deref(), Some("Northlight live 1999.jpg"));
        let sitelink = summary.sitelink.as_ref().unwrap();
        assert_eq!(sitelink.language, "en");
        assert_eq!(sitelink.title, "Northlight");
        assert_eq!(sitelink.url, "https://en.wikipedia.org/wiki/Northlight");
        assert!(summary.fallback.is_none());
    }

    #[test]
    fn another_language_keeps_the_english_sitelink_as_the_fallback() {
        let summary = entity_summary(&entity_data(), "de", "Q1000000").unwrap();

        assert_eq!(
            summary.sitelink.map(|link| link.title),
            Some("Northlight (Band)".to_string())
        );
        assert_eq!(
            summary.fallback.map(|link| link.title),
            Some("Northlight".to_string())
        );
    }

    #[test]
    fn an_entity_without_a_portrait_or_a_sitelink_keeps_nothing() {
        let data: EntityData =
            serde_json::from_str(r#"{"entities":{"Q1":{"claims":{}}}}"#).unwrap();
        let summary = entity_summary(&data, "en", "Q1").unwrap();

        assert_eq!(summary.photo, None);
        assert!(summary.sitelink.is_none());
        assert!(summary.fallback.is_none());
        assert!(entity_summary(&data, "en", "Q2").is_none());
    }

    #[test]
    fn a_deprecated_image_claim_is_ignored() {
        let data: EntityData = serde_json::from_str(
            r#"{"entities":{"Q1":{"claims":{"P18":[
                {"rank":"deprecated","mainsnak":{"datavalue":{"value":"Old.jpg","type":"string"}}},
                {"rank":"preferred","mainsnak":{"datavalue":{"value":"New.jpg","type":"string"}}}
            ]}}}}"#,
        )
        .unwrap();
        let summary = entity_summary(&data, "en", "Q1").unwrap();

        assert_eq!(summary.photo.as_deref(), Some("New.jpg"));
    }

    #[test]
    fn a_cached_entity_yields_the_portrait_of_the_artist() {
        let store = seeded_store();
        let image = portrait(&store, "en", &relations()).unwrap().unwrap();

        assert_eq!(
            image.url,
            "https://commons.wikimedia.org/wiki/Special:FilePath/Northlight%20live%201999.jpg?width=1200"
        );
        assert_eq!(image.width, Some(1200));
    }

    #[test]
    fn a_cached_entity_and_page_yield_the_biography_and_its_page() {
        let store = seeded_store();
        let biography = biography(&store, "en", &relations()).unwrap().unwrap();

        assert!(!biography.extract.unwrap().is_empty());
        assert_eq!(biography.url, "https://en.wikipedia.org/wiki/Northlight");
    }

    #[test]
    fn artist_information_from_the_fixtures_carries_a_biography_and_a_wikipedia_link() {
        let store = seeded_store();
        let artist = artist_lookup();
        let read = biography(&store, "en", &artist.relations).unwrap().unwrap();

        let mut links = map::artist_links(&artist.relations);
        map::push_link(&mut links, "wikipedia", &read.url);
        let info = map::artist_info(&artist, read.extract, links);

        assert!(
            info.biography
                .as_deref()
                .is_some_and(|text| text.contains("Scottish electronic music duo")),
            "the Wikipedia fixture's first paragraph is the biography"
        );
        assert_eq!(
            info.links
                .iter()
                .filter(|link| link.kind == "wikipedia")
                .count(),
            1,
            "the page the biography came from is linked once"
        );
    }

    #[test]
    fn an_artist_without_a_wikidata_relation_has_no_biography_or_portrait() {
        let store = MemoryStore::empty();

        assert!(portrait(&store, "en", &[]).unwrap().is_none());
        assert!(biography(&store, "en", &[]).unwrap().is_none());
    }

    #[test]
    fn a_cached_missing_entity_answers_nothing() {
        let store = MemoryStore::empty();
        crate::cache::put(
            &store,
            &entity_key("en", "Q1000000"),
            &Cached::<EntitySummary>::Missing,
        );
        crate::cache::put(
            &store,
            &page_key("en", "Northlight"),
            &Cached::<PageSummary>::Missing,
        );

        assert!(portrait(&store, "en", &relations()).unwrap().is_none());
        assert!(biography(&store, "en", &relations()).unwrap().is_none());
    }

    #[test]
    fn the_cache_keys_carry_the_language_and_the_title() {
        assert_eq!(entity_key("en", "Q1000000"), "wikidata:v1:Q1000000:en");
        assert_eq!(
            page_key("de", "Northlight (Band)"),
            "wikipedia:v1:de:Northlight (Band)"
        );
        assert_eq!(model_key("en"), "enwiki");
    }

    #[test]
    fn the_wikidata_relation_names_the_item() {
        assert_eq!(wikidata_id(&relations()).as_deref(), Some("Q1000000"));
        assert_eq!(wikidata_id(&[]), None);
    }

    #[test]
    fn a_url_that_is_not_an_item_names_nothing() {
        let relation = |resource: &str| {
            vec![ArtistRelation {
                kind: "wikidata".to_string(),
                url: Some(crate::musicbrainz::RelationUrl {
                    resource: resource.to_string(),
                }),
            }]
        };

        assert_eq!(
            wikidata_id(&relation("https://www.wikidata.org/wiki/Q42")).as_deref(),
            Some("Q42")
        );
        assert_eq!(
            wikidata_id(&relation("https://www.wikidata.org/wiki/Q42/")).as_deref(),
            Some("Q42")
        );
        assert_eq!(
            wikidata_id(&relation("https://www.wikidata.org/wiki/Q42?uselang=en")).as_deref(),
            Some("Q42")
        );
        assert_eq!(
            wikidata_id(&relation("https://www.wikidata.org/wiki/Item")),
            None
        );
        assert_eq!(
            wikidata_id(&relation("https://www.wikidata.org/wiki/Q")),
            None
        );
        assert_eq!(wikidata_id(&relation("")), None);
    }

    #[test]
    fn the_wikipedia_relation_names_the_page() {
        let page = wikipedia_page(&relations()).unwrap();

        assert_eq!(page.0, "en");
        assert_eq!(page.1, "Northlight");
        assert_eq!(page.2, "https://en.wikipedia.org/wiki/Northlight");
    }

    #[test]
    fn a_wikipedia_url_is_read_into_an_edition_and_a_title() {
        assert_eq!(
            wikipedia_url("https://en.wikipedia.org/wiki/Boards_of_Canada"),
            Some(("en".to_string(), "Boards of Canada".to_string()))
        );
        assert_eq!(
            wikipedia_url("http://de.wikipedia.org/wiki/Sigur_R%C3%B3s"),
            Some(("de".to_string(), "Sigur R\u{f3}s".to_string()))
        );
        assert_eq!(
            wikipedia_url("https://en.wikipedia.org/wiki/Boards_of_Canada?oldid=7"),
            Some(("en".to_string(), "Boards of Canada".to_string()))
        );
        assert_eq!(wikipedia_url("https://example.org/wiki/Northlight"), None);
        assert_eq!(wikipedia_url("https://en.wikipedia.org/wiki/"), None);
        assert_eq!(wikipedia_url("https://en.wikipedia.org/"), None);
        assert_eq!(wikipedia_url("https://EN.wikipedia.org/wiki/X"), None);
        assert_eq!(wikipedia_url(""), None);
    }

    #[test]
    fn a_portrait_is_read_from_the_image_claim() {
        let image = map::commons_portrait("Tenney from northeast.jpg");

        assert_eq!(
            image.url,
            "https://commons.wikimedia.org/wiki/Special:FilePath/Tenney%20from%20northeast.jpg?width=1200"
        );
        assert_eq!(image.width, Some(1200));
        assert_eq!(image.height, None);
    }
}
