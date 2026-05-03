use std::path::{Path, PathBuf};

use full_moon::ast::{Expression, Stmt, Var};
use full_moon::tokenizer::{TokenReference, TokenType};

/// Compute the dotted module portion of an FQDN from a workspace-relative
/// path (relative to the package root). Strips the `.lua` extension and
/// collapses trailing `/init` to the parent directory (Lua module
/// convention: `require("foo")` resolves to either `foo.lua` or
/// `foo/init.lua`).
///
/// Examples (package_relative input → output):
/// * `"src/utils/strings.lua"` → `"src.utils.strings"`
/// * `"src/utils/init.lua"`    → `"src.utils"`
/// * `"init.lua"`              → `""`     (file lives at package root)
/// * `"main.lua"`              → `"main"`
pub(crate) fn compute_module_path(package_relative: &str) -> String {
    let stem = strip_lua_extension(package_relative);
    let stem = stem.strip_suffix("/init").unwrap_or(&stem);
    if stem == "init" {
        String::new()
    } else {
        stem.replace('/', ".")
    }
}

fn strip_lua_extension(path: &str) -> String {
    path.strip_suffix(".lua")
        .map_or_else(|| path.to_string(), str::to_string)
}

/// Walk up from a file's parent directory looking for a `*.rockspec` file.
/// Returns the absolute path of the rockspec when found; `None` otherwise.
/// The walk stops at `workspace_root` (inclusive).
pub(crate) fn find_rockspec(file_abs_path: &Path, workspace_root: &Path) -> Option<PathBuf> {
    let mut current = file_abs_path.parent()?;
    loop {
        if let Ok(entries) = std::fs::read_dir(current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("rockspec") && path.is_file()
                {
                    return Some(path);
                }
            }
        }
        if current == workspace_root {
            return None;
        }
        current = current.parent()?;
    }
}

/// Parse the `package = "..."` field from a rockspec file's contents.
///
/// Implementation: parse the rockspec as Lua via `full_moon` (we already
/// have the parser as a workspace dep) and walk top-level
/// `Stmt::Assignment` entries looking for `package = "<literal>"`.
///
/// Day-1 only handles literal string assignments. Forms like
/// `package = "foo-" .. version` or `package = local_name` return `None`
/// and the provider falls back to the workspace directory name. This is
/// consistent with `feedback_scope_graph_not_lsp.md` — we standardize at-
/// best, we don't reproduce a full Lua interpreter.
pub(crate) fn parse_rockspec_name(content: &str) -> Option<String> {
    let ast = full_moon::parse(content).ok()?;
    for stmt in ast.nodes().stmts() {
        let Stmt::Assignment(assign) = stmt else {
            continue;
        };
        let mut vars = assign.variables().iter();
        let mut exprs = assign.expressions().iter();
        if let (Some(Var::Name(var_name)), Some(expr)) = (vars.next(), exprs.next())
            && ident_text(var_name) == "package"
            && let Some(literal) = string_literal_text(expr)
        {
            return Some(literal);
        }
    }
    None
}

/// Workspace-directory-name fallback when no rockspec is present. Returns
/// the last path component of `workspace_root` as a String, lossy-converted.
pub(crate) fn workspace_dir_name(workspace_root: &Path) -> String {
    workspace_root
        .file_name()
        .map_or_else(|| "workspace".into(), |s| s.to_string_lossy().into_owned())
}

/// Extract the identifier text from a `TokenReference`, or empty string if
/// it isn't an identifier. Trivia (whitespace / comments) is stripped — we
/// want the bare name only.
pub(crate) fn ident_text(token: &TokenReference) -> &str {
    match token.token_type() {
        TokenType::Identifier { identifier } => identifier,
        _ => "",
    }
}

/// Extract the literal text from an `Expression::String` node, stripped of
/// surrounding quotes. Returns `None` for non-string expressions or for
/// composite forms (concatenation, interpolation, identifier reference).
pub(crate) fn string_literal_text(expr: &Expression) -> Option<String> {
    let Expression::String(token) = expr else {
        return None;
    };
    let TokenType::StringLiteral { literal, .. } = token.token_type() else {
        return None;
    };
    Some(literal.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_module_path_strips_lua_extension() {
        assert_eq!(
            compute_module_path("src/utils/strings.lua"),
            "src.utils.strings"
        );
        assert_eq!(compute_module_path("main.lua"), "main");
    }

    #[test]
    fn compute_module_path_collapses_init() {
        assert_eq!(compute_module_path("src/utils/init.lua"), "src.utils");
    }

    #[test]
    fn compute_module_path_root_init_is_empty() {
        assert_eq!(compute_module_path("init.lua"), "");
    }

    #[test]
    fn compute_module_path_passthrough_for_non_lua() {
        // defensive — provider should never hand non-.lua here, but the
        // helper must not panic.
        assert_eq!(compute_module_path("README"), "README");
    }

    #[test]
    fn workspace_dir_name_extracts_last_component() {
        assert_eq!(workspace_dir_name(Path::new("/tmp/myproj")), "myproj");
    }

    #[test]
    fn workspace_dir_name_falls_back_when_root() {
        // Root paths have no file_name on Unix-style; the fallback must
        // not panic.
        let _ = workspace_dir_name(Path::new("/"));
    }

    #[test]
    fn parse_rockspec_name_extracts_simple_literal() {
        let r = "package = \"mylib\"\nversion = \"1.0-1\"\n";
        assert_eq!(parse_rockspec_name(r).as_deref(), Some("mylib"));
    }

    #[test]
    fn parse_rockspec_name_extracts_when_other_fields_present() {
        let r = "rockspec_format = \"3.0\"\npackage = \"alpha\"\nversion = \"2.0-1\"\n";
        assert_eq!(parse_rockspec_name(r).as_deref(), Some("alpha"));
    }

    #[test]
    fn parse_rockspec_name_returns_none_for_concatenation() {
        let r = "version = \"1.0\"\npackage = \"foo-\" .. version\n";
        assert_eq!(parse_rockspec_name(r), None);
    }

    #[test]
    fn parse_rockspec_name_returns_none_for_identifier_value() {
        let r = "local n = \"foo\"\npackage = n\n";
        assert_eq!(parse_rockspec_name(r), None);
    }

    #[test]
    fn parse_rockspec_name_returns_none_when_field_missing() {
        let r = "version = \"1.0\"\nsource = { url = \"git://\" }\n";
        assert_eq!(parse_rockspec_name(r), None);
    }

    #[test]
    fn parse_rockspec_name_returns_none_for_invalid_lua() {
        assert_eq!(parse_rockspec_name("package = ").as_deref(), None);
    }
}
