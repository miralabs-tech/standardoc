use std::path::Path;

use tower_lsp_server::ls_types::Uri;

/// Convert a `file://` LSP URI to a workspace-relative path string.
///
/// Returns `None` when the URI scheme is not `file://`, when the resolved
/// absolute path cannot be canonicalised, or when the canonical path is
/// outside `workspace_root`. The returned string is forward-slash
/// normalised (SCHEMA §2.3) and ready to be passed to `query::*` /
/// `IndexHandle::*` consumers that expect a stored `files.path`.
pub(crate) fn uri_to_workspace_path(uri: &Uri, workspace_root: &Path) -> Option<String> {
    if !uri.scheme().as_str().eq_ignore_ascii_case("file") {
        return None;
    }
    let abs = uri.to_file_path()?;
    let canonical = abs.canonicalize().ok()?;
    let rel = canonical.strip_prefix(workspace_root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Inverse of [`uri_to_workspace_path`]: turn a workspace-relative path
/// (forward-slash, e.g. `src/main.rs`) into the `file://` URI the LSP
/// client expects in `Location` / `TextDocumentIdentifier` payloads.
///
/// Returns `None` if the resulting URI cannot be encoded — should never
/// happen for paths produced by the indexer (canonicalised, valid UTF-8).
pub(crate) fn workspace_path_to_uri(rel: &str, workspace_root: &Path) -> Option<Uri> {
    let absolute = workspace_root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    Uri::from_file_path(absolute)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn tmp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn workspace_path_to_uri_then_back_round_trips() {
        let dir = tmp_root();
        let root = dir.path().canonicalize().unwrap();
        let rel = "src/main.rs";
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        let uri = workspace_path_to_uri(rel, &root).expect("encode uri");
        let back = uri_to_workspace_path(&uri, &root).expect("decode uri");
        assert_eq!(back, "src/main.rs");
    }

    #[test]
    fn uri_to_workspace_path_returns_none_for_non_file_scheme() {
        let dir = tmp_root();
        let root = dir.path().canonicalize().unwrap();
        let uri = Uri::from_str("https://example.com/foo").unwrap();
        assert_eq!(uri_to_workspace_path(&uri, &root), None);
    }

    #[test]
    fn uri_to_workspace_path_returns_none_when_outside_workspace() {
        let dir = tmp_root();
        let root = dir.path().canonicalize().unwrap();

        let other = tmp_root();
        let other_root = other.path().canonicalize().unwrap();
        let outside = other_root.join("foreign.rs");
        std::fs::write(&outside, "fn x() {}").unwrap();

        let uri = Uri::from_file_path(&outside).expect("encode outside uri");
        assert_eq!(uri_to_workspace_path(&uri, &root), None);
    }

    #[test]
    fn workspace_path_to_uri_handles_nested_paths_with_forward_slashes() {
        let dir = tmp_root();
        let root = dir.path().canonicalize().unwrap();
        let nested = "deep/nested/file.rs";
        std::fs::create_dir_all(root.join("deep/nested")).unwrap();
        std::fs::write(root.join(nested), "").unwrap();

        let uri = workspace_path_to_uri(nested, &root).expect("encode nested uri");
        let back = uri_to_workspace_path(&uri, &root).expect("decode nested uri");
        assert_eq!(back, "deep/nested/file.rs");
    }
}
