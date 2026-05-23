use std::collections::HashSet;

use standardoc_ir::{Language, ModuleLookup, RawCallSite, RawDocument, RawEdge, RawSymbol};

use crate::utils::strip_extension;

/// Per-language conventions consumed by [`compute_module_path`] so the
/// path-to-module logic lives in a single helper instead of being
/// duplicated 4 times. Each lang-provider exposes a `const` of this
/// type next to its extractor entry point.
pub(crate) struct LanguagePathConventions {
    /// File extensions to strip from the path tail (e.g. `[".c", ".h"]`).
    /// Order matters when one extension is a suffix of another (e.g.
    /// `.d.ts` must precede `.ts`); the first match wins.
    pub extensions: &'static [&'static str],
    /// Final path segments to drop when they alias the parent module:
    /// Rust's `lib`/`main`/`mod`, TS's `index`, Lua's `init`. Bare files
    /// matching one of these aliases (e.g. Lua `init.lua` at the
    /// package root) collapse to an empty module path.
    pub root_aliases: &'static [&'static str],
    /// When `true`, a leading `src/` (or any `*/src/`) prefix is
    /// stripped before path-segment processing. Rust convention; the
    /// other languages don't gate their source layout on a `src/` dir.
    pub strip_src_prefix: bool,
}

/// Canonical `::`-joined module path for `package_relative` under the
/// given language conventions. Returns an empty string when the file
/// IS the package root (Lua `init.lua`, TS `index.ts`, Rust `lib.rs`).
/// Both `/` and `\\` are recognised as path separators so callers can
/// pass POSIX or Windows paths interchangeably.
pub(crate) fn compute_module_path(
    conventions: &LanguagePathConventions,
    package_relative: &str,
) -> String {
    let after_src = if conventions.strip_src_prefix {
        strip_src_prefix(package_relative)
    } else {
        package_relative
    };
    let stem = strip_extension(after_src, conventions.extensions);
    let segments: Vec<&str> = stem
        .split(|c: char| c == '/' || c == '\\')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return String::new();
    }
    let drop_last = segments
        .last()
        .is_some_and(|s| conventions.root_aliases.iter().any(|a| *a == *s));
    let useful: &[&str] = if drop_last {
        &segments[..segments.len() - 1]
    } else {
        &segments[..]
    };
    useful.join("::")
}

fn strip_src_prefix(file_rel: &str) -> &str {
    if let Some(idx) = file_rel.rfind("/src/") {
        return &file_rel[idx + "/src/".len()..];
    }
    if let Some(rest) = file_rel.strip_prefix("src/") {
        return rest;
    }
    file_rel
}

#[derive(Debug)]
pub(crate) struct WalkContextCore {
    pub(crate) file_path: String,
    pub(crate) file_module_fqdn: String,
    pub(crate) symbols: Vec<RawSymbol>,
    pub(crate) edges: Vec<RawEdge>,
    pub(crate) documents: Vec<RawDocument>,
    /// IR-4: observational call-expression records — collected alongside
    /// `Calls` edges but distinct in purpose. Edges drive the symbol
    /// graph; call_sites carry textual shape (callee_text, literal arg
    /// values, receiver chain) for the post-1.0 plugin layer to
    /// re-interpret without re-parsing source.
    pub(crate) call_sites: Vec<RawCallSite>,
    pub(crate) defined_fqdns: HashSet<String>,
    pub(crate) lookup: ModuleLookup,
}

impl WalkContextCore {
    pub(crate) fn new(file_path: String, file_module_fqdn: String, language: Language) -> Self {
        let lookup = ModuleLookup::new(file_module_fqdn.clone(), language);
        Self {
            file_path,
            file_module_fqdn,
            symbols: Vec::new(),
            edges: Vec::new(),
            documents: Vec::new(),
            call_sites: Vec::new(),
            defined_fqdns: HashSet::new(),
            lookup,
        }
    }

    pub(crate) fn push_symbol(&mut self, sym: RawSymbol) {
        self.defined_fqdns.insert(sym.fqdn.clone());
        self.symbols.push(sym);
    }

    pub(crate) fn push_edge(&mut self, edge: RawEdge) {
        self.edges.push(edge);
    }

    pub(crate) fn push_document(&mut self, doc: RawDocument) {
        self.documents.push(doc);
    }

    pub(crate) fn push_call_site(&mut self, cs: RawCallSite) {
        self.call_sites.push(cs);
    }
}
