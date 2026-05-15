//! Bridge between `standardoc-rag::SymbolLookup` and the core's `query`
//! API. `CoreSymbolLookup` wraps an `IndexHandle` and exposes :
//!
//! - **`workspace_fqdns`** — every non-external symbol fqdn (workspace
//!   only). External symbols are intentionally excluded from prose
//!   auto-linking : they generate noise without traceability.
//! - **`def_site_path`** — workspace-relative path of the symbol's
//!   def-site location, used by the linker's `def_site_boost` signal.

use standardoc_rag::error::RagError;
use standardoc_rag::linker::SymbolLookup;

use crate::query::{self, SymbolFilter};
use crate::storage::handle::IndexHandle;

/// Cap for `workspace_fqdns` lookups. The auto-FQDN scan is O(N × M)
/// where N is fqdn count and M is chunk count ; bounding N at
/// `WORKSPACE_FQDN_LIMIT` keeps the linker O(M) for sane workspaces.
/// Workspaces above this cap fall back to a frontmatter-only linking
/// mode (no auto signal) — graceful degradation.
pub const WORKSPACE_FQDN_LIMIT: usize = 50_000;

pub struct CoreSymbolLookup<'a> {
    handle: &'a IndexHandle,
}

impl<'a> CoreSymbolLookup<'a> {
    pub const fn new(handle: &'a IndexHandle) -> Self {
        Self { handle }
    }
}

impl SymbolLookup for CoreSymbolLookup<'_> {
    fn workspace_fqdns(&self) -> Result<Vec<String>, RagError> {
        let filter = SymbolFilter {
            include_external: false,
            ..SymbolFilter::default()
        };
        let symbols =
            query::list_symbols(self.handle, &filter, WORKSPACE_FQDN_LIMIT).map_err(|e| {
                RagError::InvalidStoredData {
                    detail: format!("list_symbols: {e}"),
                }
            })?;
        Ok(symbols.into_iter().map(|s| s.fqdn).collect())
    }

    fn def_site_path(&self, fqdn: &str) -> Result<Option<String>, RagError> {
        let symbol =
            query::symbol_by_fqdn(self.handle, fqdn).map_err(|e| RagError::InvalidStoredData {
                detail: format!("symbol_by_fqdn({fqdn}): {e}"),
            })?;
        Ok(symbol.map(|s| s.location.file))
    }
}
