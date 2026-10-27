//! MusicBrainz plugin: a Hearthsound catalog, metadata and artwork provider
//! over the MusicBrainz web service.
//!
//! The plugin needs no account and no key, because every endpoint it uses is
//! part of MusicBrainz's public web service, the Cover Art Archive, Wikidata,
//! Wikipedia or ListenBrainz. It implements `lifecycle` (configuration
//! validation and a health probe), `catalog` (search, track, album and artist
//! lookups, an artist's discography and its most played tracks), `metadata`
//! (recording resolution, release materialization and artist information with a
//! biography) and `artwork` (the Cover Art Archive renditions of an album's
//! front cover and the Wikimedia Commons portrait of an artist), and nothing
//! else.
//!
//! `search` asks the recording, release and artist indexes in turn and answers
//! the hits of the kinds the caller named; a release search is reduced to one
//! release per release group. `get-track` reads a recording and the earliest
//! official release of its release list, `get-album` reads a release with its
//! track list, and `get-artist` reads an artist with its portrait and the album
//! count an earlier discography listing counted. `artist-albums` lists the
//! official albums, EPs and singles of an artist, one reference per release
//! group, and `artist-top-tracks` reads the artist's most played recordings from
//! ListenBrainz. `lookup-isrc` resolves a code to its recordings, and
//! `lookup-album` resolves the canonical ids the server knows to the albums the
//! web service holds.
//!
//! `resolve-recording` answers the caller's identity resolution: an ISRC, when
//! the hint carries a usable one, is looked up directly, and anything else goes
//! through a Lucene search of the recording index. Either path returns scored
//! matches, best first, with the MusicBrainz recording id and any ISRC in their
//! `ids`. `get-release` materializes an album from a release id, or from the
//! earliest official release of a release group. `get-artist-info` reads an
//! artist's web links from its url relations, naming the artist by id or
//! searching the artist index for its name, and adds the first paragraph of the
//! artist's Wikipedia page as its biography. `find-artwork` names the cover art
//! renditions of an album, from a release or release group id the server holds
//! or from a barcode or a title it looks up, and the portrait of an artist from
//! the image Wikidata names for it.
//!
//! Three instance settings point the plugin at its services: `baseUrl`, which
//! defaults to the public MusicBrainz web service, `artworkBaseUrl`, which
//! defaults to the Cover Art Archive, and `wikipediaLanguage`, which picks the
//! Wikipedia edition biographies are read from. Either URL can be pointed at a
//! mirror.
//!
//! The pure logic sits in public modules ([`url`], [`query`], [`score`],
//! [`musicbrainz`], [`listenbrainz`], [`select`], [`map`]); the binding-coupled
//! glue that drives it - the HTTP transport, the catalog, metadata and artwork
//! operations and the cache - stays private.
//!
//! # Tests
//!
//! The pure logic is unit-tested on the host target:
//!
//! ```text
//! cargo test
//! ```
//!
//! Rustdoc examples in those modules are illustrative `text` blocks rather than
//! doctests: the crate is a `cdylib` for `wasm32-wasip2`, so there is no host
//! library target for a doctest to link against. Every example appears as an
//! assertion in the unit tests of the module that carries it.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Logs a debug message through the host's log import.
///
/// The import only exists in the WebAssembly build; the host target the crate
/// is unit-tested on has no logger to write to, so there the macros of this
/// crate expand to the evaluation of their arguments and nothing else. Every
/// module logs through these macros for that reason.
#[cfg(target_family = "wasm")]
macro_rules! debug {
    ($($arg:tt)*) => {
        hearthsound_plugin::debug!($($arg)*)
    };
}

/// Logs a debug message, or nothing when built for the host target.
#[cfg(not(target_family = "wasm"))]
macro_rules! debug {
    ($($arg:tt)*) => {{
        let _ = format!($($arg)*);
    }};
}

