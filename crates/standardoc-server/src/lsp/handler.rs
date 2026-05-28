use std::sync::{Arc, Mutex, RwLock};

use standardoc_core::{
    IndexHandle, LanguageProvider, ScanFilters, WatcherHandle,
    query::{self, SymbolContext},
    spawn_watcher,
};
use standardoc_ir::{Kind, RawSymbol, Signature, SymbolLocation};
use tower_lsp_server::{
    Client, LanguageServer,
    jsonrpc::{Error as JsonRpcError, Result as JsonRpcResult},
    ls_types::{
        DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
        GotoDefinitionResponse, Hover, HoverContents, HoverParams, InitializeParams,
        InitializeResult, InitializedParams, Location, MarkupContent, MarkupKind, MessageType,
        OneOf, Position, Range, ReferenceParams, ServerCapabilities, ServerInfo, SymbolInformation,
        SymbolKind, TextDocumentSyncCapability, TextDocumentSyncKind, WorkspaceSymbolParams,
        WorkspaceSymbolResponse,
    },
};

use crate::ServerError;
use crate::lsp::paths::{uri_to_workspace_path, workspace_path_to_uri};
use crate::lsp::progress::run_cold_start_with_progress;

const WORKSPACE_SEARCH_LIMIT: usize = 100;

/// Dispatch target for `tower_lsp_server::LspService`.
///
/// Holds:
/// - `client`: push channel for `publishDiagnostics` + `$/progress`
/// - `handle`: query / submit gateway into the index
/// - `provider`: workspace `LanguageProvider`, shared with the watcher
/// - `filters`: live `ScanFilters`, hot-swapped by the watcher when
///   `standardoc.sxd` changes (lock pause-exclude-22 §1.1 multi-matchers stack)
pub struct StandardocLsp {
    client: Client,
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
    /// Filled by `initialized` once cold start completes; dropped on
    /// `shutdown` so the dispatch thread joins before stdio closes.
    watcher: Arc<Mutex<Option<WatcherHandle>>>,
}

impl StandardocLsp {
    pub(crate) fn new(
        client: Client,
        handle: IndexHandle,
        provider: Arc<dyn LanguageProvider>,
        filters: Arc<RwLock<ScanFilters>>,
    ) -> Self {
        Self {
            client,
            handle,
            provider,
            filters,
            watcher: Arc::new(Mutex::new(None)),
        }
    }
}

