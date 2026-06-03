use crate::kinds::{Kind, Language};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AliasMutability {
    Const,
    Mutable,
}

impl AliasMutability {
    pub const fn as_slug(self) -> &'static str {
        match self {
            Self::Const => "via-alias",
            Self::Mutable => "via-alias-mutable",
        }
    }
}

/// Where the code physically lives / executes. Orthogonal to `Language`:
/// a Lua program can run as `Native { language: Lua }`, be embedded via
/// `Ust { backing: "lua" }`, or compile to `Wasm`.
///
/// IR-2 1.0 widening — the six unit variants (`Browser`, `Node`,
/// `Database`, `MessageBus`, `Kernel`, `Hypervisor`) promote common
/// substrates to first-class so the (from, to) pair on a
/// [`crate::SubstrateBridge`] can express e.g. Tauri (Native↔Browser)
/// or Prisma (Node↔Database) without falling back to `Custom`. Fine-
/// grained disambiguation (`postgres` vs `mysql`, `chromium` vs
/// `webkit`) stays in `Custom` via tags like `"database:postgres"`
/// or `"browser:webkit"` — and in [`crate::BridgeKind`] (`prisma`,
/// `sqlx`, …) for the access mechanism.
///
/// `Custom` keeps the door open for UST-defined substrates added
/// without recompiling the core.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Substrate {
    Native {
        language: Language,
    },
    Ust {
        backing: String,
    },
    Wasm,
    Ffi {
        abi: String,
    },
    /// Browser-side JS runtime (V8, SpiderMonkey, JavaScriptCore). Engine
    /// specifics → `Custom { tag: "browser:<engine>" }`.
    Browser,
    /// Server-side JS runtime (Node.js, Bun, Deno). Runtime specifics →
    /// `Custom { tag: "node:<runtime>" }`.
    Node,
    /// Database execution environment (stored procs, views, triggers,
    /// agg pipelines). Engine specifics → `Custom { tag: "database:<engine>" }`.
    Database,
    /// Message-bus consumer / producer environment (Kafka, Redis streams,
    /// NATS, RabbitMQ, …). Transport specifics →
    /// `Custom { tag: "message_bus:<transport>" }`.
    MessageBus,
    /// OS kernel space (drivers, eBPF programs, kernel modules).
    Kernel,
    /// Hypervisor / VM-monitor execution (Xen, KVM extensions).
    Hypervisor,
    Custom {
        tag: String,
    },
}

impl Substrate {
    pub const fn native(language: Language) -> Self {
        Self::Native { language }
    }
}

/// Categorises a `BindingSource::LocalDecl`. Hardcoded variants cover the
/// languages extracted natively today (Rust, TS/JS, Lua). `Custom` is the
/// UST escape hatch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalDeclKind {
    Function,
    Class,
    Interface,
    Enum,
    Struct,
    Trait,
    Impl,
    TypeAlias,
    Const,
    Let,
    Var,
    Macro,
    Module,
    Namespace,
    Custom { lang: Language, tag: String },
}

/// Lexical scope category. Purely informational — the resolver only uses
/// `(start_line, end_line, parent)`. `Custom` extends to UST-introduced
/// scope kinds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Module,
    Function,
    Block,
    TypeContainer,
    Loop,
    Catch,
    Macro,
    Custom { lang: Language, tag: String },
}

/// Coarse semantic category for built-in identifiers. Hardcoded variants
/// are language-neutral semantic buckets; `Custom` lets UST languages
/// introduce new tags at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinTag {
    Net,
    Decode,
    Encode,
    Console,
    FileSystem,
    Process,
    Math,
    Time,
    Memory,
    Reflection,
    Async,
    Iter,
    Format,
    Custom { tag: String },
}

impl BuiltinTag {
    /// Kebab-slug representation, used as a suffix in edge attributes
    /// like `builtin-fs`, `builtin-async`, `builtin-<custom>`. Stable
    /// across languages so viz/queries can filter by tag without caring
    /// which provider produced the edge.
    pub fn slug(&self) -> String {
        match self {
            Self::Net => "net".into(),
            Self::Decode => "decode".into(),
            Self::Encode => "encode".into(),
            Self::Console => "console".into(),
            Self::FileSystem => "fs".into(),
            Self::Process => "process".into(),
            Self::Math => "math".into(),
            Self::Time => "time".into(),
            Self::Memory => "memory".into(),
            Self::Reflection => "reflection".into(),
            Self::Async => "async".into(),
            Self::Iter => "iter".into(),
            Self::Format => "format".into(),
            Self::Custom { tag } => tag.clone(),
        }
    }
}

