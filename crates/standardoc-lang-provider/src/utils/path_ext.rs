//! Generic file-extension stripping. Replaces three near-duplicate
//! per-language helpers (`rust::module_path::strip_rs_extension`,
//! `ts::helpers::strip_ts_extension`, `lua::helpers::strip_lua_extension`)
//! with a single function that takes the candidate extensions list.
//!
//! The order of the `exts` slice matters when extensions overlap — pass
//! `.d.ts` before `.ts` so a declaration file isn't truncated to `.d`.
//! Each entry must include the leading `.`.

/// Returns `path` with the first matching suffix from `exts` removed.
/// When no extension matches, the original `path` is returned unchanged.
///
/// Borrows the input — callers stay zero-alloc when the extension list
/// trims a known suffix.
pub(crate) fn strip_extension<'a>(path: &'a str, exts: &[&str]) -> &'a str {
    for ext in exts {
        if let Some(stem) = path.strip_suffix(ext) {
            return stem;
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_known_extension() {
        assert_eq!(strip_extension("src/lib.rs", &[".rs"]), "src/lib");
    }

    #[test]
    fn returns_input_when_no_extension_matches() {
        assert_eq!(strip_extension("Makefile", &[".rs"]), "Makefile");
    }

    #[test]
    fn first_match_wins_when_multiple_extensions_listed() {
        // `.d.ts` declared before `.ts` — declaration files keep their
        // double-extension semantics.
        assert_eq!(strip_extension("foo.d.ts", &[".d.ts", ".ts"]), "foo");
    }

    #[test]
    fn order_matters_d_ts_before_ts_otherwise_truncated_to_d() {
        // Wrong order → truncates the `.ts` and leaves a stray `.d` —
        // documents WHY the order matters.
        assert_eq!(strip_extension("foo.d.ts", &[".ts", ".d.ts"]), "foo.d");
    }

    #[test]
    fn empty_extension_list_is_noop() {
        assert_eq!(strip_extension("anything.lua", &[]), "anything.lua");
    }

    #[test]
    fn handles_lua_style_single_extension() {
        assert_eq!(strip_extension("init.lua", &[".lua"]), "init");
    }

    #[test]
    fn handles_vue_style_double_dot_extension() {
        assert_eq!(strip_extension("App.vue", &[".vue"]), "App");
    }
}