impl LanguageServer for StandardocLsp {
    async fn initialize(&self, _params: InitializeParams) -> JsonRpcResult<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "standardoc".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::NONE,
                )),
                hover_provider: Some(true.into()),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        let handle = self.handle.clone();
        let provider = Arc::clone(&self.provider);
        let filters = Arc::clone(&self.filters);
        let client = self.client.clone();
        let watcher_slot = Arc::clone(&self.watcher);

        tokio::spawn(async move {
            if let Err(e) = run_cold_start_with_progress(
                &handle,
                Arc::clone(&provider),
                Arc::clone(&filters),
                client.clone(),
            )
            .await
            {
                client
                    .log_message(MessageType::ERROR, format!("cold start failed: {e}"))
                    .await;
                return;
            }
            match spawn_watcher(handle, provider, filters) {
                Ok(w) => {
                    if let Ok(mut g) = watcher_slot.lock() {
                        *g = Some(w);
                    }
                }
                Err(e) => {
                    client
                        .log_message(MessageType::ERROR, format!("watcher boot failed: {e}"))
                        .await;
                }
            }
        });
    }

    async fn shutdown(&self) -> JsonRpcResult<()> {
        if let Ok(mut g) = self.watcher.lock() {
            *g = None;
        }
        Ok(())
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> JsonRpcResult<Option<GotoDefinitionResponse>> {
        let pos = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let Some(rel) = uri_to_workspace_path(&uri, self.handle.workspace_root()) else {
            return Ok(None);
        };

        let handle = self.handle.clone();
        let workspace_root = self.handle.workspace_root().to_path_buf();
        let sym = tokio::task::spawn_blocking(move || {
            query::symbol_at_position(&handle, &rel, pos.line + 1, pos.character)
        })
        .await
        .map_err(|e| internal_error(format!("spawn_blocking: {e}")))?
        .map_err(|e| JsonRpcError::from(ServerError::from(e)))?;

        let Some(sym) = sym else { return Ok(None) };
        let Some(loc) = location_for_symbol(&sym, &workspace_root) else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(loc)))
    }

    async fn references(&self, params: ReferenceParams) -> JsonRpcResult<Option<Vec<Location>>> {
        let pos = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let Some(rel) = uri_to_workspace_path(&uri, self.handle.workspace_root()) else {
            return Ok(None);
        };

        let handle = self.handle.clone();
        let workspace_root = self.handle.workspace_root().to_path_buf();
        let edges = tokio::task::spawn_blocking(move || -> Result<_, ServerError> {
            let Some(sym) = query::symbol_at_position(&handle, &rel, pos.line + 1, pos.character)?
            else {
                return Ok(Vec::new());
            };
            Ok(query::edges_to(&handle, &sym.fqdn)?)
        })
        .await
        .map_err(|e| internal_error(format!("spawn_blocking: {e}")))??;

        let mut locations: Vec<Location> = Vec::new();
        for edge in edges {
            for site in edge.sites {
                if let Some(loc) = location_at(
                    &site.file,
                    site.line,
                    site.col,
                    site.line,
                    site.col + 1,
                    &workspace_root,
                ) {
                    locations.push(loc);
                }
            }
        }
        Ok(Some(locations))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> JsonRpcResult<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let Some(rel) = uri_to_workspace_path(&uri, self.handle.workspace_root()) else {
            return Ok(None);
        };

        let handle = self.handle.clone();
        let symbols = tokio::task::spawn_blocking(move || query::symbols_by_file(&handle, &rel))
            .await
            .map_err(|e| internal_error(format!("spawn_blocking: {e}")))?
            .map_err(ServerError::from)?;

        Ok(Some(DocumentSymbolResponse::Nested(nest_document_symbols(
            symbols,
        ))))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> JsonRpcResult<Option<WorkspaceSymbolResponse>> {
        let query_str = params.query;
        let handle = self.handle.clone();
        let workspace_root = self.handle.workspace_root().to_path_buf();

        let symbols = tokio::task::spawn_blocking(move || {
            if query_str.trim().is_empty() {
                Ok(Vec::new())
            } else {
                query::search_text(
                    &handle,
                    &query_str,
                    WORKSPACE_SEARCH_LIMIT,
                    &query::SymbolFilter::default(),
                )
            }
        })
        .await
        .map_err(|e| internal_error(format!("spawn_blocking: {e}")))?
        .map_err(ServerError::from)?;

        let infos: Vec<SymbolInformation> = symbols
            .into_iter()
            .filter_map(|s| symbol_information(&s, &workspace_root))
            .collect();
        Ok(Some(WorkspaceSymbolResponse::Flat(infos)))
    }

    async fn hover(&self, params: HoverParams) -> JsonRpcResult<Option<Hover>> {
        let pos = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let Some(rel) = uri_to_workspace_path(&uri, self.handle.workspace_root()) else {
            return Ok(None);
        };

        let handle = self.handle.clone();
        let ctx = tokio::task::spawn_blocking(move || -> Result<_, ServerError> {
            let Some(sym) = query::symbol_at_position(&handle, &rel, pos.line + 1, pos.character)?
            else {
                return Ok(None);
            };
            Ok(query::context_for_symbol(&handle, &sym.fqdn)?)
        })
        .await
        .map_err(|e| internal_error(format!("spawn_blocking: {e}")))??;

        let Some(ctx) = ctx else { return Ok(None) };
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: render_hover_markdown(&ctx),
            }),
            range: Some(range_from_location(&ctx.symbol.location)),
        }))
    }
}

fn internal_error(message: String) -> JsonRpcError {
    let mut err = JsonRpcError::internal_error();
    err.message = message.into();
    err
}

const fn to_lsp_position(db_line: u32, db_col: u32) -> Position {
    // DB stores `start_line` 1-indexed (proc-macro2 convention). LSP `Position.line`
    // is 0-indexed. `character` is already 0-indexed in both worlds.
    Position {
        line: db_line.saturating_sub(1),
        character: db_col,
    }
}

const fn range_from_location(loc: &SymbolLocation) -> Range {
    Range {
        start: to_lsp_position(loc.start_line, loc.start_col),
        end: to_lsp_position(loc.end_line, loc.end_col),
    }
}

fn location_for_symbol(sym: &RawSymbol, workspace_root: &std::path::Path) -> Option<Location> {
    let uri = workspace_path_to_uri(&sym.location.file, workspace_root)?;
    Some(Location {
        uri,
        range: range_from_location(&sym.location),
    })
}

fn location_at(
    file: &str,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
    workspace_root: &std::path::Path,
) -> Option<Location> {
    let uri = workspace_path_to_uri(file, workspace_root)?;
    Some(Location {
        uri,
        range: Range {
            start: to_lsp_position(start_line, start_col),
            end: to_lsp_position(end_line, end_col),
        },
    })
}

