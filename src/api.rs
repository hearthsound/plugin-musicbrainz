//! HTTP access to the MusicBrainz web service through the `host-http` host
//! import.
//!
//! This is the only module that touches the network. It builds one request,
//! sends it, and turns the response into either a decoded payload or a provider
//! error; endpoint composition lives in [`crate::paths`] and the translation
//! into provider records in [`crate::map`].
//!
//! The plugin sets no `User-Agent` header, so the host adds the meaningful one
//! MusicBrainz requires to every request (see
//! https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting). It also sets no
//! authentication: the web service is public.
//!
//! MusicBrainz answers a request rate above its published average with `503`,
//! and a mirror may answer `429`; both become [`ProviderError::RateLimited`] so
//! the server backs off rather than treating the request as a failed provider.

use serde::de::DeserializeOwned;

use hearthsound_plugin::host::http::{Request, Response};
use hearthsound_plugin::http;
use hearthsound_plugin::types::ProviderError;

use crate::cache;
use crate::url;

/// Wall-clock budget for one web service request, in milliseconds.
///
/// The host clamps this to its own ceiling and to the remaining call deadline.
/// MusicBrainz is occasionally slow, so the budget is generous while staying
/// well inside the instance's 30 second call timeout.
const REQUEST_TIMEOUT_MS: u32 = 15_000;

/// Delay reported with a rate-limited error when the response carries no
/// `Retry-After` header, in milliseconds.
///
/// MusicBrainz answers an exceeded rate with `503` and no `Retry-After`, so in
/// practice this is the delay the server sees.
const DEFAULT_RETRY_AFTER_MS: u32 = 1_000;

/// Number of characters of a response body kept in an error message.
///
/// A response body is at most a few hundred kilobytes, but an error string only
/// needs enough of it for a human to recognize the failure.
const ERROR_BODY_CHARS: usize = 200;

/// Sends a GET request to a web service endpoint and decodes the JSON body.
///
/// # Parameters
///
/// - `base_url`: base URL of the web service, with or without a trailing
///   slash.
/// - `path`: web service path, already query-encoded.
///
/// # Returns
///
/// The decoded response body.
///
/// # Errors
///
/// Returns the [`status_error`] of a non-`2xx` response,
/// [`ProviderError::Unavailable`] when the host cannot perform the request at
/// all, and [`ProviderError::Internal`] when the body is not JSON for `T`.
pub(crate) fn get_json<T: DeserializeOwned>(
    base_url: &str,
    path: &str,
) -> Result<T, ProviderError> {
    get_json_at(&url::endpoint(base_url, path))
}

/// How long a lookup body stays readable in the cache, in milliseconds.
///
/// One hour: a release or an artist read by one page answer is asked for again
/// by the next album of the same artist, and the web service rate-limits one
/// request per second. An hour is long enough that a whole import or an index
/// run reads each entity once, and short enough that a correction MusicBrainz
/// made reaches the server the same day.
pub(crate) const LOOKUP_TTL_MS: u64 = 3_600_000;

/// Reads a web service lookup, from the cache when the body is stored.
///
/// A page asks for the same release or artist more than once - an album's page,
/// its release group's earliest release, and the artist its discography lists
/// all name one entity - and the web service allows one request per second, so
/// the body of a lookup is stored and reused. The body is cached as text, exactly
/// as the service sent it, so a cached answer costs no re-encoding.
///
/// # Parameters
///
/// - `store`: the cache to read and write.
/// - `base_url`: base URL of the web service.
/// - `path`: web service path, already query-encoded.
///
/// # Returns
///
/// The decoded response body.
///
/// # Errors
///
/// Returns the same errors as [`get_json`]. A cache that cannot be read or
/// written costs a request, never a failed call.
pub(crate) fn get_json_cached<T: DeserializeOwned>(
    store: &dyn cache::Store,
    base_url: &str,
    path: &str,
) -> Result<T, ProviderError> {
    let key = format!("body:{path}");
    if let Some(text) = store.read(&key)
        && let Ok(value) = serde_json::from_str::<T>(&text)
    {
        debug!("lookup {key} was used");

        return Ok(value);
    }

    let endpoint = url::endpoint(base_url, path);
    let response = http::send(&request(&endpoint))?;
    if !http::is_success(&response) {
        return Err(status_error(&response, &endpoint));
    }
    let text = String::from_utf8_lossy(&response.body).into_owned();
    let value = serde_json::from_str(&text)
        .map_err(|error| ProviderError::Internal(format!("{endpoint}: {error}")))?;
    store.write(&key, &text, LOOKUP_TTL_MS);

    Ok(value)
}

