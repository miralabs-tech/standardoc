use std::ffi::OsStr;
use std::path::Path;

use standardoc_ir::Language;

pub(crate) const SUPPORTED_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "lua", "vue", "svelte", "c", "h",
];

pub(crate) fn has_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e))
}

pub(crate) fn to_workspace_relative(abs_path: &Path, workspace_root: &Path) -> Option<String> {
    let rel = abs_path.strip_prefix(workspace_root).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.is_empty() { None } else { Some(s) }
}

pub(crate) fn guess_language(rel_path: &str) -> Option<Language> {
    let (_, ext) = rel_path.rsplit_once('.')?;
    match ext {
        "rs" => Some(Language::Rust),
        "ts" | "tsx" => Some(Language::TypeScript),
        "js" | "jsx" => Some(Language::JavaScript),
        "lua" => Some(Language::Lua),
        "vue" => Some(Language::Vue),
        "svelte" => Some(Language::Svelte),
        // `.h` routes to C, matching the provider dispatch — there is no
        // separate C++ extractor, so headers are treated as C.
        "c" | "h" => Some(Language::C),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn has_supported_extension_filters_extensions() {
        assert!(has_supported_extension(Path::new("a.rs")));
        assert!(has_supported_extension(Path::new("a.ts")));
        assert!(has_supported_extension(Path::new("a.tsx")));
        assert!(has_supported_extension(Path::new("a.js")));
        assert!(has_supported_extension(Path::new("a.jsx")));
        assert!(has_supported_extension(Path::new("a.lua")));
        assert!(has_supported_extension(Path::new("App.vue")));
        assert!(has_supported_extension(Path::new("Counter.svelte")));
        assert!(has_supported_extension(Path::new("vm.c")));
        assert!(has_supported_extension(Path::new("lur.h")));
        assert!(!has_supported_extension(Path::new("a.py")));
        assert!(!has_supported_extension(Path::new("Makefile")));
    }

    #[test]
    fn to_workspace_relative_strips_prefix() {
        let root = PathBuf::from("/ws");
        let abs = PathBuf::from("/ws/src/lib.rs");
        assert_eq!(
            to_workspace_relative(&abs, &root),
            Some("src/lib.rs".into())
        );
    }

    #[test]
    fn to_workspace_relative_outside_root_is_none() {
        let root = PathBuf::from("/ws");
        let abs = PathBuf::from("/other/lib.rs");
        assert_eq!(to_workspace_relative(&abs, &root), None);
    }

    #[test]
    fn to_workspace_relative_root_itself_is_none() {
        let root = PathBuf::from("/ws");
        assert_eq!(to_workspace_relative(&root, &root), None);
    }

    #[test]
    fn guess_language_maps_known_extensions() {
        assert_eq!(guess_language("a.rs"), Some(Language::Rust));
        assert_eq!(guess_language("a.ts"), Some(Language::TypeScript));
        assert_eq!(guess_language("a.tsx"), Some(Language::TypeScript));
        assert_eq!(guess_language("a.js"), Some(Language::JavaScript));
        assert_eq!(guess_language("a.jsx"), Some(Language::JavaScript));
        assert_eq!(guess_language("a.lua"), Some(Language::Lua));
        assert_eq!(guess_language("App.vue"), Some(Language::Vue));
        assert_eq!(guess_language("Counter.svelte"), Some(Language::Svelte));
        assert_eq!(guess_language("runtime/vm.c"), Some(Language::C));
        assert_eq!(guess_language("include/lur.h"), Some(Language::C));
        assert_eq!(guess_language("a.py"), None);
        assert_eq!(guess_language("README"), None);
    }
}
