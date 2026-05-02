use std::path::{Path, PathBuf};

pub(crate) fn find_cargo_toml(file_abs_path: &Path) -> Option<PathBuf> {
    let mut current = file_abs_path.parent()?;
    loop {
        let candidate = current.join("Cargo.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

pub(crate) fn parse_package_name(toml_content: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml_content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(section) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_package = section.trim() == "package";
            continue;
        }
        if in_package && let Some(value) = extract_name_value(trimmed) {
            return Some(value);
        }
    }
    None
}

fn extract_name_value(line: &str) -> Option<String> {
    let rest = line.strip_prefix("name")?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let bytes = rest.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let quote = bytes[0];
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let after_open = &rest[1..];
    let close = after_open.find(quote as char)?;
    Some(after_open[..close].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn find_cargo_toml_at_immediate_parent() {
        let dir = tempdir().unwrap();
        let cargo = dir.path().join("Cargo.toml");
        fs::write(&cargo, b"[package]\nname = \"foo\"\n").unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        let file = src.join("lib.rs");
        fs::write(&file, b"// empty").unwrap();
        assert_eq!(find_cargo_toml(&file), Some(cargo));
    }

    #[test]
    fn find_cargo_toml_ascends_multiple_levels() {
        let dir = tempdir().unwrap();
        let cargo = dir.path().join("Cargo.toml");
        fs::write(&cargo, b"[package]\nname = \"foo\"\n").unwrap();
        let nested = dir.path().join("src/a/b");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("c.rs");
        fs::write(&file, b"// empty").unwrap();
        assert_eq!(find_cargo_toml(&file), Some(cargo));
    }

    #[test]
    fn find_cargo_toml_returns_none_if_absent() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        let file = src.join("lib.rs");
        fs::write(&file, b"// empty").unwrap();
        assert_eq!(find_cargo_toml(&file), None);
    }

    #[test]
    fn parse_simple_package_name() {
        let toml = "\n[package]\nname = \"foo\"\nversion = \"0.1.0\"\n";
        assert_eq!(parse_package_name(toml), Some("foo".to_string()));
    }

    #[test]
    fn parse_workspace_only_returns_none() {
        let toml = "\n[workspace]\nmembers = [\"crates/*\"]\n";
        assert_eq!(parse_package_name(toml), None);
    }

    #[test]
    fn parse_ignores_name_in_other_sections() {
        let toml = "\n[dependencies]\nname = \"wrong\"\n\n[package]\nname = \"right\"\n";
        assert_eq!(parse_package_name(toml), Some("right".to_string()));
    }

    #[test]
    fn parse_handles_compact_assignment() {
        assert_eq!(
            parse_package_name("[package]\nname=\"foo\""),
            Some("foo".to_string())
        );
    }

    #[test]
    fn parse_handles_single_quotes() {
        assert_eq!(
            parse_package_name("[package]\nname = 'foo'\n"),
            Some("foo".to_string())
        );
    }

    #[test]
    fn parse_returns_none_on_unquoted_value() {
        assert_eq!(parse_package_name("[package]\nname = foo\n"), None);
    }

    #[test]
    fn parse_skips_comments() {
        let toml = "# top comment\n[package]\n# inner\nname = \"foo\"\n";
        assert_eq!(parse_package_name(toml), Some("foo".to_string()));
    }
}
