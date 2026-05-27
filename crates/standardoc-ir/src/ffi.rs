//! Stage 2 — FFI binding types shared across language providers.
//!
//! Most static-typed languages bind to foreign code through the **C ABI**:
//! Rust's `extern "C"`, Go's cgo, Zig's `extern fn`, Bun/Deno's `dlopen`,
//! and even JNI / Lua C API / Python C API are layers over a flat C-ABI
//! symbol name at the linker level. By tagging each side of a binding
//! with a `(abi, abi_name, direction)` tuple, the resolve pass can match
//! exports to imports across language boundaries without per-language
//! special cases.
//!
//! Providers populate `RawFfiBinding` during extraction; the pipeline
//! persists rows into `symbol_ffi_binding` and a single resolve pass
//! emits cross-language `IMPORTS` edges (attributed with `ffi:<abi>`).
//!
//! The taxonomy intentionally stays open: `FfiAbi::Other(String)` lets
//! a UST-registered provider tag bindings with a fresh ABI slug without
//! touching this enum (e.g. `FfiAbi::Other("syscall")` for a kernel
//! interface). The resolve pass treats unknown ABIs the same as known
//! ones — match by exact `(abi, abi_name)`.

use serde::{Deserialize, Serialize};

/// ABI category for an FFI binding. The vast majority of cross-language
/// bindings flow through `C` (Rust / Go / Zig / Bun / Deno / cffi /
/// ctypes); the named variants exist so the resolve pass can treat
/// e.g. Windows `system` calls and POSIX `c` calls as compatible only
/// where they actually are, and so consumers can filter the graph by
/// binding kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FfiAbi {
    /// Plain C ABI — the default for `extern "C"` in Rust, `//export`
    /// in Go, `export fn` in Zig, `dlopen` in Bun/Deno, etc.
    C,
    /// Windows / x86 calling conventions distinct from C: `system`,
    /// `stdcall`, `fastcall`. Rust models `system` as "C on Unix,
    /// stdcall on Windows" — we keep it as its own ABI so a binding
    /// declared `system` doesn't accidentally match a `c` export
    /// when the platform diverges.
    System,
    Stdcall,
    /// Lua C API binding — symbols named `luaopen_<module>` exported
    /// by C code are picked up at runtime by `require("<module>")`.
    Lua,
    /// JNI binding — C symbols named `Java_<pkg>_<class>_<method>`
    /// match Java `native` methods on the corresponding class.
    Jni,
    /// CPython C API — `PyInit_<module>` exports + `ctypes` /
    /// `cffi` imports referencing flat names.
    PythonCApi,
    /// Open slug for providers that need to declare an ABI without
    /// extending this enum. Resolve pass treats `Other("xyz")` as
    /// opaque: exact match required, no cross-ABI heuristics.
    Other(String),
}

impl FfiAbi {
    /// Short slug used in edge attributes (`ffi:c`, `ffi:lua`, …) and
    /// stored verbatim in the `symbol_ffi_binding.abi` column.
    #[must_use]
    pub const fn as_slug(&self) -> &str {
        match self {
            Self::C => "c",
            Self::System => "system",
            Self::Stdcall => "stdcall",
            Self::Lua => "lua",
            Self::Jni => "jni",
            Self::PythonCApi => "python_c_api",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Parse an ABI slug back. Unknown slugs land in `Other(_)`.
    #[must_use]
    pub fn from_slug(s: &str) -> Self {
        match s {
            "c" => Self::C,
            "system" => Self::System,
            "stdcall" => Self::Stdcall,
            "lua" => Self::Lua,
            "jni" => Self::Jni,
            "python_c_api" => Self::PythonCApi,
            other => Self::Other(other.to_string()),
        }
    }
}

/// Which side of the binding a symbol is on.
///
/// * `Export` — this symbol IS the implementation visible to foreign
///   callers (Rust `#[no_mangle] pub extern "C" fn`, C non-`static`
///   function, Go `//export`, Zig `export fn`).
/// * `Import` — this symbol REFERENCES a foreign implementation
///   (Rust `extern "C" { fn foo; }`, C `extern void foo();`, Bun
///   `dlopen({ foo })`, Python `ctypes.CDLL().foo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FfiDirection {
    Export,
    Import,
}

impl FfiDirection {
    /// Persisted slug. Stable for storage.
    #[must_use]
    pub const fn as_slug(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Import => "import",
        }
    }

    /// Parse a slug back. Returns `None` on unknown input — callers
    /// surface this as a storage-integrity error.
    #[must_use]
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "export" => Some(Self::Export),
            "import" => Some(Self::Import),
            _ => None,
        }
    }
}

/// One side of an FFI binding emitted by a language provider for a
/// specific symbol. The symbol is identified by its FQDN at extract
/// time; the storage layer resolves the FQDN to `symbols.id` at
/// insert time and writes a `symbol_ffi_binding` row.
///
/// `convention` is a free-text hint that a future, more specialised
/// resolve pass can use to disambiguate matches: `"lua-module"` on a
/// `luaopen_X` export, `"jni-Java_pkg_Class_method"` on a JNI export,
/// `"napi-register"` on a Node N-API entry point. Day-one resolve
/// matches purely on `(abi, abi_name)` and ignores `convention`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawFfiBinding {
    pub symbol_fqdn: String,
    pub abi: FfiAbi,
    pub direction: FfiDirection,
    /// The flat symbol name as seen by the linker. For most ABIs this
    /// is the source identifier verbatim; for `#[link_name = "alt"]`
    /// Rust imports it is the remapped name, not the Rust identifier.
    pub abi_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convention: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_abi_slug_round_trip_known_variants() {
        for abi in [
            FfiAbi::C,
            FfiAbi::System,
            FfiAbi::Stdcall,
            FfiAbi::Lua,
            FfiAbi::Jni,
            FfiAbi::PythonCApi,
        ] {
            let slug = abi.as_slug().to_string();
            let back = FfiAbi::from_slug(&slug);
            assert_eq!(abi, back, "slug {slug} must round-trip");
        }
    }

    #[test]
    fn ffi_abi_unknown_slug_lands_in_other() {
        let abi = FfiAbi::from_slug("syscall");
        assert_eq!(abi, FfiAbi::Other("syscall".into()));
        assert_eq!(abi.as_slug(), "syscall");
    }

    #[test]
    fn ffi_direction_slug_round_trip() {
        for d in [FfiDirection::Export, FfiDirection::Import] {
            assert_eq!(FfiDirection::from_slug(d.as_slug()).unwrap(), d);
        }
        assert!(FfiDirection::from_slug("sideways").is_none());
    }

    #[test]
    fn raw_ffi_binding_round_trips_through_json() {
        let b = RawFfiBinding {
            symbol_fqdn: "lurlang::runtime::vm::lur_vm_init".into(),
            abi: FfiAbi::C,
            direction: FfiDirection::Export,
            abi_name: "lur_vm_init".into(),
            convention: None,
        };
        let json = serde_json::to_string(&b).unwrap();
        let back: RawFfiBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn raw_ffi_binding_serialises_with_convention_only_when_set() {
        let b = RawFfiBinding {
            symbol_fqdn: "x::luaopen_mymod".into(),
            abi: FfiAbi::Lua,
            direction: FfiDirection::Export,
            abi_name: "luaopen_mymod".into(),
            convention: Some("lua-module".into()),
        };
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.contains("\"convention\":\"lua-module\""));
    }
}
