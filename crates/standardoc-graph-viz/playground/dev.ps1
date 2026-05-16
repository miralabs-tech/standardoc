<#
.SYNOPSIS
    One-shot dev loop for the standardoc-graph-viz playground.

.DESCRIPTION
    Spawns two independent shells running in parallel:
      1. `cargo watch` rebuilds the wasm bundle whenever any file under
         the crate's `src/` changes (calls wasm-pack to regenerate
         `pkg/`).
      2. `bun run dev` serves the playground on http://localhost:3000
         with --hot, so editing main.ts / index.html / style.css
         hot-reloads in the browser.

    Each side runs in its own console so progress bars and colour
    output render correctly — no fragile job polling. By default the
    script prefers Windows Terminal (`wt.exe`) and opens a single
    window with two side-by-side panes; if `wt` is unavailable it
    falls back to two separate PowerShell windows.

.PARAMETER SkipInitialBuild
    Skip the upfront `wasm-pack build`. Useful when you know the
    `pkg/` directory is already populated from a previous run.

.PARAMETER NoSplit
    Always spawn two separate PowerShell windows even when Windows
    Terminal is available. Handy on multi-monitor setups where you
    want each task on its own screen.

.EXAMPLE
    pwsh crates/standardoc-graph-viz/playground/dev.ps1

.EXAMPLE
    pwsh crates/standardoc-graph-viz/playground/dev.ps1 -SkipInitialBuild

.EXAMPLE
    pwsh crates/standardoc-graph-viz/playground/dev.ps1 -NoSplit
#>
[CmdletBinding()]
param(
    [switch]$SkipInitialBuild,
    [switch]$NoSplit
)

$ErrorActionPreference = 'Stop'

$playgroundDir = $PSScriptRoot
$crateDir      = Split-Path -Parent $playgroundDir
$crateSrcDir   = Join-Path $crateDir 'src'

Write-Host "[dev] crate     : $crateDir"      -ForegroundColor DarkGray
Write-Host "[dev] playground: $playgroundDir" -ForegroundColor DarkGray

function Test-Tool {
    param([string]$Name, [string]$InstallHint)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        Write-Host "[dev] missing tool: $Name" -ForegroundColor Red
        Write-Host "      → $InstallHint"     -ForegroundColor Yellow
        exit 1
    }
}

Test-Tool -Name 'cargo'     -InstallHint 'install Rust via https://rustup.rs'
Test-Tool -Name 'wasm-pack' -InstallHint 'cargo install wasm-pack'
Test-Tool -Name 'bun'       -InstallHint 'install Bun via https://bun.sh'

# cargo-watch is a Cargo subcommand, not a separate binary on PATH —
# probe via `cargo install --list` and offer a one-shot install.
$cargoWatchInstalled = cargo install --list 2>$null |
    Select-String -SimpleMatch 'cargo-watch v' -Quiet
if (-not $cargoWatchInstalled) {
    Write-Host '[dev] cargo-watch not installed — installing now…' -ForegroundColor Yellow
    cargo install cargo-watch
    if ($LASTEXITCODE -ne 0) {
        Write-Host '[dev] cargo install cargo-watch failed' -ForegroundColor Red
        exit 1
    }
}

if (-not $SkipInitialBuild) {
    Write-Host '[dev] initial wasm-pack build…' -ForegroundColor Cyan
    & wasm-pack build $crateDir --target web --out-dir pkg --dev
    if ($LASTEXITCODE -ne 0) {
        Write-Host '[dev] initial wasm-pack build failed' -ForegroundColor Red
        exit 1
    }
}

if (-not (Test-Path (Join-Path $playgroundDir 'node_modules'))) {
    Write-Host '[dev] installing playground deps (bun install)…' -ForegroundColor Cyan
    Push-Location $playgroundDir
    try {
        & bun install
        if ($LASTEXITCODE -ne 0) {
            Write-Host '[dev] bun install failed' -ForegroundColor Red
            exit 1
        }
    } finally {
        Pop-Location
    }
}

# cargo-watch's `-s` flag wants a single shell command. Threading
# `wasm-pack build <path> --target web ...` through PowerShell →
# Windows Terminal → pwsh -Command → cargo CLI eats the inner quotes,
# leaving cargo-watch to misparse the value. We side-step the whole
# escape ladder by pointing `-s` at a small `.cmd` wrapper that lives
# next to this script — a single argv token, nothing to quote.
$wrapperPath = Join-Path $playgroundDir 'build-wasm.cmd'
if (-not (Test-Path $wrapperPath)) {
    Write-Host "[dev] missing wrapper: $wrapperPath" -ForegroundColor Red
    exit 1
}
if ($crateSrcDir -match '\s') {
    Write-Host '[dev] watch path contains whitespace — cargo-watch -w quoting may break.' -ForegroundColor Yellow
    Write-Host "      $crateSrcDir" -ForegroundColor Yellow
}
$wasmCommand = "cargo watch -w $crateSrcDir -s $wrapperPath"
$playCommand = 'bun run dev'

# The child shells run with -NoExit so the user sees the output and
# can Ctrl+C / close at their leisure. -NoProfile keeps the shell cold
# and fast.
$wasmTitle = 'standardoc — wasm watch'
$playTitle = 'standardoc — playground'

$useWt = (-not $NoSplit) -and (Get-Command 'wt' -ErrorAction SilentlyContinue)

Write-Host ''
if ($useWt) {
    Write-Host '[dev] launching Windows Terminal split panes…' -ForegroundColor Green
    # Use the call operator (&) instead of Start-Process so PowerShell
    # quotes individual arguments correctly for the native exe (titles
    # contain spaces — Start-Process's ArgumentList join leaks them as
    # separate argv entries and wt then mistakes the second word as a
    # command name).
    #
    # The backtick-semicolon (`;) is a literal `;` argument — wt uses
    # `;` as the action separator that turns the rest into a chained
    # `split-pane` action on the same tab. Without the backtick,
    # PowerShell would end the statement here.
    & wt new-tab `
        --title $wasmTitle `
        -d $playgroundDir `
        pwsh -NoProfile -NoExit -Command $wasmCommand `
        `; split-pane -H `
        --title $playTitle `
        -d $playgroundDir `
        pwsh -NoProfile -NoExit -Command $playCommand
} else {
    Write-Host '[dev] launching two PowerShell windows…' -ForegroundColor Green
    $sharedArgs = @('-NoProfile', '-NoExit', '-Command')
    Start-Process -FilePath 'pwsh' `
        -WorkingDirectory $playgroundDir `
        -ArgumentList ($sharedArgs + @("`$Host.UI.RawUI.WindowTitle = '$wasmTitle'; $wasmCommand"))
    Start-Process -FilePath 'pwsh' `
        -WorkingDirectory $playgroundDir `
        -ArgumentList ($sharedArgs + @("`$Host.UI.RawUI.WindowTitle = '$playTitle'; $playCommand"))
}

Write-Host ''
Write-Host '[dev] both processes launched.'                              -ForegroundColor Green
Write-Host '[dev] playground → http://localhost:3000'                    -ForegroundColor Cyan
Write-Host '[dev] close the two child windows / panes to stop the loop.' -ForegroundColor DarkGray
