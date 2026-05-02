pub(crate) fn compute(crate_name: &str, file_rel: &str) -> String {
    let after_src = strip_src_prefix(file_rel);
    let stem_path = strip_rs_extension(after_src);
    let segments: Vec<&str> = stem_path.split('/').filter(|s| !s.is_empty()).collect();
    let drop_root = segments
        .last()
        .is_some_and(|s| matches!(*s, "lib" | "main" | "mod"));

    let useful: &[&str] = if drop_root {
        &segments[..segments.len() - 1]
    } else {
        &segments[..]
    };

    if useful.is_empty() {
        return crate_name.to_string();
    }

    let mut out = String::with_capacity(crate_name.len() + useful.len() * 8);
    out.push_str(crate_name);
    for seg in useful {
        out.push_str("::");
        out.push_str(seg);
    }
    out
}

fn strip_src_prefix(file_rel: &str) -> &str {
    if let Some(idx) = file_rel.rfind("/src/") {
        return &file_rel[idx + "/src/".len()..];
    }
    if let Some(rest) = file_rel.strip_prefix("src/") {
        return rest;
    }
    file_rel
}

fn strip_rs_extension(p: &str) -> &str {
    p.strip_suffix(".rs").unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lib_rs_at_src_root_returns_crate_name() {
        assert_eq!(compute("foo", "src/lib.rs"), "foo");
        assert_eq!(compute("foo", "crates/foo/src/lib.rs"), "foo");
    }

    #[test]
    fn main_rs_at_src_root_returns_crate_name() {
        assert_eq!(compute("foo", "src/main.rs"), "foo");
        assert_eq!(compute("foo", "crates/foo/src/main.rs"), "foo");
    }

    #[test]
    fn flat_module_file() {
        assert_eq!(compute("foo", "src/bar.rs"), "foo::bar");
        assert_eq!(compute("foo", "crates/foo/src/bar.rs"), "foo::bar");
    }

    #[test]
    fn mod_rs_collapses_to_dir_name() {
        assert_eq!(compute("foo", "src/bar/mod.rs"), "foo::bar");
        assert_eq!(
            compute("foo", "crates/foo/src/bar/baz/mod.rs"),
            "foo::bar::baz"
        );
    }

    #[test]
    fn nested_module_file() {
        assert_eq!(compute("foo", "src/bar/baz.rs"), "foo::bar::baz");
        assert_eq!(compute("foo", "src/a/b/c.rs"), "foo::a::b::c");
    }

    #[test]
    fn no_src_prefix_falls_back_to_full_path() {
        assert_eq!(compute("foo", "weird/path.rs"), "foo::weird::path");
    }

    #[test]
    fn workspace_relative_with_dashes_in_crate_dir() {
        assert_eq!(
            compute("foo-bar", "crates/foo-bar/src/inner.rs"),
            "foo-bar::inner"
        );
    }
}