/// Logs a warning through the host's log import.
///
/// The import only exists in the WebAssembly build; see [`debug!`].
#[cfg(target_family = "wasm")]
macro_rules! warn {
    ($($arg:tt)*) => {
        hearthsound_plugin::warn!($($arg)*)
    };
}

/// Logs a warning, or nothing when built for the host target.
#[cfg(not(target_family = "wasm"))]
macro_rules! warn {
    ($($arg:tt)*) => {{
        let _ = format!($($arg)*);
    }};
}

use hearthsound_plugin::catalog_metadata_artwork_provider::exports::hearthsound::plugin::artwork::{
    ArtworkSubject, Guest as ArtworkGuest,
};
use hearthsound_plugin::catalog_metadata_artwork_provider::exports::hearthsound::plugin::catalog::{
    Guest as CatalogGuest, SearchQuery, SearchResults,
};
use hearthsound_plugin::catalog_metadata_artwork_provider::exports::hearthsound::plugin::lifecycle::Guest as LifecycleGuest;
use hearthsound_plugin::catalog_metadata_artwork_provider::exports::hearthsound::plugin::metadata::{
    ArtistInfo, ArtistInfoRequest, Guest as MetadataGuest, RecordingHint, RecordingMatch,
};
use hearthsound_plugin::types::{
    Album, AlbumRef, Artist, IdPair, Image, ProviderError, Track,
};

mod api;
mod artwork;
mod cache;
mod catalog;
pub mod listenbrainz;
pub mod map;
mod metadata;
pub mod musicbrainz;
mod paths;
pub mod query;
pub mod score;
pub mod select;
pub mod url;
mod wikimedia;

/// The plugin implementation.
struct MusicBrainzMetadata;

/// Server-scoped configuration of the plugin.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    /// Base URL of the MusicBrainz web service.
    #[serde(default = "default_base_url")]
    base_url: String,
    /// Base URL of the Cover Art Archive.
    #[serde(default = "default_artwork_base_url")]
    artwork_base_url: String,
    /// Wikipedia language biographies are read from.
    #[serde(default = "default_wikipedia_language")]
    wikipedia_language: String,
}

/// Returns the web service base URL used when the instance configuration omits
/// one.
fn default_base_url() -> String {
    url::DEFAULT_BASE_URL.to_string()
}

/// Returns the Cover Art Archive base URL used when the instance configuration
/// omits one.
fn default_artwork_base_url() -> String {
    url::DEFAULT_ARTWORK_BASE_URL.to_string()
}

/// Returns the Wikipedia language used when the instance configuration omits
/// one.
fn default_wikipedia_language() -> String {
    url::DEFAULT_WIKIPEDIA_LANGUAGE.to_string()
}

/// Checks a configured base URL and returns it normalized.
///
/// # Parameters
///
/// - `value`: the configured URL.
/// - `setting`: the configuration key, for the message.
///
/// # Returns
///
/// The URL, trimmed and with exactly one trailing slash, so the endpoint paths
/// the plugin appends land on the service root.
///
/// # Errors
///
/// Returns the message naming `setting` when the URL is not an absolute `http`
/// or `https` URL.
fn check_url(value: &str, setting: &str) -> Result<String, String> {
    let normalized = url::normalize_base_url(value);

    if url::is_http_url(&normalized) {
        Ok(normalized)
    } else {
        Err(format!(
            "{setting} must be an http or https URL, got `{value}`"
        ))
    }
}

/// Checks a configured Wikipedia language code and returns it.
///
/// # Parameters
///
/// - `value`: the configured language code.
/// - `setting`: the configuration key, for the message.
///
/// # Returns
///
/// The code, as configured.
///
/// # Errors
///
/// Returns the message naming `setting` when the code is not two or three
/// lowercase ASCII letters, which is the shape of every Wikipedia language code
/// the plugin can build a host name from.
fn check_language(value: &str, setting: &str) -> Result<String, String> {
    if (2..=3).contains(&value.len()) && value.chars().all(|c| c.is_ascii_lowercase()) {
        Ok(value.to_string())
    } else {
        Err(format!(
            "{setting} must be a Wikipedia language code of two or three lowercase letters, got `{value}`"
        ))
    }
}

