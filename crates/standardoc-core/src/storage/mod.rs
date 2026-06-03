// Storage helpers exposed to the ingestion pipeline (`crate::pipeline`).
// The blanket `dead_code` allow covers an intentional scaffold stratum:
// write/lifecycle paths that are live but whose read/consumer side is
// not yet wired — e.g. `enrichments` (EN-1), `documents` (get_document),
// `module_lookup::workspace_imports_by_origin` (reverse-dep query), the
// `symbol_decl_location` read path, and the unwired `workspace_catalog`
// mutators. `fts` is fully wired. The allow is kept deliberately so the
// compiler doesn't flag these pending-consumer helpers.
#![allow(dead_code)]

pub(crate) mod call_sites;
pub(crate) mod conv;
pub(crate) mod cross_workspace;
pub(crate) mod documents;
pub(crate) mod edge_sites;
pub(crate) mod edges;
pub(crate) mod enrichments;
pub(crate) mod error;
pub(crate) mod files;
pub(crate) mod fts;
pub(crate) mod handle;
pub(crate) mod init;
pub(crate) mod lock;
pub(crate) mod migrate;
pub(crate) mod module_lookup;
pub(crate) mod projects;
pub(crate) mod retry;
pub(crate) mod schema_meta;
pub(crate) mod symbol_decl_location;
pub(crate) mod symbol_ffi_binding;
pub(crate) mod symbols;
pub(crate) mod workspace_catalog;

#[cfg(test)]
pub(crate) mod test_utils;
