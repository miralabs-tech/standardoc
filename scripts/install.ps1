# Standardoc installer for Windows — downloads the latest GitHub release for
# x86_64-pc-windows-msvc, verifies the SHA256 checksum, and installs the
# binaries to $env:STANDARDOC_HOME\bin (default: $env:USERPROFILE\.standardoc\bin).
#
# Usage:
#   irm https://raw.githubusercontent.com/miralabs-tech/standardoc/main/scripts/install.ps1 | iex
#
# Environment overrides:
#   $env:STANDARDOC_VERSION  — install a specific version (e.g. "v0.1.0"); defaults to latest
#   $env:STANDARDOC_HOME     — install root (default: $env:USERPROFILE\.standardoc)
#   $env:STANDARDOC_NO_PATH  — set to "1" to skip the PATH suggestion at the end

$ErrorActionPreference = 'Stop'

$Repo    = 'miralabs-tech/standardoc'
$HomeDir = if ($env:STANDARDOC_HOME) { $env:STANDARDOC_HOME } else { Join-Path $env:USERPROFILE '.standardoc' }
$BinDir  = Join-Path $HomeDir 'bin'

function Say([string]$msg)  { Write-Host "standardoc-install: $msg" -ForegroundColor Cyan }
function Fail([string]$msg) { Write-Host "standardoc-install: error: $msg" -ForegroundColor Red; exit 1 }

# ── Resolve version ────────────────────────────────────────────────────────
$version = $env:STANDARDOC_VERSION
if (-not $version) {
    Say 'querying latest release...'
    try {
        $release = Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$Repo/releases/latest"
        $version = $release.tag_name
    } catch {
        Fail "could not determine latest version: $_"
    }
}
Say "version: $version"

# ── Download + verify ──────────────────────────────────────────────────────
$target  = 'x86_64-pc-windows-msvc'
$asset   = "standardoc-$version-$target.zip"
$url     = "https://github.com/$Repo/releases/download/$version/$asset"
$shaUrl  = "$url.sha256"
$tmpDir  = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "standardoc-install-$([guid]::NewGuid())")

try {
    $zipPath = Join-Path $tmpDir $asset
    Say "downloading $url"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zipPath

    try {
        $shaPath = Join-Path $tmpDir "$asset.sha256"
        Invoke-WebRequest -UseBasicParsing -Uri $shaUrl -OutFile $shaPath
        Say 'verifying checksum...'
        $expected = (Get-Content $shaPath).Split(' ')[0].Trim().ToLower()
        $actual   = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLower()
        if ($expected -ne $actual) {
            Fail "checksum mismatch (expected $expected, got $actual)"
        }
    } catch {
        Say 'checksum file not available — skipping verification'
    }

    # ── Extract + install ──────────────────────────────────────────────────
    Say "extracting to $BinDir..."
    if (-not (Test-Path $BinDir)) { New-Item -ItemType Directory -Path $BinDir -Force | Out-Null }
    Expand-Archive -Path $zipPath -DestinationPath $tmpDir -Force
    $extractedDir = Join-Path $tmpDir "standardoc-$version-$target"

    Copy-Item (Join-Path $extractedDir 'standardoc.exe')        -Destination (Join-Path $BinDir 'standardoc.exe')        -Force
    Copy-Item (Join-Path $extractedDir 'standardoc-server.exe') -Destination (Join-Path $BinDir 'standardoc-server.exe') -Force

    foreach ($f in @('README.md', 'LICENSE', 'CHANGELOG.md')) {
        $src = Join-Path $extractedDir $f
        if (Test-Path $src) { Copy-Item $src -Destination $HomeDir -Force }
    }

    Say "installed standardoc.exe + standardoc-server.exe to $BinDir"

    # ── PATH hint ──────────────────────────────────────────────────────────
    if ($env:STANDARDOC_NO_PATH -ne '1') {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        if ($userPath -and $userPath.Split(';') -contains $BinDir) {
            Say "PATH already includes $BinDir — you're set."
        } else {
            Say "to add to your User PATH permanently, run:"
            Write-Host ""
            Write-Host "    [Environment]::SetEnvironmentVariable('Path', `"$BinDir;`$([Environment]::GetEnvironmentVariable('Path','User'))`", 'User')" -ForegroundColor Yellow
            Write-Host ""
            Say "(then restart your shell)"
        }
    }

    Say "verify with: $BinDir\standardoc.exe --version"
}
finally {
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
}
