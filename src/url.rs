//! URL, identifier and query encoding helpers for the MusicBrainz web service.
//!
//! Everything here is pure: it never touches the network and never depends on
//! the plugin bindings, so it is unit-tested on the host target.

/// Base URL of the MusicBrainz web service.
pub const DEFAULT_BASE_URL: &str = "https://musicbrainz.org/ws/2/";

/// Base URL of the Cover Art Archive.
pub const DEFAULT_ARTWORK_BASE_URL: &str = "https://coverartarchive.org/";

/// Returns whether `value` is an absolute `http` or `https` URL.
///
/// The check is deliberately structural rather than a full URL parse: it
/// accepts exactly the shapes a MusicBrainz base URL can have (the public web
/// service, a self-hosted mirror, or a test double on a private host), and
/// rejects everything a request could not be built from.
///
/// # Parameters
///
/// - `value`: the candidate URL; surrounding whitespace is ignored.
///
/// # Returns
///
/// `true` when `value` starts with `http://` or `https://`, carries a non-empty
/// host, and contains no whitespace.
///
/// # Example
///
/// ```text
/// is_http_url("https://musicbrainz.org/ws/2/")  -> true
/// is_http_url("http://musicbrainz.internal/ws/2/") -> true
/// is_http_url("musicbrainz.org/ws/2/")          -> false
/// is_http_url("ftp://musicbrainz.org/ws/2/")    -> false
/// ```
pub fn is_http_url(value: &str) -> bool {
    let value = value.trim();
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    let Some(rest) = rest else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !value.chars().any(char::is_whitespace)
}

/// Normalizes a configured base URL to the web service root.
///
/// Surrounding whitespace is removed and exactly one trailing slash is kept, so
/// that the endpoint paths the plugin appends always land on the web service
/// root however the administrator spelled the value.
///
/// # Parameters
///
/// - `value`: the configured base URL.
///
/// # Returns
///
/// The trimmed URL with exactly one trailing slash, or the empty string when
/// `value` is empty or only slashes.
///
/// # Example
///
/// ```text
/// normalize_base_url(" https://musicbrainz.org/ws/2 ") -> "https://musicbrainz.org/ws/2/"
/// normalize_base_url("https://musicbrainz.org/ws/2/")  -> "https://musicbrainz.org/ws/2/"
/// ```
pub fn normalize_base_url(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

/// Joins a base URL with a web service path.
///
/// # Parameters
///
/// - `base_url`: the configured base URL, with or without a trailing slash.
/// - `path`: the web service path, with or without a leading slash.
///
/// # Returns
///
/// The absolute URL of the endpoint.
///
/// # Example
///
/// ```text
/// endpoint("https://musicbrainz.org/ws/2/", "isrc/GBBPW9800010") -> "https://musicbrainz.org/ws/2/isrc/GBBPW9800010"
/// endpoint("https://mirror.example/ws/2", "/release/abc")        -> "https://mirror.example/ws/2/release/abc"
/// ```
pub fn endpoint(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        normalize_base_url(base_url).trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Percent-encodes one path or query component.
///
/// Everything outside the RFC 3986 unreserved set is escaped, including `/`,
/// `&`, `=`, `?` and `#`, so an identifier or a Lucene query cannot add
/// parameters or change the endpoint it is pasted into. A space becomes `%20`
/// rather than `+`, which the web service decodes back to a space either way.
///
/// # Parameters
///
/// - `value`: the component to encode.
///
/// # Returns
///
/// The encoded component, with every byte of a multi-byte character escaped
/// separately.
///
/// # Example
///
/// ```text
/// encode_component("Boards of Canada")        -> "Boards%20of%20Canada"
/// encode_component("recording:\"Roygbiv\"")   -> "recording%3A%22Roygbiv%22"
/// ```
pub fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(*byte));
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
            }
        }
    }
    encoded
}

/// Uppercase hexadecimal digits used by [`encode_component`].
const HEX_DIGITS: [u8; 16] = *b"0123456789ABCDEF";

/// Normalizes an ISRC to the form the web service keys recordings by.
///
/// Hints carry ISRCs as they were written into the tags of a file, which is
/// usually the hyphenated print form (`GB-BPW-98-00010`); MusicBrainz indexes
/// the bare code.
///
/// # Parameters
///
/// - `value`: the ISRC as the caller supplied it.
///
/// # Returns
///
/// The same code with whitespace and hyphens removed and every letter
/// upper-cased. A value that is not an ISRC is returned with those characters
/// removed too, so the caller can test the result with [`is_valid_isrc`].
///
/// # Example
///
/// ```text
/// normalize_isrc("gb-bpw-98-00010")  -> "GBBPW9800010"
/// normalize_isrc("GBBPW9800010")     -> "GBBPW9800010"
/// ```
pub fn normalize_isrc(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase()
}

