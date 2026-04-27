#!/usr/bin/env pwsh
# Render every template under docs-src/ to its output path at the workspace
# root, using `standardoc transform`. Run from the repo root.
#
# Usage : ./scripts/render-docs.ps1
#         (use $env:CARGO_FLAGS = '--release' for the production binary)

$ErrorActionPreference = 'Stop'

$root = Resolve-Path "$PSScriptRoot/.."
Set-Location $root

if (-not (Test-Path 'docs-src')) {
    Write-Host "render-docs: no docs-src/ folder found at $root - nothing to render."
    exit 0
}

$cargoFlags = if ($env:CARGO_FLAGS) { $env:CARGO_FLAGS } else { '--release' }

Get-ChildItem -Path 'docs-src' -Filter '*.md' -Recurse -File | ForEach-Object {
    $template = $_.FullName.Substring($root.Path.Length + 1) -replace '\\','/'
    $out = $template -replace '^docs-src/',''
    Write-Host "  rendering $template -> $out"
    $outDir = Split-Path $out -Parent
    if ($outDir -and -not (Test-Path $outDir)) {
        New-Item -ItemType Directory -Force -Path $outDir | Out-Null
    }
    $rendered = & cargo run --quiet $cargoFlags -p standardoc -- transform . $template
    if ($LASTEXITCODE -ne 0) {
        throw "render-docs: 'standardoc transform' failed for $template"
    }
    [System.IO.File]::WriteAllText((Resolve-Path $out -Relative:$false 2>$null) ?? (Join-Path $root.Path $out), ($rendered -join "`n"))
}

Write-Host 'render-docs: done.'