/// Reads this instance's configuration.
///
/// # Returns
///
/// The configuration as the administrator stored it.
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfig`] when the configuration cannot be
/// read or deserialized.
fn config() -> Result<Config, ProviderError> {
    hearthsound_plugin::config::server_config()
}

/// Returns the MusicBrainz base URL of one configuration.
///
/// # Parameters
///
/// - `config`: the instance configuration.
///
/// # Returns
///
/// The configured base URL, normalized by [`check_url`].
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfig`] when the base URL is not an
/// absolute `http` or `https` URL.
fn base_url(config: &Config) -> Result<String, ProviderError> {
    check_url(&config.base_url, "baseUrl").map_err(ProviderError::InvalidConfig)
}

/// Returns the Cover Art Archive base URL of one configuration.
///
/// # Parameters
///
/// - `config`: the instance configuration.
///
/// # Returns
///
/// The configured base URL, normalized by [`check_url`].
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfig`] when the base URL is not an
/// absolute `http` or `https` URL.
fn artwork_base_url(config: &Config) -> Result<String, ProviderError> {
    check_url(&config.artwork_base_url, "artworkBaseUrl").map_err(ProviderError::InvalidConfig)
}

/// Returns the Wikipedia language of one configuration.
///
/// # Parameters
///
/// - `config`: the instance configuration.
///
/// # Returns
///
/// The configured language code, checked by [`check_language`].
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfig`] when the language code is not two
/// or three lowercase letters.
fn wikipedia_language(config: &Config) -> Result<String, ProviderError> {
    check_language(&config.wikipedia_language, "wikipediaLanguage")
        .map_err(ProviderError::InvalidConfig)
}

