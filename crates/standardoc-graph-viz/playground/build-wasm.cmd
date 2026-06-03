@echo off
:: Wrapper invoked by cargo-watch from dev.ps1. Lives in the playground
:: folder so the crate path resolves via %~dp0.. (this script's dir
:: then go up one level). Passing a single-token script path to
:: cargo-watch's -s flag dodges the embedded-quote escaping problem
:: we hit when threading "wasm-pack build <path with separators>"
:: through PowerShell → Windows Terminal → pwsh -Command → cargo CLI.
wasm-pack build "%~dp0.." --target web --out-dir pkg --dev