/// Sends a GET request to an absolute URL and decodes the JSON body.
///
/// Wikidata, Wikipedia and ListenBrainz are read at URLs of their own rather
/// than under the configured web service, so their endpoints are built whole.
///
/// # Parameters
///
/// - `endpoint`: the absolute URL of the endpoint, already query-encoded.
///
/// # Returns
///
/// The decoded response body.
///
/// # Errors
///
/// Returns the same errors as [`get_json`].
pub(crate) fn get_json_at<T: DeserializeOwned>(endpoint: &str) -> Result<T, ProviderError> {
    send_json(request(endpoint))
}

/// Builds the GET request of the ListenBrainz popularity endpoint.
///
/// The endpoint is not public any more: it answers every anonymous request with
/// `401` ("you need to provide an Auth token"), which is why the token is not
/// optional here. A plugin instance without one does not ask at all - see
/// [`crate::catalog::artist_top_tracks`] - so this returns `None` rather than a
/// request that is certain to be refused.
///
/// # Parameters
///
/// - `endpoint`: the absolute URL of the endpoint, already query-encoded.
/// - `token`: the configured ListenBrainz token, trimmed; an empty one is no
///   token.
///
/// # Returns
///
/// The request, carrying the plugin's request timeout and `Authorization: Token
/// <token>`, or `None` when no usable token is configured.
pub(crate) fn popularity_request(endpoint: &str, token: Option<&str>) -> Option<Request> {
    let token = token.map(str::trim).filter(|token| !token.is_empty())?;
    let mut request = request(endpoint);
    request
        .headers
        .push(("authorization".to_owned(), format!("Token {token}")));

    Some(request)
}

/// Builds the GET request of one endpoint without sending it.
///
/// A caller that has to label its request adds the headers to the returned
/// request and sends it with [`send_json`].
///
/// # Parameters
///
/// - `endpoint`: the absolute URL of the endpoint, already query-encoded.
///
/// # Returns
///
/// The request, carrying the plugin's request timeout and no headers.
pub(crate) fn request(endpoint: &str) -> Request {
    let mut request = http::get(endpoint);
    request.timeout_ms = Some(REQUEST_TIMEOUT_MS);

    request
}

/// Sends one prepared request and decodes the JSON body.
///
/// # Parameters
///
/// - `request`: the request to perform.
///
/// # Returns
///
/// The decoded response body.
///
/// # Errors
///
/// Returns the [`status_error`] of a non-`2xx` response,
/// [`ProviderError::Unavailable`] when the host cannot perform the request at
/// all, and [`ProviderError::Internal`] when the body is not JSON for `T`.
pub(crate) fn send_json<T: DeserializeOwned>(request: Request) -> Result<T, ProviderError> {
    let endpoint = request.url.clone();
    let response = http::send(&request)?;

    decode(&response, &endpoint)
}

/// Decodes one web service response.
///
/// # Parameters
///
/// - `response`: the response to decode.
/// - `url`: the request URL, used in error messages.
///
/// # Returns
///
/// The decoded body.
///
/// # Errors
///
/// Returns the [`status_error`] of a non-`2xx` response, and
/// [`ProviderError::Internal`] when a successful response is not JSON for `T`.
fn decode<T: DeserializeOwned>(response: &Response, url: &str) -> Result<T, ProviderError> {
    if !http::is_success(response) {
        return Err(status_error(response, url));
    }

    serde_json::from_slice(&response.body)
        .map_err(|error| ProviderError::Internal(format!("{url}: {error}")))
}