/// Returns whether `value` is a bare ISRC.
///
/// # Parameters
///
/// - `value`: the candidate code, already normalized by [`normalize_isrc`].
///
/// # Returns
///
/// `true` when `value` is exactly 12 ASCII letters or digits.
///
/// # Example
///
/// ```text
/// is_valid_isrc("GBBPW9800010") -> true
/// is_valid_isrc("GB-BPW-9800010") -> false
/// is_valid_isrc("") -> false
/// ```
pub fn is_valid_isrc(value: &str) -> bool {
    value.len() == 12 && value.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https_urls() {
        assert!(is_http_url("https://musicbrainz.org/ws/2/"));
        assert!(is_http_url("http://musicbrainz.internal:8080/ws/2/"));
        assert!(is_http_url("  https://mirror.example/ws/2  "));
    }

    #[test]
    fn rejects_urls_without_a_host_or_scheme() {
        assert!(!is_http_url("musicbrainz.org/ws/2/"));
        assert!(!is_http_url("ftp://musicbrainz.org/ws/2/"));
        assert!(!is_http_url("https://"));
        assert!(!is_http_url("https:///ws/2/"));
        assert!(!is_http_url("http://music brainz.org/ws/2/"));
    }

    #[test]
    fn normalization_keeps_exactly_one_trailing_slash() {
        assert_eq!(
            normalize_base_url("  https://musicbrainz.org/ws/2  "),
            "https://musicbrainz.org/ws/2/"
        );
        assert_eq!(
            normalize_base_url("https://musicbrainz.org/ws/2///"),
            "https://musicbrainz.org/ws/2/"
        );
        assert_eq!(normalize_base_url(""), "");
        assert_eq!(normalize_base_url(" / "), "");
    }

    #[test]
    fn endpoint_joins_exactly_one_slash() {
        assert_eq!(
            endpoint("https://musicbrainz.org/ws/2/", "isrc/GBBPW9800010"),
            "https://musicbrainz.org/ws/2/isrc/GBBPW9800010"
        );
        assert_eq!(
            endpoint("https://mirror.example/ws/2", "/release/abc"),
            "https://mirror.example/ws/2/release/abc"
        );
        assert_eq!(
            endpoint(
                "https://musicbrainz.org/ws/2/",
                "recording?query=x&fmt=json"
            ),
            "https://musicbrainz.org/ws/2/recording?query=x&fmt=json"
        );
    }

    #[test]
    fn encoding_escapes_reserved_characters() {
        assert_eq!(encode_component("Boards of Canada"), "Boards%20of%20Canada");
        assert_eq!(
            encode_component("recording:\"Roygbiv\""),
            "recording%3A%22Roygbiv%22"
        );
        assert_eq!(
            encode_component("dur:[1000 TO 2000]"),
            "dur%3A%5B1000%20TO%202000%5D"
        );
        assert_eq!(encode_component("a=1&b=2"), "a%3D1%26b%3D2");
        assert_eq!(encode_component("MB-ID_1.0~"), "MB-ID_1.0~");
    }

    #[test]
    fn encoding_escapes_every_utf8_byte() {
        assert_eq!(encode_component("Bjork"), "Bjork");
        assert_eq!(
            encode_component("Boards of Canada - Roygbiv"),
            "Boards%20of%20Canada%20-%20Roygbiv"
        );
    }

    #[test]
    fn isrc_normalization_strips_hyphens_and_whitespace() {
        assert_eq!(normalize_isrc("gb-bpw-98-00010"), "GBBPW9800010");
        assert_eq!(normalize_isrc(" GBBPW9800010 "), "GBBPW9800010");
        assert_eq!(normalize_isrc("GBBPW9800010"), "GBBPW9800010");
        assert_eq!(normalize_isrc(""), "");
    }

    #[test]
    fn only_bare_twelve_character_isrcs_are_valid() {
        assert!(is_valid_isrc("GBBPW9800010"));
        assert!(is_valid_isrc("usum71704040"));
        assert!(!is_valid_isrc("GB-BPW-9800010"));
        assert!(!is_valid_isrc("GBBPW980001"));
        assert!(!is_valid_isrc("GBBPW98000100"));
        assert!(!is_valid_isrc(""));
    }
}
