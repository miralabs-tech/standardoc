use std::collections::HashSet;

use standardoc_ir::{Language, ModuleLookup, RawDocument, RawEdge, RawSymbol};

#[derive(Debug)]
pub(crate) struct WalkContextCore {
    pub(crate) file_path: String,
    pub(crate) file_module_fqdn: String,
    pub(crate) symbols: Vec<RawSymbol>,
    pub(crate) edges: Vec<RawEdge>,
    pub(crate) documents: Vec<RawDocument>,
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
}