/// Origin of an `IdentResolution`. Every entry in `ModuleLookup.bindings`
/// carries exactly one `BindingSource`. The resolver uses it to know how
/// to turn a binding into a `ResolvedOrUnresolved` edge target.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingSource {
    Import {
        module_path: String,
        original_name: Option<String>,
        is_type_only: bool,
        is_re_export: bool,
    },
    LocalDecl {
        decl_kind: LocalDeclKind,
    },
    TypeParam,
    Param,
    Builtin {
        tag: BuiltinTag,
        synthetic_fqdn: String,
    },
    Bridge {
        from_substrate: Substrate,
        to_substrate: Substrate,
        synthetic_fqdn: String,
    },
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopeRange {
    pub start_line: u32,
    pub end_line: u32,
    pub parent: Option<u32>,
    pub kind: ScopeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdentResolution {
    pub name: String,
    pub source: BindingSource,
    pub resolved_fqdn: Option<String>,
    pub aliases_to: Option<String>,
    pub mutability: Option<AliasMutability>,
    pub scope_idx: u32,
    pub attributes: Vec<String>,
    pub ir_kind: Option<Kind>,
}

/// Flat list entry per import binding. Duplicates the relevant info from
/// `bindings[local_name].source: BindingSource::Import` but is indexed for
/// the Stage 3b cross-workspace SQL join (`workspace_imports`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImportRecord {
    pub local_name: String,
    pub origin_module: String,
    pub origin_symbol: Option<String>,
    pub is_type_only: bool,
    pub is_re_export: bool,
}

/// AOT-built per-module identifier resolution table. Stage 3a uses this
/// in-memory only (built by `*::lookup::build_*_lookup`, consumed by the
/// visitor, dropped). Stage 3b persists it via bincode in `module_lookups`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleLookup {
    pub module_fqdn: String,
    pub language: Language,
    pub scopes: Vec<ScopeRange>,
    pub bindings: HashMap<String, Vec<IdentResolution>>,
    pub imports: Vec<ImportRecord>,
    pub built_at_epoch_ms: u64,
    /// Byte-span → scope_idx map populated by the pre-pass so the
    /// Stage 3a-6c/8c visitors can look up "which scope_idx covers this
    /// AST node?" without replaying the scope DFS themselves. NOT
    /// persisted — the span keys are walk-local artefacts; cross-
    /// workspace consumers (Stage 3b) only need root-scope bindings and
    /// re-derive nothing from this map.
    #[serde(skip)]
    pub span_to_scope: HashMap<(u32, u32), u32>,
}

impl ModuleLookup {
    /// Index of the implicit root scope (always 0).
    pub const ROOT_SCOPE: u32 = 0;

    pub fn new(module_fqdn: String, language: Language) -> Self {
        let root = ScopeRange {
            start_line: 1,
            end_line: u32::MAX,
            parent: None,
            kind: ScopeKind::Module,
        };
        Self {
            module_fqdn,
            language,
            scopes: vec![root],
            bindings: HashMap::new(),
            imports: Vec::new(),
            built_at_epoch_ms: epoch_ms_now(),
            span_to_scope: HashMap::new(),
        }
    }

    /// Push a new scope range and return its arena index.
    #[allow(clippy::cast_possible_truncation)]
    pub fn push_scope(&mut self, range: ScopeRange) -> u32 {
        let idx = self.scopes.len() as u32;
        self.scopes.push(range);
        idx
    }

    /// Push a new scope AND record its byte span in `span_to_scope`. The
    /// visitor consults this map at each scope-introducing AST node to
    /// recover the corresponding scope_idx without replaying the DFS.
    /// Spans must be unique per scope; duplicate keys would only happen
    /// if two distinct scopes shared the same byte range (impossible for
    /// well-formed source).
    pub fn push_scope_with_span(&mut self, range: ScopeRange, byte_lo: u32, byte_hi: u32) -> u32 {
        let idx = self.push_scope(range);
        self.span_to_scope.insert((byte_lo, byte_hi), idx);
        idx
    }

    /// Recover the scope_idx assigned to the AST node spanning
    /// `[byte_lo, byte_hi)`. Returns `None` when the pre-pass did not
    /// record this span (caller should fall back to the current scope_idx
    /// so resolution still works for non-scope-introducing nodes).
    pub fn scope_idx_for_span(&self, byte_lo: u32, byte_hi: u32) -> Option<u32> {
        self.span_to_scope.get(&(byte_lo, byte_hi)).copied()
    }

    /// Insert a binding. Multiple inserts under the same name model
    /// shadowing — later entries win when resolved from the same scope.
    pub fn push_binding(&mut self, resolution: IdentResolution) {
        self.bindings
            .entry(resolution.name.clone())
            .or_default()
            .push(resolution);
    }

    /// Record an import in the flat `imports` list (Stage 3b consumes it).
    pub fn push_import(&mut self, record: ImportRecord) {
        self.imports.push(record);
    }

    /// Resolve `name` starting from `scope_idx` and walking up the parent
    /// chain to root. Returns the most-inner shadowing binding found, or
    /// `None` if no scope in the chain has a matching binding.
    pub fn resolve_local(&self, name: &str, mut scope_idx: u32) -> Option<&IdentResolution> {
        let entries = self.bindings.get(name)?;
        loop {
            if let Some(found) = entries.iter().rev().find(|r| r.scope_idx == scope_idx) {
                return Some(found);
            }
            match self.scopes.get(scope_idx as usize).and_then(|s| s.parent) {
                Some(parent) => scope_idx = parent,
                None => return None,
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn epoch_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests;
