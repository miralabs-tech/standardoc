//! Tiny text utilities shared by the Vue and Svelte template parsers.
//! Both originally carried a near-identical `find_keyword` /
//! `find_top_level_keyword` (balanced-paren / brace / bracket / quote
//! aware substring lookup) — folded into a single helper here.

/// Returns `(byte_pos, kw.len())` of the first occurrence of `kw` in `s`
/// that lives at the top level — i.e. NOT inside a parenthesised /
/// bracketed / braced group, and NOT inside a single, double or backtick
/// string literal.
///
/// Used by Vue's `v-for="item in items"` splitter (`" in "` / `" of "`)
/// and by Svelte's `{#each ... as ...}` splitter (`" as "`,
/// `" then "`, `" catch "`).
pub(crate) fn find_top_level_keyword(s: &str, kw: &str) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let kw_bytes = kw.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut depth_brace = 0i32;
    let mut in_quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_quote {
            if b == q {
                in_quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' => {
                in_quote = Some(b);
                i += 1;
                continue;
            }
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            b'{' => depth_brace += 1,
            b'}' => depth_brace -= 1,
            _ => {}
        }
        if depth_paren == 0
            && depth_bracket == 0
            && depth_brace == 0
            && i + kw_bytes.len() <= bytes.len()
            && &bytes[i..i + kw_bytes.len()] == kw_bytes
        {
            return Some((i, kw_bytes.len()));
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_simple_keyword() {
        assert_eq!(find_top_level_keyword("a in b", " in "), Some((1, 4)));
    }

    #[test]
    fn skips_keyword_inside_parens() {
        // `in` inside `(item, idx)` is NOT a top-level match — the real
        // top-level ` in ` is the one outside the parens.
        let (pos, len) = find_top_level_keyword("(item in inner) in items", " in ").unwrap();
        // The match must be the OUTER `" in "` — confirm by checking
        // the bytes that follow it spell `items`.
        let after = &"(item in inner) in items"[pos + len..];
        assert_eq!(after, "items");
    }

    #[test]
    fn skips_keyword_inside_braces() {
        let (pos, len) = find_top_level_keyword("{a in b} in items", " in ").unwrap();
        let after = &"{a in b} in items"[pos + len..];
        assert_eq!(after, "items");
    }

    #[test]
    fn skips_keyword_inside_string_literal() {
        let (pos, len) = find_top_level_keyword("\"a in b\" in items", " in ").unwrap();
        let after = &"\"a in b\" in items"[pos + len..];
        assert_eq!(after, "items");
    }

    #[test]
    fn returns_none_when_no_top_level_match() {
        assert_eq!(find_top_level_keyword("(in)", " in "), None);
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(find_top_level_keyword("", " in "), None);
    }

    #[test]
    fn keyword_longer_than_input_returns_none() {
        assert_eq!(find_top_level_keyword("a", " in "), None);
    }

    #[test]
    fn matches_as_keyword_for_svelte_each_clause() {
        let (pos, len) = find_top_level_keyword("items as item", " as ").unwrap();
        let after = &"items as item"[pos + len..];
        assert_eq!(after, "item");
    }
}
