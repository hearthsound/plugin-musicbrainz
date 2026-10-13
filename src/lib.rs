//! MusicBrainz metadata plugin: a Hearthsound metadata provider over the
//! MusicBrainz web service.
//!
//! The plugin needs no account and no key, because every endpoint it uses is
//! part of MusicBrainz's public web service. It implements `lifecycle`
//! (configuration validation and a health probe) and `metadata` (recording
//! resolution and release materialization), and nothing else.
//!
//! `resolve-recording` answers the caller's identity resolution: an ISRC, when
//! the hint carries a usable one, is looked up directly, and anything else goes
//! through a Lucene search of the recording index. Either path returns scored
//! matches, best first, with the MusicBrainz recording id and any ISRC in their
//! `ids`. `get-release` materializes an album from a release id, or from the
//! earliest official release of a release group.
//!
//! The only instance setting is `baseUrl`, which defaults to the public web
//! service and can be pointed at a mirror.
//!
//! The pure logic sits in public modules ([`url`], [`query`], [`score`],
//! [`musicbrainz`], [`map`]); the binding-coupled glue that drives it - the
//! HTTP transport and the metadata operations - stays private.
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

use hearthsound_plugin::metadata_provider::exports::hearthsound::plugin::lifecycle::Guest as LifecycleGuest;
use hearthsound_plugin::metadata_provider::exports::hearthsound::plugin::metadata::{
    Guest as MetadataGuest, RecordingHint, RecordingMatch,
};
use hearthsound_plugin::types::{Album, IdPair, ProviderError};

mod api;
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
}

/// Returns the base URL used when the instance configuration omits one.
fn default_base_url() -> String {
    url::DEFAULT_BASE_URL.to_string()
}

/// Reads this instance's configuration and returns its base URL.
///
/// # Returns
///
/// The configured base URL, trimmed and with exactly one trailing slash, so the
/// endpoint paths the plugin appends land on the web service root.
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfig`] when the configuration cannot be
/// read or deserialized, and when the base URL is not an absolute `http` or
/// `https` URL.
fn base_url() -> Result<String, ProviderError> {
    let config: Config = hearthsound_plugin::config::server_config()?;
    let base_url = url::normalize_base_url(&config.base_url);

    if url::is_http_url(&base_url) {
        Ok(base_url)
    } else {
        Err(ProviderError::InvalidConfig(format!(
            "baseUrl must be an http or https URL, got `{}`",
            config.base_url
        )))
    }
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
/// configuration, and a `baseUrl` that is not an absolute `http` or `https`
/// URL.
fn validate_config(server_config: &str) -> Result<(), Vec<String>> {
    let config: Config = match serde_json::from_str(server_config) {
        Ok(config) => config,
        Err(error) => return Err(vec![format!("server config: {error}")]),
    };
    let base_url = url::normalize_base_url(&config.base_url);

    if url::is_http_url(&base_url) {
        Ok(())
    } else {
        Err(vec![format!(
            "baseUrl must be an http or https URL, got `{}`",
            config.base_url
        )])
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
}

hearthsound_plugin::export_metadata_provider!(MusicBrainzMetadata);

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
}
