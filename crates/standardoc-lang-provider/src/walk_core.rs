use std::collections::HashSet;

use standardoc_ir::{Language, ModuleLookup, RawCallSite, RawDocument, RawEdge, RawSymbol};

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
