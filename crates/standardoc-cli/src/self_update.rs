//! `standardoc self-update` — atomic in-place binary upgrade.
//!
//! Pulls the latest release manifest from GitHub Releases, picks the
//! archive for the current platform, verifies its sha256 against the
//! manifest, extracts it via the system `tar` tool (handles both
//! `.tar.gz` and `.zip` since Windows 10 1809 ships bsdtar), and
//! atomically swaps the running binary with the new one.
//!
//! Atomic swap strategy is platform-specific:
//!
//!   * **Unix** — `rename(new, current)` is atomic ; the old inode
//!     keeps serving the running process, the new inode shows up on
//!     the next exec.
//!   * **Windows** — `rename` on a running `.exe` is legal but
//!     `remove` is not. We rename `standardoc.exe` to
//!     `standardoc.exe.old` (legal even while running), copy the new
//!     binary in, and leave the `.old` for the next invocation to
//!     clean up. A previous `.old` is removed best-effort at the
//!     start of every self-update run.
//!
//! Shares the same `version.json` schema the VSCode extension's
//! `binary-installer.ts` consumes — there is one source of truth for
//! "where does the standardoc binary come from".

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const RELEASE_REPO: &str = "miralabs-tech/standardoc";

/// Default manifest URL. Uses GitHub's `/releases/latest/download/<asset>`
/// alias which auto-redirects to the most recent non-prerelease tag —
/// `standardoc self-update` always pulls the latest stable without
/// needing a separate API call to discover the tag.
fn default_manifest_url() -> String {
    format!("https://github.com/{RELEASE_REPO}/releases/latest/download/version.json")
}

/// Manifest URL for a specific version tag, used by `--version v1.2.3`
/// to pin the update to a specific release (useful for rollbacks).
fn manifest_url_for_version(tag: &str) -> String {
    format!("https://github.com/{RELEASE_REPO}/releases/download/{tag}/version.json")
}

/// Target triple this binary was built for. Resolved at compile time
/// via `cfg!` so an unsupported host doesn't compile in the first
/// place — no runtime "what am I" dance.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const TARGET_TRIPLE: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const TARGET_TRIPLE: &str = "aarch64-unknown-linux-gnu";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const TARGET_TRIPLE: &str = "x86_64-apple-darwin";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const TARGET_TRIPLE: &str = "aarch64-apple-darwin";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";

/// File extension of the published release archive for this target.
#[cfg(target_os = "windows")]
const ARCHIVE_EXT: &str = "zip";
#[cfg(not(target_os = "windows"))]
const ARCHIVE_EXT: &str = "tar.gz";

/// Filename of the binary inside the extracted archive.
#[cfg(target_os = "windows")]
const EXE_NAME: &str = "standardoc.exe";
#[cfg(not(target_os = "windows"))]
const EXE_NAME: &str = "standardoc";

/// Manifest published alongside each release — same shape consumed by
/// the VSCode extension's binary installer. `binaries` maps target
/// triple → archive URL ; `checksums_sha256` maps target triple →
/// expected hash. Other fields are accepted but ignored here.
#[derive(Debug, Deserialize)]
struct VersionManifest {
    core_version: String,
    binaries: std::collections::HashMap<String, String>,
    checksums_sha256: std::collections::HashMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SelfUpdateError {
    #[error("unsupported host platform (target {os} / {arch})")]
    UnsupportedPlatform {
        os: &'static str,
        arch: &'static str,
    },
    #[error("network: {0}")]
    Network(String),
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("no asset published for target `{0}` in the latest release")]
    NoAssetForTarget(String),
    #[error("sha256 mismatch on downloaded archive: expected {expected}, got {got}")]
    Sha256Mismatch { expected: String, got: String },
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("extracted archive did not contain `{0}`")]
    MissingExtractedBinary(String),
}