const fn kind_to_lsp(kind: Kind) -> SymbolKind {
    match kind {
        Kind::Callable => SymbolKind::FUNCTION,
        Kind::Type => SymbolKind::CLASS,
        Kind::Value => SymbolKind::VARIABLE,
        Kind::Module => SymbolKind::MODULE,
        Kind::Macro => SymbolKind::OPERATOR,
    }
}

#[allow(deprecated)]
fn symbol_information(
    sym: &RawSymbol,
    workspace_root: &std::path::Path,
) -> Option<SymbolInformation> {
    let location = location_for_symbol(sym, workspace_root)?;
    Some(SymbolInformation {
        name: sym.fqdn.clone(),
        kind: kind_to_lsp(sym.kind),
        tags: None,
        deprecated: None,
        location,
        container_name: sym.module.clone(),
    })
}

#[allow(deprecated)]
fn make_doc_symbol(sym: &RawSymbol, children: Vec<DocumentSymbol>) -> DocumentSymbol {
    let range = range_from_location(&sym.location);
    DocumentSymbol {
        name: sym.name.clone(),
        detail: sym.signature.as_ref().map(render_signature_inline),
        kind: kind_to_lsp(sym.kind),
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
    }
}

/// Build a nested tree from the flat `symbols_by_file` result.
///
/// Sort by `(start, -end)` so a parent symbol is always seen before its
/// children. A stack tracks the current ancestor chain: while the top of the
/// stack does not contain the current symbol it is finalised and attached to
/// either its own parent or the root list.
fn nest_document_symbols(mut symbols: Vec<RawSymbol>) -> Vec<DocumentSymbol> {
    symbols.sort_by(|a, b| {
        let al = &a.location;
        let bl = &b.location;
        al.start_line
            .cmp(&bl.start_line)
            .then_with(|| al.start_col.cmp(&bl.start_col))
            .then_with(|| bl.end_line.cmp(&al.end_line))
            .then_with(|| bl.end_col.cmp(&al.end_col))
    });

    let n = symbols.len();
    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut roots: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for i in 0..n {
        while let Some(&top) = stack.last() {
            if range_contains(&symbols[top].location, &symbols[i].location) {
                break;
            }
            stack.pop();
        }
        match stack.last() {
            Some(&top) => children_of[top].push(i),
            None => roots.push(i),
        }
        stack.push(i);
    }

    build_subtree(&roots, &symbols, &children_of)
}

fn build_subtree(
    indices: &[usize],
    flat: &[RawSymbol],
    children_of: &[Vec<usize>],
) -> Vec<DocumentSymbol> {
    indices
        .iter()
        .map(|&i| {
            let kids = build_subtree(&children_of[i], flat, children_of);
            make_doc_symbol(&flat[i], kids)
        })
        .collect()
}

const fn range_contains(outer: &SymbolLocation, inner: &SymbolLocation) -> bool {
    let starts_before_or_eq = outer.start_line < inner.start_line
        || (outer.start_line == inner.start_line && outer.start_col <= inner.start_col);
    let ends_after_or_eq = outer.end_line > inner.end_line
        || (outer.end_line == inner.end_line && outer.end_col >= inner.end_col);
    starts_before_or_eq && ends_after_or_eq
}

fn render_signature_inline(sig: &Signature) -> String {
    let mut buf = String::new();
    if sig.modifiers.is_async {
        buf.push_str("async ");
    }
    buf.push('(');
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&p.name);
        buf.push_str(": ");
        buf.push_str(&p.ty.display);
    }
    buf.push(')');
    if let Some(ret) = &sig.returns {
        buf.push_str(" -> ");
        buf.push_str(&ret.display);
    }
    buf
}

fn render_hover_markdown(ctx: &SymbolContext) -> String {
    let mut out = String::new();
    out.push_str("```rust\n");
    out.push_str(&ctx.symbol.fqdn);
    if let Some(sig) = &ctx.symbol.signature {
        out.push_str(&render_signature_inline(sig));
    }
    out.push_str("\n```");

    if let Some(desc) = &ctx.document_description {
        out.push_str("\n\n");
        out.push_str(desc);
    }
    if let Some(desc) = &ctx.enrichment_description {
        if ctx.document_description.is_none() {
            out.push_str("\n\n");
        } else {
            out.push_str("\n\n---\n\n");
        }
        out.push_str("_inferred_: ");
        out.push_str(desc);
    }
    out
}

#[cfg(test)]
mod tests;
