use std::path::{Path, PathBuf};

use standardoc_ir::Visibility;
use swc_core::common::Loc;

use crate::walk_core::LanguagePathConventions;

/// UTF-16 column (0-indexed) for a swc `Loc`. swc's `col` (`CharPos`) is
/// already counted in UTF-16 code units — its multibyte adjustment maps
/// 1/2/3 UTF-8 bytes to one unit and a 4-byte (astral) char to two (see
/// `MultiByteChar::byte_to_char_diff`). That is exactly what the LSP /
/// VSCode `Position.character` field expects. The previously stamped
/// `col_display` is tab-expanded (display width) and therefore wrong the
/// moment a tab precedes the symbol, which is why we read `col` instead.
pub(crate) fn loc_utf16_col(loc: &Loc) -> u32 {
    u32::try_from(loc.col.0).unwrap_or(u32::MAX)
}

/// Order matters: `.d.ts` must be tried before `.ts`, etc. Lock 41 §1
/// Q9 added `.vue` and `.svelte` so SFC files compute the same module
/// path their script content would have under a plain TS file.
pub(crate) const TS_CONVENTIONS: LanguagePathConventions = LanguagePathConventions {
    extensions: &[
        ".d.ts", ".d.tsx", ".d.mts", ".d.cts", ".tsx", ".ts", ".jsx", ".js", ".mts", ".cts",
        ".mjs", ".cjs", ".vue", ".svelte",
    ],
    root_aliases: &["index"],
    strip_src_prefix: false,
};

/// Map a TS/JS access modifier to canonical IR visibility.
///
/// `public` / absent (top-level export) → `Public`.
/// `private` / module-internal (no `export` keyword) → `Private`.
/// `protected` (class members) → `Protected`.
/// `crate` is unused — TS has no equivalent of Rust crate visibility.
pub(crate) fn map_access_modifier(raw: Option<&str>, exported: bool) -> Visibility {
    match raw {
        Some("public") => Visibility::Public,
        Some("protected") => Visibility::Protected,
        None if exported => Visibility::Public,
        Some(_) | None => Visibility::Private,
    }
}

/// Compute the module portion of an FQDN from a workspace-relative path
/// (relative to the package root). Strips the file extension and collapses
/// trailing `/index` to the parent directory (mirror of Rust `mod.rs`).
///
/// Examples (package_relative input → output):
/// * `"src/auth/login.ts"`   → `"src::auth::login"`
/// * `"src/auth/index.ts"`   → `"src::auth"`
/// * `"src/index.ts"`        → `"src"`
/// * `"index.ts"`            → `""`  (file lives at package root)
pub(crate) fn compute_module_path(package_relative: &str) -> String {
    crate::walk_core::compute_module_path(&TS_CONVENTIONS, package_relative)
}

