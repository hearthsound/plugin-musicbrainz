//! Scoring of recording candidates against the hint they were resolved from.
//!
//! A candidate arrives from one of two places: an ISRC lookup, where the
//! identifier itself is the evidence, or a recording search, where MusicBrainz
//! supplies a relevance score. Either way the candidate starts from a base
//! score in `0..=1` and is then adjusted by how well its title and duration
//! agree with the hint, so a search result the index ranked highly but that
//! names a different take lands below one that actually matches.
//!
//! # Formula
//!
//! ```text
//! base      = 1.0 for an ISRC hit, MusicBrainz score / 100 for a search hit
//! quality   = sum(weight(signal) * agreement(signal)) / sum(weight(signal))
//! score     = clamp(base * quality, 0, 1)
//! ```
//!
//! The signals are the title (weight 2) and the duration (weight 1). A signal
//! the hint or the candidate does not carry is left out of the average, so a
//! hint with no duration is judged on its title alone: an ISRC hit that agrees
//! on the title scores exactly 1.0, while one whose title disagrees scores 0.3.
//! When neither signal is available the quality is 1.0 and the base score
//! stands unchanged.
//!
//! This module is pure and unit-tested on the host target.

/// Weight of the title signal in the agreement average.
const TITLE_WEIGHT: f32 = 2.0;

/// Weight of the duration signal in the agreement average.
const DURATION_WEIGHT: f32 = 1.0;

/// Agreement of two durations within [`DURATION_MATCH_MS`].
const DURATION_MATCH: f32 = 1.0;

/// Agreement of two titles that normalize to the same text.
const TITLE_EQUAL: f32 = 1.0;

/// Agreement of two titles where one normalizes to a substring of the other,
/// which covers suffixed takes such as `Roygbiv (album version)`.
const TITLE_CONTAINS: f32 = 0.8;

/// Agreement of two titles that share no containment, which keeps a candidate
/// in the result set but well below the confidence threshold.
const TITLE_DIFFERENT: f32 = 0.3;

/// Difference in milliseconds within which two durations count as the same
/// recording. Encoders pad and truncate a little, so a couple of seconds is
/// not evidence of a different take.
const DURATION_MATCH_MS: u32 = 2_000;

/// Difference in milliseconds within which two durations still agree.
const DURATION_NEAR_MS: u32 = 5_000;

/// Difference in milliseconds within which two durations are weak evidence at
/// best; beyond it they disagree.
const DURATION_LOOSE_MS: u32 = 15_000;

/// Normalizes a title for comparison.
///
/// Case is folded, every character that is not a letter or a digit is dropped,
/// runs of dropped characters become a single space, and the result is trimmed.
///
/// # Parameters
///
/// - `value`: the raw title.
///
/// # Returns
///
/// The comparison form, which is empty when the title carried no letters or
/// digits.
///
/// # Example
///
/// ```text
/// normalize_title("Roygbiv (Album Version)") -> "roygbiv album version"
/// normalize_title("  Royal National Anthem ")  -> "royal national anthem"
/// ```
pub fn normalize_title(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut separator = false;
    for c in value.chars() {
        if c.is_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push(' ');
            }
            separator = false;
            normalized.extend(c.to_lowercase());
        } else {
            separator = true;
        }
    }
    normalized
}

/// Returns how much a candidate title agrees with the hint's title.
///
/// # Parameters
///
/// - `hint`: the title from the recording hint.
/// - `candidate`: the title MusicBrainz reports.
///
/// # Returns
///
/// [`TITLE_EQUAL`] when the two normalize to the same text, including when
/// either side is empty and there is nothing to disagree about,
/// [`TITLE_CONTAINS`] when one normalized title contains the other, and
/// [`TITLE_DIFFERENT`] otherwise.
///
/// # Example
///
/// ```text
/// title_agreement("Roygbiv", "Roygbiv")            -> 1.0
/// title_agreement("Roygbiv", "Roygbiv (Live)")     -> 0.8
/// title_agreement("Roygbiv", "Rue The Whirl")      -> 0.3
/// ```
pub fn title_agreement(hint: &str, candidate: &str) -> f32 {
    let hint = normalize_title(hint);
    let candidate = normalize_title(candidate);

    if hint.is_empty() || candidate.is_empty() || hint == candidate {
        TITLE_EQUAL
    } else if candidate.contains(&hint) || hint.contains(&candidate) {
        TITLE_CONTAINS
    } else {
        TITLE_DIFFERENT
    }
}

/// Returns how much a candidate duration agrees with the hint's duration.
///
/// # Parameters
///
/// - `hint_ms`: the duration from the recording hint, in milliseconds.
/// - `candidate_ms`: the duration MusicBrainz reports, in milliseconds.
///
/// # Returns
///
/// `1.0` within [`DURATION_MATCH_MS`], `0.9` within [`DURATION_NEAR_MS`],
/// `0.5` within [`DURATION_LOOSE_MS`], and `0.0` beyond it.
///
/// # Example
///
/// ```text
/// duration_agreement(139_253, 139_253) -> 1.0
/// duration_agreement(139_253, 141_000) -> 1.0
/// duration_agreement(139_253, 143_000) -> 0.9
/// duration_agreement(139_253, 160_000) -> 0.0
/// ```
pub fn duration_agreement(hint_ms: u32, candidate_ms: u32) -> f32 {
    let difference = hint_ms.abs_diff(candidate_ms);
    if difference <= DURATION_MATCH_MS {
        DURATION_MATCH
    } else if difference <= DURATION_NEAR_MS {
        0.9
    } else if difference <= DURATION_LOOSE_MS {
        0.5
    } else {
        0.0
    }
}

