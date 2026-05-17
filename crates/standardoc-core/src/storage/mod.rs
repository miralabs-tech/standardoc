// Storage helpers exposed to the ingestion pipeline (`crate::pipeline`).
// `enrichments`, `documents`, and `fts` are implemented but not wired by
// 14a — they will be consumed by the enrichment pass and the daemon side
// in later sessions, hence the surviving `dead_code` allow.
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
pub(crate) mod symbols;
pub(crate) mod workspace_catalog;

#[cfg(test)]
pub(crate) mod test_utils;
