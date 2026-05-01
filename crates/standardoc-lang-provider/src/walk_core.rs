use std::collections::HashSet;

use standardoc_ir::{RawEdge, RawSymbol};

#[derive(Debug)]
pub(crate) struct WalkContextCore {
    pub(crate) file_path: String,
    pub(crate) file_module_fqdn: String,
    pub(crate) symbols: Vec<RawSymbol>,
    pub(crate) edges: Vec<RawEdge>,
    pub(crate) defined_fqdns: HashSet<String>,
}

impl WalkContextCore {
    pub(crate) fn new(file_path: String, file_module_fqdn: String) -> Self {
        Self {
            file_path,
            file_module_fqdn,
            symbols: Vec::new(),
            edges: Vec::new(),
            defined_fqdns: HashSet::new(),
        }
    }

    pub(crate) fn push_symbol(&mut self, sym: RawSymbol) {
        self.defined_fqdns.insert(sym.fqdn.clone());
        self.symbols.push(sym);
    }

    pub(crate) fn push_edge(&mut self, edge: RawEdge) {
        self.edges.push(edge);
    }
}