/// Walk filesystem ancestors from `file_abs_path` until a `package.json` is
/// found. Returns the canonical path to the closest `package.json`, or `None`
/// when the file lives outside any package (workspace-root fallback handled
/// by the caller).
pub(crate) fn find_package_json(file_abs_path: &Path) -> Option<PathBuf> {
    let mut current = file_abs_path.parent()?;
    loop {
        let candidate = current.join("package.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

/// Parse a `package.json` blob and return the `name` field, if present.
/// Returns `None` for workspace-root `package.json` files that have only
/// `workspaces = [...]` and no `name`.
pub(crate) fn parse_package_name(json_content: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json_content).ok()?;
    value.get("name")?.as_str().map(str::to_string)
}

/// Static property-name extraction (`PropName` → `Option<String>`). Returns
/// `Some(name)` for identifier, string, and numeric keys (numbers
/// stringified); `None` for computed (`[expr]: ...`) and bigint keys. Shared
/// by the symbol walker (method / property names) and the FFI tagger
/// (object-literal keys).
pub(crate) fn prop_name_static(key: &swc_core::ecma::ast::PropName) -> Option<String> {
    use swc_core::ecma::ast::PropName;
    match key {
        PropName::Ident(i) => Some(i.sym.to_string()),
        PropName::Str(s) => Some(s.value.to_string_lossy().into_owned()),
        PropName::Num(n) => Some(n.value.to_string()),
        PropName::Computed(_) | PropName::BigInt(_) => None,
    }
}

/// Bug C-1 — textual name of a `TsEnumMember.id`. swc parses both
/// `Ident(Foo)` and `Str("Foo")` flavors (TS allows `enum E { "foo bar" = 1 }`).
pub(crate) fn ts_enum_member_id_name(id: &swc_core::ecma::ast::TsEnumMemberId) -> String {
    use swc_core::ecma::ast::TsEnumMemberId;
    match id {
        TsEnumMemberId::Ident(i) => i.sym.to_string(),
        TsEnumMemberId::Str(s) => s.value.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn map_access_modifier_public_explicit() {
        assert_eq!(
            map_access_modifier(Some("public"), false),
            Visibility::Public
        );
    }

    #[test]
    fn map_access_modifier_private_explicit() {
        assert_eq!(
            map_access_modifier(Some("private"), true),
            Visibility::Private
        );
    }

    #[test]
    fn map_access_modifier_protected_explicit() {
        assert_eq!(
            map_access_modifier(Some("protected"), false),
            Visibility::Protected
        );
    }

    #[test]
    fn map_access_modifier_no_keyword_exported_is_public() {
        assert_eq!(map_access_modifier(None, true), Visibility::Public);
    }

    #[test]
    fn map_access_modifier_no_keyword_not_exported_is_private() {
        assert_eq!(map_access_modifier(None, false), Visibility::Private);
    }

    #[test]
    fn compute_module_path_flat_file() {
        assert_eq!(compute_module_path("src/auth/login.ts"), "src::auth::login");
    }

    #[test]
    fn compute_module_path_index_collapses_to_parent() {
        assert_eq!(compute_module_path("src/auth/index.ts"), "src::auth");
    }

    #[test]
    fn compute_module_path_root_index_collapses_to_src() {
        assert_eq!(compute_module_path("src/index.ts"), "src");
    }

    #[test]
    fn compute_module_path_top_level_index_is_empty() {
        assert_eq!(compute_module_path("index.ts"), "");
    }

    #[test]
    fn compute_module_path_handles_tsx_extension() {
        assert_eq!(compute_module_path("src/App.tsx"), "src::App");
    }

    #[test]
    fn compute_module_path_handles_jsx_extension() {
        assert_eq!(compute_module_path("src/legacy.jsx"), "src::legacy");
    }

    #[test]
    fn compute_module_path_handles_js_extension() {
        assert_eq!(compute_module_path("scripts/build.js"), "scripts::build");
    }

    #[test]
    fn compute_module_path_handles_d_ts_declaration() {
        assert_eq!(compute_module_path("dist/index.d.ts"), "dist");
        assert_eq!(compute_module_path("types/api.d.ts"), "types::api");
    }

    #[test]
    fn compute_module_path_handles_mts_cts_mjs_cjs() {
        assert_eq!(compute_module_path("src/a.mts"), "src::a");
        assert_eq!(compute_module_path("src/b.cts"), "src::b");
        assert_eq!(compute_module_path("src/c.mjs"), "src::c");
        assert_eq!(compute_module_path("src/d.cjs"), "src::d");
    }

    #[test]
    fn compute_module_path_no_extension_kept_as_segments() {
        assert_eq!(compute_module_path("src/no-ext"), "src::no-ext");
    }

    #[test]
    fn compute_module_path_empty_input_is_empty() {
        assert_eq!(compute_module_path(""), "");
    }

    #[test]
    fn find_package_json_at_immediate_parent() {
        let dir = tempdir().unwrap();
        let pkg = dir.path().join("package.json");
        fs::write(&pkg, b"{\"name\": \"foo\"}").unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        let file = src.join("index.ts");
        fs::write(&file, b"// empty").unwrap();
        assert_eq!(find_package_json(&file), Some(pkg));
    }

    #[test]
    fn find_package_json_ascends_multiple_levels() {
        let dir = tempdir().unwrap();
        let pkg = dir.path().join("package.json");
        fs::write(&pkg, b"{\"name\": \"foo\"}").unwrap();
        let nested = dir.path().join("src/a/b");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("c.ts");
        fs::write(&file, b"// empty").unwrap();
        assert_eq!(find_package_json(&file), Some(pkg));
    }

    #[test]
    fn find_package_json_returns_none_if_absent() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        let file = src.join("index.ts");
        fs::write(&file, b"// empty").unwrap();
        assert_eq!(find_package_json(&file), None);
    }

    #[test]
    fn parse_package_name_simple() {
        let json = r#"{"name": "@myorg/api", "version": "0.1.0"}"#;
        assert_eq!(parse_package_name(json), Some("@myorg/api".to_string()));
    }

    #[test]
    fn parse_package_name_workspace_root_no_name() {
        let json = r#"{"private": true, "workspaces": ["packages/*"]}"#;
        assert_eq!(parse_package_name(json), None);
    }

    #[test]
    fn parse_package_name_invalid_json_returns_none() {
        assert_eq!(parse_package_name("not json"), None);
    }

    #[test]
    fn parse_package_name_name_not_a_string_returns_none() {
        let json = r#"{"name": 42}"#;
        assert_eq!(parse_package_name(json), None);
    }

    #[test]
    fn parse_package_name_with_extra_fields() {
        let json = r#"
        {
          "name": "foo",
          "version": "1.2.3",
          "main": "dist/index.js",
          "types": "dist/index.d.ts"
        }
        "#;
        assert_eq!(parse_package_name(json), Some("foo".to_string()));
    }
}
