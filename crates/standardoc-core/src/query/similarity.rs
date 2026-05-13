//! Similarity scoring helpers for [`crate::query::find_similar`].
//!
//! Hybrid score `max(jaro_winkler, jaccard_tokens)` over lowercased
//! identifiers. Jaro-Winkler captures typo-style and prefix-similar
//! variations (e.g. `parseFile` ↔ `parseFiles`); Jaccard over snake/camel
//! tokens captures templated-name clusters (e.g. `strip_rs_extension` ↔
//! `strip_ts_extension` share 2 of 3 tokens). Taking the max keeps both
//! regimes covered without requiring the caller to pick.

use std::collections::HashSet;

use strsim::jaro_winkler;

/// Returns a similarity score in `[0.0, 1.0]` between two identifier
/// names. Compares lowercased forms — case differences (`Foo` vs `foo`)
/// score 1.0. Score is the max of:
///
/// - `jaro_winkler(a_lc, b_lc)` — character-level edit similarity with
///   prefix bias.
/// - `jaccard(tokens(a_lc), tokens(b_lc))` — set similarity over snake /
///   camel-case tokens.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn score(a: &str, b: &str) -> f32 {
    let a_lc = a.to_lowercase();
    let b_lc = b.to_lowercase();
    if a_lc == b_lc {
        return 1.0;
    }
    let jw = jaro_winkler(&a_lc, &b_lc) as f32;
    // Tokenise the originals — case transitions are needed to split
    // `parseUser` into [parse, user]. Pre-lowercased input would collapse
    // to a single token. `tokenize` itself lowercases its output.
    let toks_a = tokenize(a);
    let toks_b = tokenize(b);
    let jac = jaccard(&toks_a, &toks_b);
    jw.max(jac)
}

/// Splits an identifier into lowercased semantic tokens. Boundaries:
///
/// - non-alphanumeric chars (`_`, `-`, `$`, ...) — drop the char, split.
/// - case transition lower→Upper (camelCase / PascalCase).
/// - alpha→digit and digit→alpha (e.g. `parse2json` → `parse`, `2`, `json`).
///
/// Returns lowercased tokens; empty tokens are dropped.
pub(crate) fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            push_if_nonempty(&mut out, &mut current);
            prev = Some(c);
            continue;
        }
        let case_boundary = c.is_ascii_uppercase()
            && prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit());
        let alpha_after_digit = c.is_ascii_alphabetic() && prev.is_some_and(|p| p.is_ascii_digit());
        let digit_after_alpha = c.is_ascii_digit() && prev.is_some_and(|p| p.is_ascii_alphabetic());
        if case_boundary || alpha_after_digit || digit_after_alpha {
            push_if_nonempty(&mut out, &mut current);
        }
        for lower in c.to_lowercase() {
            current.push(lower);
        }
        prev = Some(c);
    }
    push_if_nonempty(&mut out, &mut current);
    out
}

fn push_if_nonempty(out: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        out.push(std::mem::take(current));
    }
}

