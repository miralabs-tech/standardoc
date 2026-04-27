#!/usr/bin/env sh
# Standardoc installer — downloads the latest GitHub release for the current
# platform, verifies the SHA256 checksum, and installs the binaries to
# `$STANDARDOC_HOME/bin` (default: `$HOME/.standardoc/bin`).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/miralabs-tech/standardoc/main/scripts/install.sh | sh
#
# Environment overrides:
#   STANDARDOC_VERSION   — install a specific version (e.g. "v0.1.0"); defaults to latest
#   STANDARDOC_HOME      — install root (default: $HOME/.standardoc)
#   STANDARDOC_NO_PATH   — set to "1" to skip the PATH suggestion at the end

set -eu

REPO="miralabs-tech/standardoc"
HOME_DIR="${STANDARDOC_HOME:-$HOME/.standardoc}"
BIN_DIR="$HOME_DIR/bin"

err() { printf 'standardoc-install: error: %s\n' "$*" >&2; exit 1; }
say() { printf 'standardoc-install: %s\n' "$*"; }

command -v curl >/dev/null 2>&1 || err "curl is required"
command -v tar  >/dev/null 2>&1 || err "tar is required"

# ── Detect platform ────────────────────────────────────────────────────────
os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
case "$os-$arch" in
    linux-x86_64)        target="x86_64-unknown-linux-gnu" ;;
    linux-aarch64|linux-arm64) target="aarch64-unknown-linux-gnu" ;;
    darwin-x86_64)       target="x86_64-apple-darwin" ;;
    darwin-arm64)        target="aarch64-apple-darwin" ;;
    *) err "unsupported platform: $os-$arch (open an issue at https://github.com/$REPO/issues)" ;;
esac
say "platform detected: $target"

# ── Resolve version ────────────────────────────────────────────────────────
version="${STANDARDOC_VERSION:-}"
if [ -z "$version" ]; then
    say "querying latest release..."
    version=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)
    [ -n "$version" ] || err "could not determine latest version"
fi
say "version: $version"

# ── Download + verify ──────────────────────────────────────────────────────
asset="standardoc-$version-$target.tar.gz"
url="https://github.com/$REPO/releases/download/$version/$asset"
sha_url="$url.sha256"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "downloading $url"
curl -fsSL --output "$tmp/$asset" "$url" || err "download failed"

if curl -fsSL --output "$tmp/$asset.sha256" "$sha_url" 2>/dev/null; then
    say "verifying checksum..."
    expected=$(awk '{print $1}' "$tmp/$asset.sha256")
    if command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
    elif command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tmp/$asset" | awk '{print $1}')
    else
        say "no shasum/sha256sum found — skipping verification"
        actual="$expected"
    fi
    [ "$expected" = "$actual" ] || err "checksum mismatch (expected $expected, got $actual)"
fi

# ── Extract + install ──────────────────────────────────────────────────────
say "extracting to $BIN_DIR..."
mkdir -p "$BIN_DIR"
tar -xzf "$tmp/$asset" -C "$tmp"
extracted_dir="$tmp/standardoc-$version-$target"
cp "$extracted_dir/standardoc" "$BIN_DIR/standardoc"
cp "$extracted_dir/standardoc-server" "$BIN_DIR/standardoc-server"
chmod +x "$BIN_DIR/standardoc" "$BIN_DIR/standardoc-server"

# Also install the README/LICENSE/CHANGELOG alongside.
cp "$extracted_dir/README.md" "$HOME_DIR/" 2>/dev/null || true
cp "$extracted_dir/LICENSE"   "$HOME_DIR/" 2>/dev/null || true
cp "$extracted_dir/CHANGELOG.md" "$HOME_DIR/" 2>/dev/null || true

say "installed standardoc + standardoc-server to $BIN_DIR"

# ── PATH hint ──────────────────────────────────────────────────────────────
if [ "${STANDARDOC_NO_PATH:-}" != "1" ]; then
    case ":$PATH:" in
        *":$BIN_DIR:"*) say "PATH already includes $BIN_DIR — you're set." ;;
        *)
            say "add this to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
            printf '\n    export PATH="%s:$PATH"\n\n' "$BIN_DIR"
            ;;
    esac
fi

say "verify with: $BIN_DIR/standardoc --version"
