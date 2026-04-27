#!/usr/bin/env pwsh
# Interactive build helper for standardoc-server.
#
# - dev   : builds to target-dev/ — never conflicts with running servers
# - prod  : kills running standardoc-server.exe, then builds to target/
# - inspect : lists running servers (pid, parent ide, mode, workspace)

$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $RepoRoot

function Get-StandardocServers {
    Get-CimInstance Win32_Process -Filter "Name = 'standardoc-server.exe'" |
        ForEach-Object {
            $parent = Get-Process -Id $_.ParentProcessId -ErrorAction SilentlyContinue
            $cmd = $_.CommandLine
            $mode = if ($cmd -match '--(mcp|lsp|web|export)') { $matches[1] } else { '?' }
            $workspace = if ($cmd -match '--workspace\s+"?([^"]+?)"?(\s|$)') { $matches[1] } else { '?' }
            [PSCustomObject]@{
                PID         = $_.ProcessId
                Mode        = $mode
                Parent      = if ($parent) { "$($parent.ProcessName) ($($_.ParentProcessId))" } else { "<gone> ($($_.ParentProcessId))" }
                Workspace   = $workspace
            }
        }
}

function Show-Servers {
    $servers = Get-StandardocServers
    if ($servers.Count -eq 0) {
        Write-Host "No standardoc-server.exe processes running." -ForegroundColor Green
        return @()
    }
    Write-Host "`nRunning standardoc-server.exe processes:" -ForegroundColor Cyan
    $servers | Format-Table -AutoSize | Out-String | Write-Host
    return $servers
}

function Invoke-DevBuild {
    Write-Host "`nBuilding release into target-dev/ (no kill required)..." -ForegroundColor Cyan
    cargo build --release --features standardoc-web/embedded-frontend --target-dir target-dev
    if ($LASTEXITCODE -eq 0) {
        $bin = Join-Path $RepoRoot 'target-dev\release\standardoc-server.exe'
        Write-Host "`nBuilt: $bin" -ForegroundColor Green
    }
}

function Invoke-ProdBuild {
    $servers = Show-Servers
    if ($servers.Count -gt 0) {
        $confirm = Read-Host "`nKill these $($servers.Count) process(es) and rebuild? (y/N)"
        if ($confirm -ne 'y' -and $confirm -ne 'Y') {
            Write-Host "Aborted." -ForegroundColor Yellow
            return
        }
        foreach ($s in $servers) {
            try {
                Stop-Process -Id $s.PID -Force -ErrorAction Stop
                Write-Host "Killed PID $($s.PID)" -ForegroundColor DarkGray
            } catch {
                Write-Host "Failed to kill PID $($s.PID): $_" -ForegroundColor Red
            }
        }
        Start-Sleep -Milliseconds 500
    }
    Write-Host "`nBuilding release into target/..." -ForegroundColor Cyan
    cargo build --release --features standardoc-web/embedded-frontend
    if ($LASTEXITCODE -eq 0) {
        $bin = Join-Path $RepoRoot 'target\release\standardoc-server.exe'
        Write-Host "`nBuilt: $bin" -ForegroundColor Green
        Write-Host "Restart your IDE(s) to spawn fresh servers." -ForegroundColor Yellow
    }
}

while ($true) {
    Write-Host "`nstandardoc build helper" -ForegroundColor Magenta
    Write-Host "  [1] dev      build to target-dev/ (parallel-safe)"
    Write-Host "  [2] prod     kill servers + build to target/"
    Write-Host "  [3] inspect  list running servers, no build"
    Write-Host "  [q] quit"
    $choice = Read-Host "`nChoice"

    switch ($choice) {
        '1' { Invoke-DevBuild }
        '2' { Invoke-ProdBuild }
        '3' { [void](Show-Servers) }
        'q' { return }
        'Q' { return }
        default { Write-Host "Unknown choice: $choice" -ForegroundColor Red }
    }
}