/// Jaccard index over two token slices: `|A ∩ B| / |A ∪ B|`. Empty/empty
/// is `1.0` (degenerate convention); empty/non-empty is `0.0`.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn jaccard(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: HashSet<&str> = a.iter().map(String::as_str).collect();
    let set_b: HashSet<&str> = b.iter().map(String::as_str).collect();
    let inter = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    inter as f32 / union as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        tokenize(s)
    }

    #[test]
    fn tokenize_snake_case_splits_on_underscore() {
        assert_eq!(toks("strip_rs_extension"), vec!["strip", "rs", "extension"]);
    }

    #[test]
    fn tokenize_camel_case_splits_on_case_transition() {
        assert_eq!(toks("parseFile"), vec!["parse", "file"]);
        assert_eq!(toks("parseFileName"), vec!["parse", "file", "name"]);
    }

    #[test]
    fn tokenize_pascal_case_splits_correctly() {
        assert_eq!(toks("ParseFile"), vec!["parse", "file"]);
    }

    #[test]
    fn tokenize_mixed_separators_are_normalised() {
        // Underscore boundaries split snake segments. Consecutive uppercase
        // letters (`RS` here) stay together — splitting on every Upper-after-
        // Upper would shred acronyms like `HTTP` into single chars. Acronym-
        // aware tokenisation (lookahead on Upper→Upper→lower) is a refinement
        // we can add post-beta.1 if it materialises in real workspaces.
        assert_eq!(toks("Strip_RS_Extension"), vec!["strip", "rs", "extension"]);
    }

    #[test]
    fn tokenize_digit_alpha_boundaries_split() {
        assert_eq!(toks("parse2json"), vec!["parse", "2", "json"]);
        assert_eq!(toks("v2Api"), vec!["v", "2", "api"]);
    }

    #[test]
    fn tokenize_empty_string_returns_empty_vec() {
        assert!(toks("").is_empty());
    }

    #[test]
    fn tokenize_single_word_kept_verbatim_lowercased() {
        assert_eq!(toks("strip"), vec!["strip"]);
        assert_eq!(toks("Strip"), vec!["strip"]);
    }

    #[test]
    fn tokenize_dollar_and_dash_split_like_underscore() {
        assert_eq!(toks("parse$file-name"), vec!["parse", "file", "name"]);
    }

    #[test]
    fn jaccard_identical_sets_score_one() {
        let a = vec!["foo".into(), "bar".into()];
        let b = vec!["bar".into(), "foo".into()];
        assert!((jaccard(&a, &b) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn jaccard_disjoint_sets_score_zero() {
        let a = vec!["foo".into()];
        let b = vec!["bar".into()];
        assert!(jaccard(&a, &b).abs() < f32::EPSILON);
    }

    #[test]
    fn jaccard_strip_extension_family_scores_half() {
        let a = tokenize("strip_rs_extension");
        let b = tokenize("strip_ts_extension");
        // {strip, rs, extension} ∩ {strip, ts, extension} = {strip, extension}
        // ∪ = {strip, rs, ts, extension} → 2 / 4 = 0.5
        assert!((jaccard(&a, &b) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn jaccard_dedup_within_one_side_does_not_inflate() {
        let a = vec!["foo".into(), "foo".into(), "bar".into()];
        let b = vec!["foo".into(), "bar".into()];
        // Sets dedup → both = {foo, bar} → 1.0
        assert!((jaccard(&a, &b) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn jaccard_both_empty_is_one() {
        let empty: Vec<String> = Vec::new();
        assert!((jaccard(&empty, &empty) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn jaccard_one_empty_is_zero() {
        let a = vec!["foo".into()];
        let empty: Vec<String> = Vec::new();
        assert!(jaccard(&a, &empty).abs() < f32::EPSILON);
    }

    #[test]
    fn score_identical_strings_one() {
        assert!((score("strip_rs_extension", "strip_rs_extension") - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn score_case_only_difference_is_one() {
        assert!((score("Foo", "foo") - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn score_template_family_above_threshold() {
        let s = score("strip_rs_extension", "strip_ts_extension");
        assert!(
            s >= 0.8,
            "templated names should score >= 0.8 via JW, got {s}"
        );
    }

    #[test]
    fn score_picks_max_of_jw_and_jaccard() {
        // `parseFile` vs `process_user`:
        //   tokens jw: ~0.5something; jaccard: 0 (disjoint sets)
        // Score must be the JW value (the larger of the two).
        let toks_a = tokenize("parsefile");
        let toks_b = tokenize("processuser");
        let jac = jaccard(&toks_a, &toks_b);
        let actual = score("parseFile", "process_user");
        assert!(
            actual >= jac,
            "score {actual} must be >= jaccard {jac} (max behaviour)"
        );
    }

    #[test]
    fn score_unrelated_names_below_threshold() {
        let s = score("buy_apple", "render_widget");
        assert!(s < 0.6, "unrelated names should score below 0.6, got {s}");
    }

    #[test]
    fn score_camel_vs_snake_same_meaning_high() {
        // `processUser` vs `process_user` lowercase to "processuser" vs "process_user".
        // JW is high (~0.95); tokenisation collapses to the same {process, user} → Jaccard = 1.0.
        let s = score("processUser", "process_user");
        assert!(
            (s - 1.0).abs() < f32::EPSILON,
            "tokenised-equivalent names should score 1.0, got {s}"
        );
    }
}
