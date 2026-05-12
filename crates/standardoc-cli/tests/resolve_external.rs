//! End-to-end dogfood of the lazy on-demand `resolve_external` chain.
//!
//! Runs `ResolverRegistry::for_workspace` → `registry.resolve(...)` against a
//! real `cargo metadata` subprocess on a fixture workspace that declares a
//! `serde` dependency. Validates that the cargo resolver locates serde's
//! source tree, ingests it via `RustProvider`, and surfaces a `RawSymbol`
//! for the requested FQDN.
//!
//! `#[ignore]` by default — requires `cargo` on `PATH` plus either network or
//! a warm `~/.cargo/registry/` cache for `serde`. Run explicitly with:
//!
//! ```sh
//! cargo test -p standardoc-cli --test resolve_external -- --ignored --nocapture
//! ```

use std::process::Command;
use std::sync::Arc;

use standardoc_core::{IndexHandle, LanguageProvider, ResolveOutcome, ResolverRegistry, query};
use standardoc_ir::SourceOrigin;
use standardoc_lang_provider::WorkspaceProvider;

const FIXTURE_MANIFEST: &str = "[package]
name = \"e2e-resolve-external\"
version = \"0.0.1\"
edition = \"2024\"

[lib]
path = \"src/lib.rs\"

[dependencies]
serde = \"1\"
";

#[test]
#[ignore = "requires cargo binary + warm `~/.cargo/registry` cache (or network) for serde"]
fn registry_resolves_serde_deserialize_via_cargo_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    std::fs::write(root.join("Cargo.toml"), FIXTURE_MANIFEST).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();

    let status = Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(root)
        .status()
        .expect("cargo binary on PATH required");
    assert!(status.success(), "cargo generate-lockfile failed");

    let handle = IndexHandle::open(root).expect("open IndexHandle");
    let provider: Arc<dyn LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let registry = ResolverRegistry::for_workspace(root.to_path_buf());
    assert!(
        !registry.is_empty(),
        "cargo resolver must register on workspace with Cargo.lock"
    );

    // serde 1.x exposes its API surface via `pub use core::de::Deserialize;`
    // in `src/lib.rs`. With fix B (item-level re-exports), the Rust provider
    // emits a phantom symbol at `serde::Deserialize` pointing at the
    // canonical `serde::core::de::Deserialize`. The cargo resolver's
    // `symbol_by_fqdn` lookup matches the phantom.
    let outcome = registry
        .resolve(&handle, provider.as_ref(), "serde::Deserialize")
        .expect("registry resolve must not error");

    match outcome {
        ResolveOutcome::Resolved {
            symbol,
            source_origin,
        } => {
            assert_eq!(source_origin, SourceOrigin::CargoRegistry);
            assert_eq!(symbol.fqdn, "serde::Deserialize");
            assert_eq!(symbol.name, "Deserialize");
        }
        other => {
            // Dogfood diagnostic: when resolve fails, dump every indexed
            // symbol whose `name == "Deserialize"` so the actual emitted
            // FQDN is visible. Reveals FQDN-shape mismatches between the
            // resolver's lookup and the Rust provider's output.
            let by_name = query::symbols_by_name(&handle, "Deserialize", 50)
                .expect("symbols_by_name must not error");
            eprintln!(
                "--- diagnostic: {} indexed symbols named `Deserialize` ---",
                by_name.len()
            );
            for s in &by_name {
                eprintln!(
                    "  fqdn=`{}` kind={:?} file=`{}`",
                    s.fqdn, s.kind, s.location.file
                );
            }
            let total = query::list_symbols(
                &handle,
                &query::SymbolFilter::default(),
                10,
            )
            .expect("list_symbols must not error");
            eprintln!(
                "--- diagnostic: first 10 of indexed symbols (any name) ---"
            );
            for s in &total {
                eprintln!("  fqdn=`{}` kind={:?}", s.fqdn, s.kind);
            }
            panic!(
                "expected Resolved for `serde::Deserialize`, got {other:?} \
                 — see diagnostic above"
            );
        }
    }
}