/// Run a self-update. Returns the version the binary was upgraded to,
/// or `None` when already on the latest version (and `force` was not
/// set). `dry_run` resolves and prints what would happen, then
/// returns without touching the filesystem.
pub(crate) async fn run(
    dry_run: bool,
    force: bool,
    version: Option<String>,
) -> Result<Option<String>, SelfUpdateError> {
    let current_exe = env::current_exe().map_err(SelfUpdateError::Io)?;
    let current_version = env!("CARGO_PKG_VERSION");

    let target = TARGET_TRIPLE;
    if target.is_empty() {
        return Err(SelfUpdateError::UnsupportedPlatform {
            os: env::consts::OS,
            arch: env::consts::ARCH,
        });
    }

    let manifest_url = match version.as_deref() {
        Some(tag) => manifest_url_for_version(tag),
        None => default_manifest_url(),
    };

    eprintln!("self-update: current = {current_version}");
    eprintln!("self-update: target  = {target}");
    eprintln!("self-update: manifest = {manifest_url}");

    let manifest = fetch_manifest(&manifest_url).await?;
    eprintln!("self-update: latest  = {}", manifest.core_version);

    if !force && manifest.core_version == current_version {
        eprintln!("self-update: already on the latest version, nothing to do.");
        return Ok(None);
    }

    let asset = pick_platform_asset(&manifest, target)?;
    eprintln!("self-update: asset   = {}", asset.url);
    eprintln!("self-update: sha256  = {}", &asset.sha256[..16]);

    if dry_run {
        eprintln!("self-update: dry-run, stopping before download.");
        return Ok(Some(manifest.core_version));
    }

    let archive_bytes = fetch_bytes(&asset.url).await?;
    verify_sha256(&archive_bytes, &asset.sha256)?;

    let tmp = tempfile::tempdir()?;
    let archive_path = tmp.path().join(format!("standardoc-update.{ARCHIVE_EXT}"));
    fs::write(&archive_path, &archive_bytes)?;
    extract_with_tar(&archive_path, tmp.path())?;

    let inner_dir = format!("standardoc-v{}-{target}", manifest.core_version);
    let new_exe = tmp.path().join(&inner_dir).join(EXE_NAME);
    if !new_exe.exists() {
        return Err(SelfUpdateError::MissingExtractedBinary(format!(
            "{inner_dir}/{EXE_NAME}"
        )));
    }

    atomic_swap(&new_exe, &current_exe)?;
    eprintln!(
        "self-update: installed {} → {}",
        manifest.core_version,
        current_exe.display()
    );
    Ok(Some(manifest.core_version))
}

#[derive(Debug)]
struct Asset {
    url: String,
    sha256: String,
}

fn pick_platform_asset(manifest: &VersionManifest, target: &str) -> Result<Asset, SelfUpdateError> {
    let url = manifest
        .binaries
        .get(target)
        .ok_or_else(|| SelfUpdateError::NoAssetForTarget(target.to_string()))?
        .clone();
    let sha256 = manifest
        .checksums_sha256
        .get(target)
        .ok_or_else(|| SelfUpdateError::NoAssetForTarget(target.to_string()))?
        .to_lowercase();
    Ok(Asset { url, sha256 })
}

async fn fetch_manifest(url: &str) -> Result<VersionManifest, SelfUpdateError> {
    let body = fetch_text(url).await?;
    serde_json::from_str(&body).map_err(|e| SelfUpdateError::Manifest(e.to_string()))
}

async fn fetch_text(url: &str) -> Result<String, SelfUpdateError> {
    let client = http_client()?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| SelfUpdateError::Network(format!("GET {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(SelfUpdateError::Network(format!(
            "GET {url}: HTTP {}",
            resp.status()
        )));
    }
    resp.text()
        .await
        .map_err(|e| SelfUpdateError::Network(format!("read body {url}: {e}")))
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, SelfUpdateError> {
    let client = http_client()?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| SelfUpdateError::Network(format!("GET {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(SelfUpdateError::Network(format!(
            "GET {url}: HTTP {}",
            resp.status()
        )));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| SelfUpdateError::Network(format!("read body {url}: {e}")))
}

