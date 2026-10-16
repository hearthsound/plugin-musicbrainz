//! Lucene query construction for the MusicBrainz search indexes.
//!
//! The web service takes the `query` parameter of a search request as a Lucene
//! query, so a title, an artist name or a barcode pasted into it unescaped
//! could change the query rather than constrain it. This module builds the
//! query for one recording hint, for the release of a barcode and for the
//! release group of an album, and escapes every value that goes into it. It is
//! pure: it never touches the network, so it is unit-tested on the host target.

/// Number of recordings a search asks for.
///
/// Ten candidates are enough for the scorer to rank, and the server trims the
/// response to what it needs; a larger page would only cost bandwidth and
/// memory.
pub const SEARCH_LIMIT: u32 = 10;

/// Half-width of the duration window a search asks the index for, in
/// milliseconds.
///
/// A hint's duration comes from a file's tags or from a catalogue provider, and
/// is commonly a few seconds off the recording it names, so the window is
/// deliberately wider than the scoring bands in [`crate::score`].
pub const DURATION_WINDOW_MS: u32 = 5_000;

/// Characters that mean something to Lucene's query parser.
///
/// `&&` and `||` are covered by escaping `&` and `|` individually: escaping is
/// a no-op for the parser's purposes either way, and escaping both characters
/// keeps the rule uniform.
const RESERVED: [char; 19] = [
    '+', '-', '!', '(', ')', '{', '}', '[', ']', '^', '"', '~', '*', '?', ':', '\\', '/', '&', '|',
];

/// Escapes every Lucene operator in `value`.
///
/// # Parameters
///
/// - `value`: the raw text, for example a title or an artist name.
///
/// # Returns
///
/// The text with a backslash in front of every character of [`RESERVED`], which
/// makes the parser treat each of them as a literal.
///
/// # Example
///
/// ```text
/// escape_lucene("Boards of Canada")        -> "Boards of Canada"
/// escape_lucene("Rock (Remix)")            -> "Rock \\(Remix\\)"
/// escape_lucene("AC/DC")                   -> "AC\\/DC"
/// escape_lucene("Who? What! + More")       -> "Who\\? What\\! \\+ More"
/// ```
pub fn escape_lucene(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        if RESERVED.contains(&c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Builds the Lucene query for one recording hint.
///
/// The query is a conjunction of the signals the hint carries:
///
/// - `recording:"<title>"`, always present because a hint without a title is
///   not searchable;
/// - `artist:"<first artist>"`, the first non-empty artist of the hint;
/// - `release:"<album>"`, when the hint names an album;
/// - `dur:[<duration - 5000> TO <duration + 5000>]`, when the hint carries a
///   duration; the lower bound saturates at zero so the range stays valid for
///   very short recordings.
///
/// # Parameters
///
/// - `title`: the hint's title.
/// - `artists`: the hint's artists, in order; only the first non-empty one is
///   used, because a MusicBrainz `artist` term matches the combined credit of a
///   recording.
/// - `album`: the hint's album, when it has one.
/// - `duration_ms`: the hint's duration in milliseconds, when it has one.
///
/// # Returns
///
/// The query text, or `None` when the title is empty or only whitespace. The
/// caller must URL-encode the result with
/// [`encode_component`](crate::url::encode_component) before putting it in a
/// request.
///
/// # Example
///
/// ```text
/// recording_query("Roygbiv", &[], None, None)
///   -> "recording:\"Roygbiv\""
/// recording_query("Roygbiv", &["Boards of Canada".into()], None, Some(139_000))
///   -> "recording:\"Roygbiv\" AND artist:\"Boards of Canada\" AND dur:[134000 TO 144000]"
/// recording_query("  ", &[], None, None)
///   -> None
/// ```
pub fn recording_query(
    title: &str,
    artists: &[String],
    album: Option<&str>,
    duration_ms: Option<u32>,
) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }

    let mut terms = vec![format!("recording:\"{}\"", escape_lucene(title))];

    if let Some(artist) = artists
        .iter()
        .map(String::as_str)
        .find(|a| !a.trim().is_empty())
    {
        terms.push(format!("artist:\"{}\"", escape_lucene(artist.trim())));
    }

    if let Some(album) = album.map(str::trim).filter(|a| !a.is_empty()) {
        terms.push(format!("release:\"{}\"", escape_lucene(album)));
    }

    if let Some(duration_ms) = duration_ms {
        terms.push(format!(
            "dur:[{} TO {}]",
            duration_ms.saturating_sub(DURATION_WINDOW_MS),
            duration_ms.saturating_add(DURATION_WINDOW_MS)
        ));
    }

    Some(terms.join(" AND "))
}

/// Builds the Lucene query for the release that carries a barcode.
///
/// # Parameters
///
/// - `barcode`: the barcode of the release, as the caller supplied it.
///
/// # Returns
///
/// The query text, or `None` when the barcode is empty or only whitespace and
/// there is nothing to search for. The caller must URL-encode the result with
/// [`encode_component`](crate::url::encode_component) before putting it in a
/// request.
///
/// # Example
///
/// ```text
/// barcode_query("602547508228") -> Some("barcode:602547508228")
/// barcode_query("  ")           -> None
/// ```
pub fn barcode_query(barcode: &str) -> Option<String> {
    let barcode = barcode.trim();
    if barcode.is_empty() {
        return None;
    }

    Some(format!("barcode:{}", escape_lucene(barcode)))
}