/// Maps a non-success response to a provider error.
///
/// # Parameters
///
/// - `response`: a response whose status is not `2xx`.
/// - `url`: the request URL, used in the messages.
///
/// # Returns
///
/// [`ProviderError::NotFound`] for `404`,
/// [`ProviderError::RateLimited`] for `429` and `503` (the `Retry-After` header
/// in milliseconds, or [`DEFAULT_RETRY_AFTER_MS`] when it is absent or not a
/// number of seconds), [`ProviderError::Unauthorized`] for `401` and `403`,
/// [`ProviderError::Unavailable`] for every other `5xx`, and
/// [`ProviderError::Internal`] for anything else - a `400`, for example, means
/// the plugin built a query the web service rejects, which is its own fault
/// rather than the provider's. Message carriers hold the status and at most the
/// first [`ERROR_BODY_CHARS`] characters of the body.
fn status_error(response: &Response, url: &str) -> ProviderError {
    match response.status {
        404 => ProviderError::NotFound,
        429 | 503 => {
            ProviderError::RateLimited(retry_after_ms(response).unwrap_or(DEFAULT_RETRY_AFTER_MS))
        }
        401 | 403 => ProviderError::Unauthorized(describe(response, url)),
        status if (500..600).contains(&status) => {
            ProviderError::Unavailable(describe(response, url))
        }
        _ => ProviderError::Internal(describe(response, url)),
    }
}

/// Renders a status line and an excerpt of the body for an error message.
///
/// # Parameters
///
/// - `response`: the response to describe.
/// - `url`: the request URL.
///
/// # Returns
///
/// `<url> answered HTTP <status>`, followed by `: <excerpt>` when the body is
/// not empty.
fn describe(response: &Response, url: &str) -> String {
    let excerpt = body_excerpt(response);
    if excerpt.is_empty() {
        format!("{url} answered HTTP {}", response.status)
    } else {
        format!("{url} answered HTTP {}: {excerpt}", response.status)
    }
}

/// Returns the body of a response as a short, printable excerpt.
///
/// # Parameters
///
/// - `response`: the response to read.
///
/// # Returns
///
/// The body decoded as UTF-8 with invalid sequences replaced, whitespace
/// collapsed, and truncated to [`ERROR_BODY_CHARS`] characters.
fn body_excerpt(response: &Response) -> String {
    let text = String::from_utf8_lossy(&response.body);
    let mut excerpt: String = text
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .chars()
        .take(ERROR_BODY_CHARS)
        .collect();

    if text.chars().count() > ERROR_BODY_CHARS {
        excerpt.push_str("...");
    }

    excerpt
}

