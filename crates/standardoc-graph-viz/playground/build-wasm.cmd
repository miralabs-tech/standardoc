@echo off
rem Wrapper invoked by cargo-watch from dev.ps1. Lives in the playground
rem folder so the crate path resolves via %~dp0.. (this script's dir
rem then go up one level). Passing a single-token script path to
rem cargo-watch's -s flag dodges the embedded-quote escaping problem
rem we hit when threading "wasm-pack build <path with separators>"
rem through PowerShell → Windows Terminal → pwsh -Command → cargo CLI.
wasm-pack build "%~dp0.." --target web --out-dir pkg --dev