/// Scores one candidate recording against the hint it was resolved from.
///
/// The base score comes from the lookup that produced the candidate and the
/// agreement average adjusts it, as described in the module documentation. The
/// core of the server treats a score of `0.9` or more as a confident match, so
/// a candidate only keeps that confidence when every signal the hint carries
/// agrees.
///
/// # Parameters
///
/// - `base`: `1.0` for a recording an ISRC lookup returned, otherwise the
///   MusicBrainz search score divided by 100. Values outside `0..=1` are
///   clamped.
/// - `hint_title`: the hint's title, when it has one.
/// - `hint_duration_ms`: the hint's duration in milliseconds, when it has one.
/// - `candidate_title`: the title MusicBrainz reports for the candidate.
/// - `candidate_duration_ms`: the duration MusicBrainz reports for the
///   candidate, when it has one.
///
/// # Returns
///
/// The adjusted score, in `0..=1`.
///
/// # Example
///
/// ```text
/// match_score(1.0, Some("Roygbiv"), Some(139_253), "Roygbiv", Some(139_253)) -> 1.0
/// match_score(0.8, Some("Roygbiv"), None, "Roygbiv", None)                  -> 0.8
/// match_score(1.0, Some("Roygbiv"), None, "Rue The Whirl", None)            -> 0.3
/// ```
pub fn match_score(
    base: f32,
    hint_title: Option<&str>,
    hint_duration_ms: Option<u32>,
    candidate_title: &str,
    candidate_duration_ms: Option<u32>,
) -> f32 {
    let mut weighted = 0.0;
    let mut weight = 0.0;

    if let Some(title) = hint_title.filter(|title| !normalize_title(title).is_empty()) {
        weighted += TITLE_WEIGHT * title_agreement(title, candidate_title);
        weight += TITLE_WEIGHT;
    }

    if let (Some(hint), Some(candidate)) = (hint_duration_ms, candidate_duration_ms) {
        weighted += DURATION_WEIGHT * duration_agreement(hint, candidate);
        weight += DURATION_WEIGHT;
    }

    let quality = if weight == 0.0 {
        1.0
    } else {
        weighted / weight
    };

    (base.clamp(0.0, 1.0) * quality).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_folds_case_and_punctuation() {
        assert_eq!(
            normalize_title("Roygbiv (Album Version)"),
            "roygbiv album version"
        );
        assert_eq!(normalize_title("  Rue   The Whirl  "), "rue the whirl");
        assert_eq!(normalize_title("AC/DC"), "ac dc");
        assert_eq!(normalize_title("!!!"), "");
    }

    #[test]
    fn an_identical_title_agrees_fully() {
        assert_eq!(title_agreement("Roygbiv", "Roygbiv"), 1.0);
        assert_eq!(title_agreement("roygbiv!", "Roygbiv"), 1.0);
    }

    #[test]
    fn a_contained_title_agrees_mostly() {
        assert_eq!(title_agreement("Roygbiv", "Roygbiv (Live)"), 0.8);
        assert_eq!(title_agreement("Roygbiv (Live)", "Roygbiv"), 0.8);
    }

    #[test]
    fn a_different_title_disagrees() {
        assert_eq!(title_agreement("Roygbiv", "Rue The Whirl"), 0.3);
        assert_eq!(title_agreement("Roygbiv", ""), 1.0);
    }

    #[test]
    fn duration_bands_are_ordered() {
        assert_eq!(duration_agreement(139_253, 139_253), 1.0);
        assert_eq!(duration_agreement(139_253, 141_253), 1.0);
        assert_eq!(duration_agreement(139_253, 143_000), 0.9);
        assert_eq!(duration_agreement(139_253, 150_000), 0.5);
        assert_eq!(duration_agreement(139_253, 160_000), 0.0);
    }

    #[test]
    fn an_isrc_hit_that_agrees_stays_confident() {
        let score = match_score(
            1.0,
            Some("Roygbiv"),
            Some(139_253),
            "Roygbiv",
            Some(139_253),
        );

        assert_eq!(score, 1.0);
        assert!(score >= 0.9);
    }

    #[test]
    fn an_isrc_hit_with_only_a_title_stays_confident() {
        let score = match_score(1.0, Some("Roygbiv"), None, "Roygbiv", None);

        assert_eq!(score, 1.0);
    }

    #[test]
    fn a_search_hit_is_scaled_by_its_relevance() {
        assert_eq!(
            match_score(0.8, Some("Roygbiv"), None, "Roygbiv", None),
            0.8
        );
        assert_eq!(match_score(0.5, None, None, "Anything", None), 0.5);
    }

    #[test]
    fn a_disagreeing_title_pulls_a_candidate_below_the_threshold() {
        let score = match_score(1.0, Some("Roygbiv"), None, "Rue The Whirl", None);

        assert_eq!(score, 0.3);
        assert!(score < 0.9);
    }

    #[test]
    fn a_disagreeing_duration_pulls_only_its_own_weight() {
        // Title agrees (weight 2), duration is far off (weight 1).
        let score = match_score(
            1.0,
            Some("Roygbiv"),
            Some(139_253),
            "Roygbiv",
            Some(160_000),
        );

        assert!((score - 2.0 / 3.0).abs() < 1e-6, "{score}");
        assert!(score < 0.9);
    }

    #[test]
    fn a_hint_without_signals_keeps_the_base_score() {
        assert_eq!(match_score(1.0, None, None, "Anything", Some(1)), 1.0);
        assert_eq!(match_score(0.72, Some("   "), None, "Anything", None), 0.72);
    }

    #[test]
    fn the_base_score_is_clamped() {
        assert_eq!(match_score(2.0, None, None, "x", None), 1.0);
        assert_eq!(match_score(-1.0, None, None, "x", None), 0.0);
    }
}