/// Reads the `Retry-After` header as a delay in milliseconds.
///
/// # Parameters
///
/// - `response`: the response to read the header from.
///
/// # Returns
///
/// The header value in seconds times 1000, saturated to `u32`, or `None` when
/// the header is absent, not a number, or the HTTP-date form of `Retry-After`
/// (which the web service does not use).
fn retry_after_ms(response: &Response) -> Option<u32> {
    let value = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.trim())?;

    value
        .parse::<u32>()
        .ok()
        .map(|seconds| seconds.saturating_mul(1_000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Store as _;

    /// A store that keeps its entries in memory and records its writes.
    struct MemoryStore {
        entries: std::cell::RefCell<std::collections::HashMap<String, String>>,
        writes: std::cell::RefCell<Vec<(String, u64)>>,
    }

    impl MemoryStore {
        /// Creates an empty store.
        fn empty() -> MemoryStore {
            MemoryStore {
                entries: std::cell::RefCell::new(std::collections::HashMap::new()),
                writes: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl cache::Store for MemoryStore {
        fn read(&self, key: &str) -> Option<String> {
            self.entries.borrow().get(key).cloned()
        }

        fn write(&self, key: &str, value: &str, ttl_ms: u64) {
            self.entries
                .borrow_mut()
                .insert(key.to_owned(), value.to_owned());
            self.writes.borrow_mut().push((key.to_owned(), ttl_ms));
        }
    }

    /// A lookup whose body is stored is answered from the store.
    ///
    /// The host's HTTP import does not exist on this target and aborts the
    /// process when it is called, so an answer proves the request was never
    /// made.
    #[test]
    fn a_stored_lookup_body_is_answered_from_the_cache() {
        let store = MemoryStore::empty();
        let path = "release/6360f948-17d1-4135-a580-34dc200930d8?inc=recordings&fmt=json";
        store.write(
            &format!("body:{path}"),
            r#"{"id":"6360f948-17d1-4135-a580-34dc200930d8","title":"Blue Monday","media":[]}"#,
            60_000,
        );

        let release: crate::musicbrainz::Release =
            get_json_cached(&store, "https://musicbrainz.invalid/ws/2/", path).unwrap();

        assert_eq!(release.id, "6360f948-17d1-4135-a580-34dc200930d8");
        assert_eq!(release.title, "Blue Monday");
        assert_eq!(
            store.writes.borrow().len(),
            1,
            "the cached answer is not written again"
        );
    }

    /// Builds a response with the given status, headers and body.
    fn response(status: u16, headers: &[(&str, &str)], body: &str) -> Response {
        Response {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn decodes_a_successful_response() {
        let decoded: serde_json::Value = decode(
            &response(200, &[], r#"{"isrc":"GBBPW9800010"}"#),
            "https://x",
        )
        .unwrap();

        assert_eq!(decoded["isrc"], "GBBPW9800010");
    }

    #[test]
    fn a_missing_document_is_not_found() {
        let error = decode::<serde_json::Value>(
            &response(404, &[], r#"{"error":"Not Found"}"#),
            "https://x",
        )
        .unwrap_err();

        assert!(matches!(error, ProviderError::NotFound));
    }

    #[test]
    fn a_rate_limited_response_carries_the_retry_delay() {
        assert!(matches!(
            decode::<serde_json::Value>(&response(429, &[("retry-after", "2")], ""), "https://x")
                .unwrap_err(),
            ProviderError::RateLimited(2_000)
        ));
        // The header name is case-insensitive.
        assert!(matches!(
            decode::<serde_json::Value>(&response(429, &[("Retry-After", "30")], ""), "https://x")
                .unwrap_err(),
            ProviderError::RateLimited(30_000)
        ));
    }

    #[test]
    fn a_busy_response_is_rate_limited() {
        // MusicBrainz answers an exceeded request rate with 503 and no
        // Retry-After header.
        assert!(matches!(
            decode::<serde_json::Value>(
                &response(503, &[], "The MusicBrainz web server is currently busy."),
                "https://x"
            )
            .unwrap_err(),
            ProviderError::RateLimited(1_000)
        ));
    }

    #[test]
    fn an_unreadable_retry_delay_falls_back_to_the_default() {
        assert!(matches!(
            decode::<serde_json::Value>(
                &response(429, &[("retry-after", "soon")], ""),
                "https://x"
            )
            .unwrap_err(),
            ProviderError::RateLimited(1_000)
        ));
        assert!(matches!(
            decode::<serde_json::Value>(
                &response(429, &[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")], ""),
                "https://x"
            )
            .unwrap_err(),
            ProviderError::RateLimited(1_000)
        ));
    }

    #[test]
    fn other_server_failures_are_unavailable() {
        let error = decode::<serde_json::Value>(
            &response(500, &[], "Internal Server Error"),
            "https://musicbrainz.org/ws/2/isrc/X",
        )
        .unwrap_err();

        match error {
            ProviderError::Unavailable(message) => {
                assert!(message.contains("HTTP 500"), "{message}");
                assert!(message.contains("Internal Server Error"), "{message}");
            }
            _ => panic!("expected an unavailable error"),
        }
    }

    #[test]
    fn a_rejected_request_is_an_internal_error() {
        let error = decode::<serde_json::Value>(
            &response(400, &[], r#"{"error":"Invalid mbid."}"#),
            "https://musicbrainz.org/ws/2/release/nope",
        )
        .unwrap_err();

        match error {
            ProviderError::Internal(message) => {
                assert!(message.contains("HTTP 400"), "{message}");
                assert!(message.contains("Invalid mbid"), "{message}");
            }
            _ => panic!("expected an internal error"),
        }
    }

    #[test]
    fn a_denied_request_is_unauthorized() {
        assert!(matches!(
            decode::<serde_json::Value>(&response(403, &[], "denied"), "https://x").unwrap_err(),
            ProviderError::Unauthorized(_)
        ));
    }

    #[test]
    fn a_body_that_is_not_json_is_an_internal_error() {
        let error =
            decode::<serde_json::Value>(&response(200, &[], "<html>"), "https://x").unwrap_err();

        assert!(matches!(error, ProviderError::Internal(_)));
    }

    #[test]
    fn an_excerpt_is_short_and_printable() {
        let long = "x".repeat(500);
        let excerpt = body_excerpt(&response(500, &[], &long));

        assert_eq!(excerpt.chars().count(), ERROR_BODY_CHARS + 3);
        assert!(excerpt.ends_with("..."));

        let whitespace = body_excerpt(&response(500, &[], "a\n  b\tc"));
        assert_eq!(whitespace, "a b c");
    }
}