/// Validates the plugin's server-scoped configuration.
///
/// # Parameters
///
/// - `server_config`: the configuration JSON as the administrator stored it.
///
/// # Returns
///
/// `Ok(())` when the configuration is usable.
///
/// # Errors
///
/// Returns one message per problem: JSON that does not deserialize into the
/// configuration, a `baseUrl` that is not an absolute `http` or `https` URL, an
/// `artworkBaseUrl` that is not one either, and a `wikipediaLanguage` that is
/// not a language code.
fn validate_config(server_config: &str) -> Result<(), Vec<String>> {
    let config: Config = match serde_json::from_str(server_config) {
        Ok(config) => config,
        Err(error) => return Err(vec![format!("server config: {error}")]),
    };

    let mut errors = Vec::new();

    for (value, setting) in [
        (&config.base_url, "baseUrl"),
        (&config.artwork_base_url, "artworkBaseUrl"),
    ] {
        if let Err(error) = check_url(value, setting) {
            errors.push(error);
        }
    }

    if let Err(error) = check_language(&config.wikipedia_language, "wikipediaLanguage") {
        errors.push(error);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Implements the lifecycle interface.
impl LifecycleGuest for MusicBrainzMetadata {
    fn validate_config(server_config: String) -> Result<(), Vec<String>> {
        validate_config(&server_config)
    }

    fn health() -> Result<String, ProviderError> {
        let base_url = base_url(&config()?)?;
        metadata::health(&base_url)?;

        Ok(format!("MusicBrainz web service reachable at {base_url}"))
    }
}

/// Implements the catalog interface.
impl CatalogGuest for MusicBrainzMetadata {
    fn search(query: SearchQuery) -> Result<SearchResults, ProviderError> {
        debug!(
            "search {:?} with {} kinds, limit {}, offset {}",
            query.text,
            query.kinds.len(),
            query.limit,
            query.offset
        );

        catalog::search(&base_url(&config()?)?, &query)
    }

    fn get_track(id: String) -> Result<Track, ProviderError> {
        debug!("get-track for {id}");

        catalog::get_track(&base_url(&config()?)?, &id)
    }

    fn get_album(id: String) -> Result<Album, ProviderError> {
        debug!("get-album for {id}");

        catalog::get_album(&base_url(&config()?)?, &cache::InstanceStore, &id)
    }

    fn get_artist(id: String) -> Result<Artist, ProviderError> {
        debug!("get-artist for {id}");

        let config = config()?;

        catalog::get_artist(
            &base_url(&config)?,
            &cache::InstanceStore,
            &wikipedia_language(&config)?,
            &id,
        )
    }

    fn artist_albums(id: String, limit: u32, offset: u32) -> Result<Vec<AlbumRef>, ProviderError> {
        debug!("artist-albums for {id}, limit {limit}, offset {offset}");

        catalog::artist_albums(
            &base_url(&config()?)?,
            &cache::InstanceStore,
            &id,
            limit,
            offset,
        )
    }

    fn artist_top_tracks(id: String, limit: u32) -> Result<Vec<Track>, ProviderError> {
        debug!("artist-top-tracks for {id}, limit {limit}");

        catalog::artist_top_tracks(&id, limit)
    }

    fn lookup_isrc(isrc: String) -> Result<Vec<Track>, ProviderError> {
        debug!("lookup-isrc for {isrc}");

        catalog::lookup_isrc(&base_url(&config()?)?, &isrc)
    }

    fn lookup_album(ids: Vec<IdPair>) -> Result<Vec<AlbumRef>, ProviderError> {
        debug!("lookup-album for {} ids", ids.len());

        catalog::lookup_album(&base_url(&config()?)?, &cache::InstanceStore, &ids)
    }
}

/// Implements the metadata interface.
impl MetadataGuest for MusicBrainzMetadata {
    fn resolve_recording(hint: RecordingHint) -> Result<Vec<RecordingMatch>, ProviderError> {
        debug!(
            "resolve-recording {:?} with {} artists, album {:?}, duration {:?}, isrc {:?}",
            hint.title,
            hint.artists.len(),
            hint.album,
            hint.duration_ms,
            hint.isrc
        );

        metadata::resolve_recording(&base_url(&config()?)?, &hint)
    }

    fn get_release(ids: Vec<IdPair>) -> Result<Album, ProviderError> {
        debug!("get-release for {} ids", ids.len());

        metadata::get_release(&base_url(&config()?)?, &cache::InstanceStore, &ids)
    }

    /// Looks the requested artist up in the MusicBrainz artist index.
    ///
    /// # Parameters
    ///
    /// - `request`: the artist the caller wants information on, named by an
    ///   `mb-artist` id or by name.
    ///
    /// # Returns
    ///
    /// The artist's `mb-artist` id, its web links, the Wikipedia page a
    /// biography was read from and that page's first paragraph, or `None` when
    /// neither the id nor the name found an artist.
    ///
    /// # Errors
    ///
    /// Returns the provider error of the request that failed.
    fn get_artist_info(request: ArtistInfoRequest) -> Result<Option<ArtistInfo>, ProviderError> {
        debug!(
            "get-artist-info for {:?} with {} ids",
            request.name,
            request.ids.len()
        );

        let config = config()?;

        metadata::artist_info(
            &base_url(&config)?,
            &cache::InstanceStore,
            &wikipedia_language(&config)?,
            &request,
        )
    }
}

/// Implements the artwork interface.
impl ArtworkGuest for MusicBrainzMetadata {
    /// Finds the renditions of an album's front cover or of an artist's
    /// portrait.
    ///
    /// # Parameters
    ///
    /// - `subject`: the entity whose picture is wanted. An album subject is
    ///   served from the Cover Art Archive, an artist subject from Wikimedia
    ///   Commons, and any other kind is answered with an empty list.
    ///
    /// # Returns
    ///
    /// The renditions of the picture, narrowest first, and an empty list when
    /// the subject names nothing the services know.
    ///
    /// # Errors
    ///
    /// Returns the provider error of the request that failed.
    fn find_artwork(subject: ArtworkSubject) -> Result<Vec<Image>, ProviderError> {
        let config = config()?;

        let found = artwork::find_artwork(
            &base_url(&config)?,
            &artwork_base_url(&config)?,
            &cache::InstanceStore,
            &wikipedia_language(&config)?,
            &subject,
        )?;

        debug!(
            "find-artwork for a {:?} subject {:?} with {} ids found {} images",
            subject.kind,
            subject.title,
            subject.ids.len(),
            found.len()
        );

        Ok(found)
    }
}

hearthsound_plugin::export_catalog_metadata_artwork_provider!(MusicBrainzMetadata);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_valid_configuration() {
        assert_eq!(validate_config("{}"), Ok(()));
        assert_eq!(
            validate_config(r#"{"baseUrl":"https://musicbrainz.org/ws/2/"}"#),
            Ok(())
        );
        assert_eq!(
            validate_config(r#"{"baseUrl":"http://musicbrainz.internal:8080/ws/2"}"#),
            Ok(())
        );
        assert_eq!(
            validate_config(r#"{"artworkBaseUrl":"https://archive.example/cover-art"}"#),
            Ok(())
        );
        assert_eq!(validate_config(r#"{"wikipediaLanguage":"de"}"#), Ok(()));
        assert_eq!(
            validate_config(
                r#"{"baseUrl":"https://mirror.example/ws/2","artworkBaseUrl":"https://archive.example/","wikipediaLanguage":"pt"}"#
            ),
            Ok(())
        );
        assert_eq!(
            validate_config(
                r#"{"artworkBaseUrl":"https://archive.example/","wikipediaLanguage":"en"}"#
            ),
            Ok(())
        );
    }

    #[test]
    fn normalizes_the_configured_base_url() {
        assert_eq!(
            url::normalize_base_url("https://mirror.example/ws/2"),
            "https://mirror.example/ws/2/"
        );
        assert_eq!(
            url::endpoint("https://mirror.example/ws/2", "isrc/GBBPW9800010"),
            "https://mirror.example/ws/2/isrc/GBBPW9800010"
        );
    }

    #[test]
    fn rejects_a_base_url_that_is_not_http() {
        let errors = validate_config(r#"{"baseUrl":"musicbrainz.org/ws/2"}"#).unwrap_err();

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("baseUrl"), "{}", errors[0]);
    }

    #[test]
    fn rejects_an_artwork_base_url_that_is_not_http() {
        let errors = validate_config(r#"{"artworkBaseUrl":"coverartarchive.org"}"#).unwrap_err();

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("artworkBaseUrl"), "{}", errors[0]);
    }

    #[test]
    fn rejects_a_wikipedia_language_that_is_not_a_language_code() {
        for language in ["", "e", "english", "EN", "en-US", "en "] {
            let config = format!(r#"{{"wikipediaLanguage":"{language}"}}"#);
            let errors = validate_config(&config).unwrap_err();

            assert_eq!(errors.len(), 1, "{language}");
            assert!(errors[0].contains("wikipediaLanguage"), "{}", errors[0]);
        }
    }

    #[test]
    fn accepts_a_three_letter_wikipedia_language() {
        assert_eq!(check_language("gsw", "wikipediaLanguage"), Ok("gsw".into()));
        assert_eq!(check_language("en", "wikipediaLanguage"), Ok("en".into()));
        assert!(check_language(" en ", "wikipediaLanguage").is_err());
    }

    #[test]
    fn reports_every_unusable_setting_at_once() {
        let errors = validate_config(
            r#"{"baseUrl":"musicbrainz.org/ws/2","artworkBaseUrl":"coverartarchive.org","wikipediaLanguage":"English"}"#,
        )
        .unwrap_err();

        assert_eq!(errors.len(), 3);
        assert!(errors[0].contains("baseUrl"), "{}", errors[0]);
        assert!(errors[1].contains("artworkBaseUrl"), "{}", errors[1]);
        assert!(errors[2].contains("wikipediaLanguage"), "{}", errors[2]);
    }

    #[test]
    fn rejects_configuration_of_an_unexpected_shape() {
        let errors = validate_config(r#"{"baseUrl":7}"#).unwrap_err();

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("server config"), "{}", errors[0]);
    }

    #[test]
    fn the_default_base_url_is_the_public_web_service() {
        assert_eq!(default_base_url(), "https://musicbrainz.org/ws/2/");
        assert_eq!(url::DEFAULT_BASE_URL, default_base_url());
    }

    #[test]
    fn the_default_artwork_base_url_is_the_cover_art_archive() {
        assert_eq!(default_artwork_base_url(), "https://coverartarchive.org/");
        assert_eq!(url::DEFAULT_ARTWORK_BASE_URL, default_artwork_base_url());
    }

    #[test]
    fn the_manifest_allows_every_host_of_the_recorded_redirect_chains() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../manifest.json")).unwrap();
        let allowed = manifest_hosts(&manifest);

        for host in map::COVER_REDIRECT_HOSTS
            .iter()
            .chain(map::PORTRAIT_REDIRECT_HOSTS)
        {
            assert!(
                allowed.iter().any(|entry| host_entry_allows(entry, host)),
                "the manifest must allow `{host}`, it allows {allowed:?}"
            );
        }
    }

    #[test]
    fn a_manifest_host_entry_matches_the_way_the_host_matches_it() {
        // The host compares a wildcard as a suffix with at least one label in
        // front of it, a literal case-insensitively, and a `$config:` entry as
        // whatever its setting resolves to, which no recorded host can claim.
        assert!(host_entry_allows(
            "commons.wikimedia.org",
            "commons.wikimedia.org"
        ));
        assert!(host_entry_allows("archive.org", "archive.org"));
        assert!(!host_entry_allows("*.archive.org", "archive.org"));
        assert!(host_entry_allows(
            "*.archive.org",
            "dn710906.ca.archive.org"
        ));
        assert!(!host_entry_allows("*.archive.org", "example.org"));
        assert!(!host_entry_allows("archive.org", "dn710906.ca.archive.org"));
        assert!(!host_entry_allows("$config:baseUrl", "musicbrainz.org"));
        assert!(!host_entry_allows(
            "$config:artworkBaseUrl",
            "thumb.wikimedia.org"
        ));
    }

    /// Returns the host entries of a manifest.
    ///
    /// An entry is either a plain host or an object with a `host` member and a
    /// per-host rate limit.
    ///
    /// # Parameters
    ///
    /// - `manifest`: the parsed manifest.
    ///
    /// # Returns
    ///
    /// The entries, in manifest order.
    fn manifest_hosts(manifest: &serde_json::Value) -> Vec<String> {
        manifest["permissions"]["http"]["hosts"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| match entry {
                        serde_json::Value::String(host) => Some(host.clone()),
                        other => other["host"].as_str().map(str::to_string),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns whether one manifest host entry allows a host.
    ///
    /// # Parameters
    ///
    /// - `entry`: the manifest entry, as written.
    /// - `host`: the host to test.
    ///
    /// # Returns
    ///
    /// `true` when the entry names the host exactly, ignoring case, and when it
    /// is a `*.` wildcard whose domain is a suffix of the host with at least one
    /// label in front of it. A `$config:` entry allows nothing here, because
    /// what it resolves to is the administrator's setting.
    fn host_entry_allows(entry: &str, host: &str) -> bool {
        if let Some(domain) = entry.strip_prefix("*.") {
            return host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'));
        }

        if entry.starts_with("$config:") {
            return false;
        }

        entry.eq_ignore_ascii_case(host)
    }

    #[test]
    fn the_default_wikipedia_language_is_english() {
        assert_eq!(default_wikipedia_language(), "en");
        assert_eq!(
            url::DEFAULT_WIKIPEDIA_LANGUAGE,
            default_wikipedia_language()
        );
    }
}
