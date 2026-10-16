//! MusicBrainz plugin: a Hearthsound metadata and artwork provider over the
//! MusicBrainz web service and the Cover Art Archive.
//!
//! The plugin needs no account and no key, because every endpoint it uses is
//! part of MusicBrainz's public web service. It implements `lifecycle`
//! (configuration validation and a health probe), `metadata` (recording
//! resolution, release materialization and artist information) and `artwork`
//! (the Cover Art Archive renditions of an album's front cover), and nothing
//! else.
//!
//! `resolve-recording` answers the caller's identity resolution: an ISRC, when
//! the hint carries a usable one, is looked up directly, and anything else goes
//! through a Lucene search of the recording index. Either path returns scored
//! matches, best first, with the MusicBrainz recording id and any ISRC in their
//! `ids`. `get-release` materializes an album from a release id, or from the
//! earliest official release of a release group. `get-artist-info` reads an
//! artist's web links from its url relations, naming the artist by id or
//! searching the artist index for its name. `find-artwork` names the cover art
//! renditions of an album, from a release or release group id the server holds
//! or from a barcode or a title it looks up.
//!
//! Two instance settings point the plugin at its two services: `baseUrl`, which
//! defaults to the public MusicBrainz web service, and `artworkBaseUrl`, which
//! defaults to the Cover Art Archive. Either can be pointed at a mirror.
//!
//! The pure logic sits in public modules ([`url`], [`query`], [`score`],
//! [`musicbrainz`], [`map`]); the binding-coupled glue that drives it - the
//! HTTP transport, the metadata and artwork operations and the cache - stays
//! private.
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

use hearthsound_plugin::metadata_artwork_provider::exports::hearthsound::plugin::artwork::{
    ArtworkSubject, Guest as ArtworkGuest,
};
use hearthsound_plugin::metadata_artwork_provider::exports::hearthsound::plugin::lifecycle::Guest as LifecycleGuest;
use hearthsound_plugin::metadata_artwork_provider::exports::hearthsound::plugin::metadata::{
    ArtistInfo, ArtistInfoRequest, Guest as MetadataGuest, RecordingHint, RecordingMatch,
};
use hearthsound_plugin::types::{Album, IdPair, Image, ProviderError};

mod api;
mod artwork;
pub mod map;
mod metadata;
pub mod musicbrainz;
pub mod query;
pub mod score;
pub mod url;

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

/// Reads this instance's configuration and returns its MusicBrainz base URL.
///
/// # Returns
///
/// The configured base URL, normalized by [`check_url`].
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfig`] when the configuration cannot be
/// read or deserialized, and when the base URL is not an absolute `http` or
/// `https` URL.
fn base_url() -> Result<String, ProviderError> {
    let config: Config = hearthsound_plugin::config::server_config()?;

    check_url(&config.base_url, "baseUrl").map_err(ProviderError::InvalidConfig)
}

/// Reads this instance's configuration and returns its Cover Art Archive base
/// URL.
///
/// # Returns
///
/// The configured base URL, normalized by [`check_url`].
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfig`] when the configuration cannot be
/// read or deserialized, and when the base URL is not an absolute `http` or
/// `https` URL.
fn artwork_base_url() -> Result<String, ProviderError> {
    let config: Config = hearthsound_plugin::config::server_config()?;

    check_url(&config.artwork_base_url, "artworkBaseUrl").map_err(ProviderError::InvalidConfig)
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
/// configuration, a `baseUrl` that is not an absolute `http` or `https` URL,
/// and an `artworkBaseUrl` that is not one either.
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
        let base_url = base_url()?;
        metadata::health(&base_url)?;

        Ok(format!("MusicBrainz web service reachable at {base_url}"))
    }
}

/// Implements the metadata interface.
impl MetadataGuest for MusicBrainzMetadata {
    fn resolve_recording(hint: RecordingHint) -> Result<Vec<RecordingMatch>, ProviderError> {
        hearthsound_plugin::debug!(
            "resolve-recording {:?} with {} artists, album {:?}, duration {:?}, isrc {:?}",
            hint.title,
            hint.artists.len(),
            hint.album,
            hint.duration_ms,
            hint.isrc
        );

        metadata::resolve_recording(&base_url()?, &hint)
    }

    fn get_release(ids: Vec<IdPair>) -> Result<Album, ProviderError> {
        hearthsound_plugin::debug!("get-release for {} ids", ids.len());

        metadata::get_release(&base_url()?, &ids)
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
    /// The artist's `mb-artist` id and its web links, or `None` when neither
    /// the id nor the name found an artist.
    ///
    /// # Errors
    ///
    /// Returns the provider error of the request that failed.
    fn get_artist_info(request: ArtistInfoRequest) -> Result<Option<ArtistInfo>, ProviderError> {
        hearthsound_plugin::debug!(
            "get-artist-info for {:?} with {} ids",
            request.name,
            request.ids.len()
        );

        metadata::artist_info(&base_url()?, &request)
    }
}

/// Implements the artwork interface.
impl ArtworkGuest for MusicBrainzMetadata {
    /// Finds the Cover Art Archive renditions of an album's front cover.
    ///
    /// # Parameters
    ///
    /// - `subject`: the entity whose picture is wanted. An album subject is
    ///   served; any other kind is answered with an empty list.
    ///
    /// # Returns
    ///
    /// The renditions of the cover, narrowest first, and an empty list when the
    /// subject is not an album or names one the web service does not know.
    ///
    /// # Errors
    ///
    /// Returns the provider error of the request that failed.
    fn find_artwork(subject: ArtworkSubject) -> Result<Vec<Image>, ProviderError> {
        let found = artwork::find_artwork(
            &base_url()?,
            &artwork_base_url()?,
            &artwork::InstanceStore,
            &subject,
        )?;

        hearthsound_plugin::debug!(
            "find-artwork for a {:?} subject {:?} with {} ids found {} images",
            subject.kind,
            subject.title,
            subject.ids.len(),
            found.len()
        );

        Ok(found)
    }
}

hearthsound_plugin::export_metadata_artwork_provider!(MusicBrainzMetadata);

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
        assert_eq!(
            validate_config(
                r#"{"baseUrl":"https://mirror.example/ws/2","artworkBaseUrl":"https://archive.example/"}"#
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
    fn reports_every_unusable_setting_at_once() {
        let errors = validate_config(
            r#"{"baseUrl":"musicbrainz.org/ws/2","artworkBaseUrl":"coverartarchive.org"}"#,
        )
        .unwrap_err();

        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("baseUrl"), "{}", errors[0]);
        assert!(errors[1].contains("artworkBaseUrl"), "{}", errors[1]);
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
}