fn http_client() -> Result<reqwest::Client, SelfUpdateError> {
    reqwest::Client::builder()
        .user_agent(concat!(
            "standardoc-self-update/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|e| SelfUpdateError::Network(format!("build client: {e}")))
}

fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), SelfUpdateError> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let got = hex_lower(&hasher.finalize());
    if got != expected_hex.to_lowercase() {
        return Err(SelfUpdateError::Sha256Mismatch {
            expected: expected_hex.to_lowercase(),
            got,
        });
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn extract_with_tar(archive: &Path, dest: &Path) -> Result<(), SelfUpdateError> {
    // `tar -xf <archive> -C <dest>` handles both .tar.gz and .zip on
    // every supported platform — same approach the TS installer uses,
    // so the system-tar requirement is already implicit.
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .map_err(|e| SelfUpdateError::Io(io::Error::other(format!("spawn tar: {e}"))))?;
    if !status.success() {
        return Err(SelfUpdateError::Io(io::Error::other(format!(
            "tar exited with status {status}"
        ))));
    }
    Ok(())
}

/// Atomically replace `current` with `new`. Platform-specific:
///
///   * Unix : `rename(new, current)` — atomic, the running process
///     keeps its inode, next exec gets the new binary.
///   * Windows : rename current to `<current>.old` (legal on a
///     running `.exe`, unlike delete), then copy new into place.
///     `.old` is cleaned up best-effort at the start of every
///     subsequent run.
fn atomic_swap(new: &Path, current: &Path) -> Result<(), SelfUpdateError> {
    #[cfg(windows)]
    {
        let old_path = old_sidecar(current);
        // Sweep a leftover .old from a previous run, ignore failures
        // (it might still be locked if a sibling standardoc child is
        // somehow holding it — harmless, we'll try again next time).
        let _ = fs::remove_file(&old_path);
        fs::rename(current, &old_path).map_err(SelfUpdateError::Io)?;
        if let Err(e) = fs::copy(new, current) {
            // Try to restore the old binary if the copy failed midway
            // so we don't leave the user without a `standardoc.exe`.
            let _ = fs::rename(&old_path, current);
            return Err(SelfUpdateError::Io(e));
        }
        // `.old` will get cleaned up on the next self-update call.
        Ok(())
    }
    #[cfg(not(windows))]
    {
        // POSIX rename across the same filesystem is atomic. The new
        // file may need exec bits — `tar -xf` preserves them, so this
        // is usually a no-op, but be defensive in case the archive
        // was produced without them.
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(new)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(new, perms)?;
        fs::rename(new, current).map_err(SelfUpdateError::Io)
    }
}

/// `<current>.old` next to the current binary — Windows-only sweep target.
#[cfg(windows)]
fn old_sidecar(current: &Path) -> PathBuf {
    let mut s = current.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

// Suppress dead-code warning on non-windows where `old_sidecar`
// isn't called — keeping the import unconditional simplifies tests.
#[cfg(not(windows))]
#[allow(dead_code)]
fn old_sidecar(current: &Path) -> PathBuf {
    let mut s = current.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_manifest(target: &str, url: &str, sha: &str) -> VersionManifest {
        let mut bins = HashMap::new();
        bins.insert(target.to_string(), url.to_string());
        let mut sums = HashMap::new();
        sums.insert(target.to_string(), sha.to_string());
        VersionManifest {
            core_version: "9.9.9".to_string(),
            binaries: bins,
            checksums_sha256: sums,
        }
    }

    #[test]
    fn pick_platform_asset_returns_entry_for_known_target() {
        let m = fake_manifest("x86_64-pc-windows-msvc", "https://x/a.zip", "ABCDEF");
        let a = pick_platform_asset(&m, "x86_64-pc-windows-msvc").unwrap();
        assert_eq!(a.url, "https://x/a.zip");
        assert_eq!(a.sha256, "abcdef", "sha must be normalised to lowercase");
    }

    #[test]
    fn pick_platform_asset_errors_on_unknown_target() {
        let m = fake_manifest("x86_64-pc-windows-msvc", "u", "s");
        let err = pick_platform_asset(&m, "riscv64-unknown-linux").unwrap_err();
        assert!(
            matches!(err, SelfUpdateError::NoAssetForTarget(t) if t == "riscv64-unknown-linux")
        );
    }

    #[test]
    fn verify_sha256_accepts_correct_hash() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        verify_sha256(
            b"hello",
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        )
        .expect("matching hash must pass");
    }

    #[test]
    fn verify_sha256_accepts_uppercase_hex() {
        verify_sha256(
            b"hello",
            "2CF24DBA5FB0A30E26E83B2AC5B9E29E1B161E5C1FA7425E73043362938B9824",
        )
        .expect("uppercase expected hex must still match");
    }

    #[test]
    fn verify_sha256_rejects_wrong_hash() {
        let err = verify_sha256(b"hello", "deadbeef".repeat(8).as_str()).unwrap_err();
        assert!(matches!(err, SelfUpdateError::Sha256Mismatch { .. }));
    }

    #[test]
    fn old_sidecar_appends_dot_old() {
        let p = old_sidecar(Path::new(r"C:\bin\standardoc.exe"));
        assert!(p.to_string_lossy().ends_with("standardoc.exe.old"));
    }

    #[test]
    fn default_manifest_url_uses_latest_alias() {
        let u = default_manifest_url();
        assert!(u.contains("releases/latest/download/version.json"), "{u}");
        assert!(u.contains("miralabs-tech/standardoc"), "{u}");
    }

    #[test]
    fn pinned_manifest_url_carries_the_tag() {
        let u = manifest_url_for_version("v1.0.0-beta.5");
        assert!(
            u.contains("releases/download/v1.0.0-beta.5/version.json"),
            "{u}"
        );
    }

    #[test]
    fn target_triple_matches_one_of_the_supported_platforms() {
        // Just guard against the compile-time const drifting away from
        // the release matrix — every supported triple is non-empty.
        assert!(!TARGET_TRIPLE.is_empty());
        assert!(TARGET_TRIPLE.contains('-'));
    }
}