/// Builds the Lucene query for the release group of an album.
///
/// The query is the release group's title and, when the caller knows one, the
/// first non-empty artist of the album: a release group's `artist` term
/// matches the credit of the release group itself.
///
/// # Parameters
///
/// - `title`: the album title.
/// - `artists`: the album's artists, in order; only the first non-empty one is
///   used.
///
/// # Returns
///
/// The query text, or `None` when the title is empty or only whitespace. The
/// caller must URL-encode the result with
/// [`encode_component`](crate::url::encode_component) before putting it in a
/// request.
///
/// # Example
///
/// ```text
/// release_group_query("Music Has the Right to Children", &[])
///   -> "releasegroup:\"Music Has the Right to Children\""
/// release_group_query("Geogaddi", &["Boards of Canada".into()])
///   -> "releasegroup:\"Geogaddi\" AND artist:\"Boards of Canada\""
/// ```
pub fn release_group_query(title: &str, artists: &[String]) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }

    let mut terms = vec![format!("releasegroup:\"{}\"", escape_lucene(title))];

    if let Some(artist) = artists
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .find(|a| !a.is_empty())
    {
        terms.push(format!("artist:\"{}\"", escape_lucene(artist)));
    }

    Some(terms.join(" AND "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_survives_escaping() {
        assert_eq!(escape_lucene("Boards of Canada"), "Boards of Canada");
        assert_eq!(escape_lucene("Roygbiv"), "Roygbiv");
    }

    #[test]
    fn every_lucene_operator_is_escaped() {
        assert_eq!(escape_lucene("Rock (Remix)"), "Rock \\(Remix\\)");
        assert_eq!(escape_lucene("AC/DC"), "AC\\/DC");
        assert_eq!(
            escape_lucene("Who? What! + More"),
            "Who\\? What\\! \\+ More"
        );
        assert_eq!(escape_lucene("a && b || c"), "a \\&\\& b \\|\\| c");
        assert_eq!(
            escape_lucene("\"quoted\" [x] {y} ^z~*"),
            "\\\"quoted\\\" \\[x\\] \\{y\\} \\^z\\~\\*"
        );
        assert_eq!(escape_lucene("back\\slash"), "back\\\\slash");
        assert_eq!(escape_lucene("a:b - c"), "a\\:b \\- c");
    }

    #[test]
    fn a_title_alone_is_a_valid_query() {
        assert_eq!(
            recording_query("Roygbiv", &[], None, None).unwrap(),
            "recording:\"Roygbiv\""
        );
    }

    #[test]
    fn an_empty_title_is_not_searchable() {
        assert_eq!(recording_query("", &[], None, None), None);
        assert_eq!(
            recording_query(
                "   ",
                &["Boards of Canada".to_string()],
                Some("MHTRTC"),
                Some(1)
            ),
            None
        );
    }

    #[test]
    fn the_first_non_empty_artist_is_used() {
        let artists = vec![String::new(), " BoC ".to_string(), "Ignored".to_string()];
        assert_eq!(
            recording_query("Roygbiv", &artists, None, None).unwrap(),
            "recording:\"Roygbiv\" AND artist:\"BoC\""
        );
    }

    #[test]
    fn the_album_becomes_a_release_term() {
        assert_eq!(
            recording_query(
                "Roygbiv",
                &[],
                Some("Music Has the Right to Children"),
                None
            )
            .unwrap(),
            "recording:\"Roygbiv\" AND release:\"Music Has the Right to Children\""
        );
        assert_eq!(
            recording_query("Roygbiv", &[], Some("  "), None).unwrap(),
            "recording:\"Roygbiv\""
        );
    }

    #[test]
    fn the_duration_becomes_a_bounded_range() {
        assert_eq!(
            recording_query("Roygbiv", &[], None, Some(139_253)).unwrap(),
            "recording:\"Roygbiv\" AND dur:[134253 TO 144253]"
        );
    }

    #[test]
    fn a_short_duration_does_not_underflow_the_window() {
        assert_eq!(
            recording_query("Intro", &[], None, Some(1_000)).unwrap(),
            "recording:\"Intro\" AND dur:[0 TO 6000]"
        );
    }

    #[test]
    fn values_are_escaped_inside_the_query() {
        let artists = vec!["AC/DC".to_string()];
        assert_eq!(
            recording_query(
                "Rock (Remix)",
                &artists,
                Some("Live! [2024]"),
                Some(200_000)
            )
            .unwrap(),
            "recording:\"Rock \\(Remix\\)\" AND artist:\"AC\\/DC\" AND release:\"Live\\! \\[2024\\]\" AND dur:[195000 TO 205000]"
        );
    }

    #[test]
    fn a_barcode_becomes_a_barcode_term() {
        assert_eq!(
            barcode_query(" 602547508228 ").unwrap(),
            "barcode:602547508228"
        );
        assert_eq!(
            barcode_query("06025475-08228").unwrap(),
            "barcode:06025475\\-08228"
        );
    }

    #[test]
    fn an_empty_barcode_is_not_searchable() {
        assert_eq!(barcode_query(""), None);
        assert_eq!(barcode_query("   "), None);
    }

    #[test]
    fn a_release_group_title_alone_is_a_valid_query() {
        assert_eq!(
            release_group_query("Geogaddi", &[]).unwrap(),
            "releasegroup:\"Geogaddi\""
        );
        assert_eq!(
            release_group_query("  Music Has the Right to Children  ", &[]).unwrap(),
            "releasegroup:\"Music Has the Right to Children\""
        );
    }

    #[test]
    fn the_first_non_empty_artist_joins_the_release_group_query() {
        let artists = vec![String::new(), " Boards of Canada ".to_string()];

        assert_eq!(
            release_group_query("Geogaddi", &artists).unwrap(),
            "releasegroup:\"Geogaddi\" AND artist:\"Boards of Canada\""
        );
    }

    #[test]
    fn an_empty_release_group_title_is_not_searchable() {
        assert_eq!(release_group_query("", &[]), None);
        assert_eq!(
            release_group_query("  ", &["Boards of Canada".to_string()]),
            None
        );
    }
}
