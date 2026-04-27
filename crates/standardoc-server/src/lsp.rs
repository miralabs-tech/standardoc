//! LSP server — provides `VSCode` / Helix / Neovim / Zed with:
//!
//! - **Completions** on `{{ @doc.…` — proposes available keys
//! - **Hover** on a `@doc.KEY` reference — label + description + signature
//! - **Goto-definition** on `@doc.KEY` → source file of the symbol
//! - **References** on `@doc.KEY` — all cross-file occurrences
//! - **`Workspace`/document symbol** — all blocks for Ctrl+T, `.md` outline
//! - **Diagnostics** pushed from the validator on open + change + rescan
//!
//! Architecture:
//! - Built on `tower-lsp` (tokio runtime)
//! - **Reuses `ServerState`** — index, watcher, config, diagnostics: the
//!   same building block as the MCP. That's the core design choice.
//! - Open `.md` document state is held separately (`HashMap`<Url, String>)
//!   because the LSP needs the in-memory content to parse positions at
//!   completion / hover time.
//! - A **background task** subscribes to the `IndexEvent` broadcast from
//!   `ServerState` to re-push diagnostics for every open `.md` on each
//!   watcher rescan — otherwise squigglies go stale the moment the user
//!   edits a `.rs` somewhere else.

#![allow(
    clippy::needless_pass_by_value,
    clippy::significant_drop_tightening,
    clippy::significant_drop_in_scrutinee,
    clippy::format_push_string
)]

use crate::state::SharedStdout;
use standardoc_core::validator::validate;
use standardoc_web::state::IndexEvent;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic as LspDiagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
    Location, MarkedString, MessageType, OneOf, Position, PrepareRenameResponse, Range,
    ReferenceParams, RenameOptions, RenameParams, SemanticToken, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities, ServerInfo,
    SymbolInformation, SymbolKind as LspSymbolKind, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Url, WorkspaceEdit,
    WorkspaceSymbolParams,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

pub(crate) async fn run(workspace: PathBuf) -> Result<(), String> {
    // Important: `ServerState::boot` starts the watcher (sync thread). We
    // instantiate it outside the LSP handlers' tokio runtime — they just
    // read it through `Arc`. The dummy `SharedStdout`: the LSP doesn't use
    // stdout for cross-channel notifications — it has its own tower-lsp
    // transport.
    let dummy_stdout: SharedStdout = Arc::new(std::sync::Mutex::new(std::io::stdout()));
    let state = Arc::new(
        crate::state::ServerState::boot(&workspace, &dummy_stdout)
            .map_err(|err| format!("LSP boot failed: {err}"))?,
    );
    let documents: Arc<RwLock<HashMap<Url, String>>> = Arc::new(RwLock::new(HashMap::new()));

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let state_for_factory = Arc::clone(&state);
    let documents_for_factory = Arc::clone(&documents);

    let (service, socket) = LspService::new(move |client| {
        // Background task: on each `IndexChanged` (full or incremental
        // rescan from the watcher), we re-push diagnostics for every open
        // `.md`. Without this the squigglies stay pinned to the old index
        // — the user edits a `.rs`, the worker re-scans, but the open `.md`
        // doesn't know its references just moved.
        let bg_client = client.clone();
        let bg_state = Arc::clone(&state_for_factory);
        let bg_documents = Arc::clone(&documents_for_factory);
        let mut events = state_for_factory.subscribe_events();
        tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                if !matches!(event, IndexEvent::IndexChanged { .. }) {
                    continue;
                }
                let uris: Vec<Url> = bg_documents.read().await.keys().cloned().collect();
                for uri in uris {
                    publish_diagnostics_for(&bg_client, &bg_state, uri).await;
                }
            }
        });

        Backend {
            client,
            state: Arc::clone(&state_for_factory),
            documents: Arc::clone(&documents_for_factory),
        }
    });
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}

struct Backend {
    client: Client,
    state: Arc<crate::state::ServerState>,
    documents: Arc<RwLock<HashMap<Url, String>>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> LspResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_owned(), ":".to_owned()]),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: SEMANTIC_TOKEN_TYPES.to_vec(),
                                token_modifiers: vec![],
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: None,
                            ..Default::default()
                        },
                    ),
                ),
                code_action_provider: Some(
                    tower_lsp::lsp_types::CodeActionProviderCapability::Simple(true),
                ),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options:
                        tower_lsp::lsp_types::WorkDoneProgressOptions::default(),
                })),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "standardoc".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "standardoc LSP server initialized")
            .await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text;
        self.documents.write().await.insert(uri.clone(), text);
        self.publish_diagnostics(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        // FULL sync: there is a single change containing full content.
        if let Some(change) = params.content_changes.into_iter().next() {
            self.documents
                .write()
                .await
                .insert(uri.clone(), change.text);
        }
        self.publish_diagnostics(uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.write().await.remove(&uri);
        // Clear diagnostics for the closed file.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let documents = self.documents.read().await;
        let Some(text) = documents.get(&uri) else {
            return Ok(None);
        };
        let line = line_at(text, position.line as usize).unwrap_or("");
        let col = position.character as usize;
        let prefix_in_line = &line[..col.min(line.len())];

        // We trigger completions when the prefix contains `@doc.` somewhere
        // inside the current `{{ ... }}` expression. Light heuristic.
        let Some(at_doc_start) = prefix_in_line.rfind("@doc.") else {
            return Ok(None);
        };
        let after_at_doc = &prefix_in_line[at_doc_start + "@doc.".len()..];
        // Stop if we already passed a `:` (we're in the accessor, not the key).
        if after_at_doc.contains(':') {
            return Ok(None);
        }

        let typed_key_prefix = after_at_doc;
        // Match key prefix first, then fall back to label prefix. Lets the
        // user type a short name (e.g. `LanguageProvider`) and still get
        // the full FQN key inserted.
        let lc_prefix = typed_key_prefix.to_ascii_lowercase();
        let items: Vec<CompletionItem> = {
            let idx = self.state.index();
            idx.blocks
                .iter()
                .filter(|(k, b)| {
                    typed_key_prefix.is_empty()
                        || k.to_ascii_lowercase().starts_with(&lc_prefix)
                        || b.label.to_ascii_lowercase().starts_with(&lc_prefix)
                        || k.split('.')
                            .any(|seg| seg.to_ascii_lowercase().starts_with(&lc_prefix))
                })
                .take(100)
                .map(|(key, block)| CompletionItem {
                    label: key.clone(),
                    kind: Some(CompletionItemKind::REFERENCE),
                    detail: block
                        .symbol
                        .as_ref()
                        .map(|s| s.signature.clone())
                        .or_else(|| Some(block.label.clone())),
                    documentation: first_description(block)
                        .map(tower_lsp::lsp_types::Documentation::String),
                    ..Default::default()
                })
                .collect()
        };
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> LspResult<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let documents = self.documents.read().await;
        let Some(text) = documents.get(&uri) else {
            return Ok(None);
        };
        let Some(key) = key_at_position(text, position) else {
            return Ok(None);
        };
        // Direct lookup, then fallback by label or FQN-suffix match.
        let md = {
            let idx = self.state.index();
            let block = idx
                .blocks
                .get(&key)
                .or_else(|| idx.blocks.values().find(|b| b.label == key))
                .or_else(|| {
                    idx.blocks
                        .values()
                        .find(|b| b.key.as_str().ends_with(&format!(".{key}")))
                });
            let Some(block) = block else {
                return Ok(None);
            };
            let actual_key = block.key.as_str();
            let mut md = format!("**{}** — `{}`", block.label, actual_key);
            if let Some(symbol) = &block.symbol {
                md.push_str(&format!("\n\n```\n{}\n```", symbol.signature));
            }
            if let Some(desc) = first_description(block) {
                md.push_str("\n\n");
                md.push_str(&desc);
            }
            md.push_str(&format!(
                "\n\n_source: `{}:{}`_",
                block.meta.path.display(),
                block.meta.line_start,
            ));
            md
        };

        Ok(Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String(md)),
            range: None,
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> LspResult<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let documents = self.documents.read().await;
        let Some(text) = documents.get(&uri) else {
            return Ok(None);
        };
        let Some(key) = key_at_position(text, position) else {
            return Ok(None);
        };
        let (target_path, line) = {
            let idx = self.state.index();
            let block = idx
                .blocks
                .get(&key)
                .or_else(|| idx.blocks.values().find(|b| b.label == key))
                .or_else(|| {
                    idx.blocks
                        .values()
                        .find(|b| b.key.as_str().ends_with(&format!(".{key}")))
                });
            let Some(block) = block else {
                return Ok(None);
            };
            (
                self.state.workspace_root_join(&block.meta.path),
                block.meta.line_start.saturating_sub(1),
            )
        };
        let Ok(target_uri) = Url::from_file_path(&target_path) else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
            range: Range {
                start: Position { line, character: 0 },
                end: Position { line, character: 0 },
            },
        })))
    }

    /// Find all `@doc.KEY` (and `@docs.module(KEY)`) references across open
    /// `.md` documents and on-disk narrative pages. KEY resolution at the
    /// caret reuses the same logic as `hover` / `goto_definition` — one
    /// source of truth for matching a KEY.
    async fn references(&self, params: ReferenceParams) -> LspResult<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let target_key = {
            let documents = self.documents.read().await;
            let Some(text) = documents.get(&uri) else {
                return Ok(None);
            };
            let Some(key) = key_at_position(text, position) else {
                return Ok(None);
            };
            // Resolve via the index to normalize short-names → FQN. That
            // way references find both `@doc.ParseError` and
            // `@doc.matchigo.parser.ParseError` whether the user clicked
            // on one or the other.
            let idx = self.state.index();
            let resolved = idx
                .blocks
                .get(&key)
                .or_else(|| idx.blocks.values().find(|b| b.label == key))
                .or_else(|| {
                    idx.blocks
                        .values()
                        .find(|b| b.key.as_str().ends_with(&format!(".{key}")))
                })
                .map(|b| b.key.as_str().to_owned())
                .unwrap_or(key);
            resolved
        };

        // Acceptable match aliases: the canonical FQN + the short name
        // (last segment) — the user may write `@doc.ParseError` even if
        // the FQN is `matchigo.parser.ParseError`.
        let short = target_key
            .rsplit_once('.')
            .map_or_else(|| target_key.clone(), |(_, s)| s.to_owned());
        let mut needles: Vec<String> = vec![target_key.clone()];
        if short != target_key {
            needles.push(short);
        }

        let mut locations = Vec::new();
        let mut seen_uris: std::collections::HashSet<Url> = std::collections::HashSet::new();

        // 1. Open documents (freshest source — what the user sees on screen,
        //    not yet saved to disk).
        {
            let docs = self.documents.read().await;
            for (doc_uri, content) in docs.iter() {
                seen_uris.insert(doc_uri.clone());
                push_key_refs(&mut locations, doc_uri, content, &needles);
            }
        }

        // 2. On-disk narrative pages (index for the `.md` not currently open).
        let workspace_root = self.state.workspace_root().to_path_buf();
        let pages: Vec<(PathBuf, String)> = {
            let idx = self.state.index();
            idx.pages
                .values()
                .map(|p| (workspace_root.join(&p.path), p.raw_body.clone()))
                .collect()
        };
        for (abs_path, body) in pages {
            let Ok(page_uri) = Url::from_file_path(&abs_path) else {
                continue;
            };
            if seen_uris.contains(&page_uri) {
                continue;
            }
            push_key_refs(&mut locations, &page_uri, &body, &needles);
        }

        Ok(Some(locations))
    }

    /// Outline of the open `.md`: markdown headings + `@doc.KEY` references
    /// in the document. Lets editors populate the breadcrumb and the
    /// "Outline" view without needing a full client-side markdown parser.
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> LspResult<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let documents = self.documents.read().await;
        let Some(text) = documents.get(&uri) else {
            return Ok(None);
        };

        let mut symbols: Vec<DocumentSymbol> = Vec::new();
        for (line_idx, line) in text.lines().enumerate() {
            let line_u32 = u32::try_from(line_idx).unwrap_or(u32::MAX);
            let line_len = u32::try_from(line.len()).unwrap_or(u32::MAX);

            // Markdown headings: `#`, `##`, … up to `######`. We ignore
            // `#` not followed by a space (likely a tag).
            let trimmed = line.trim_start();
            let level = trimmed.bytes().take_while(|&b| b == b'#').count();
            if (1..=6).contains(&level) {
                let rest = &trimmed[level..];
                if rest.starts_with(' ') {
                    let title = rest.trim().to_owned();
                    if !title.is_empty() {
                        symbols.push(make_doc_symbol(
                            title,
                            Some(format!("H{level}")),
                            LspSymbolKind::STRING,
                            line_u32,
                            line_len,
                        ));
                        continue;
                    }
                }
            }

            // `@doc.KEY` references on the line — helpful to navigate a
            // large template with many inclusions.
            let mut search_from = 0;
            while let Some(rel) = line[search_from..].find("@doc.") {
                let absolute = search_from + rel + "@doc.".len();
                let after = &line[absolute..];
                let end = after
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_')
                    .unwrap_or(after.len());
                if end == 0 {
                    search_from = absolute;
                    continue;
                }
                let key = &after[..end];
                symbols.push(make_doc_symbol(
                    format!("@doc.{key}"),
                    None,
                    LspSymbolKind::KEY,
                    line_u32,
                    line_len,
                ));
                search_from = absolute + end;
            }
        }

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    /// Pre-rename validation. The client calls this **before** `rename`
    /// to check that the target position is renameable and to grab the
    /// exact token range. Returning `None` disables the rename menu;
    /// returning `Range` lets the client show the prompt with the current
    /// name pre-selected.
    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> LspResult<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;
        let documents = self.documents.read().await;
        let Some(text) = documents.get(&uri) else {
            return Ok(None);
        };
        let Some((key, range)) = key_at_position_with_range(text, position) else {
            return Ok(None);
        };
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range,
            placeholder: key,
        }))
    }

    /// Rename of a `@doc.KEY` across every `.md` in the workspace — open
    /// documents and on-disk pages. We also patch the `@<doc_tag> <key>`
    /// annotation in the source file (`.rs`/`.ts`/…) when the block is
    /// `Annotated` / `Hybrid`. `Inferred` blocks have no source tag to
    /// patch — for those, the user should rename the underlying AST symbol
    /// (rust-analyzer / tsserver job, not standardoc's).
    async fn rename(&self, params: RenameParams) -> LspResult<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        if !is_valid_doc_key(&new_name) {
            return Err(tower_lsp::jsonrpc::Error::invalid_params(format!(
                "'{new_name}' is not a valid DocKey (alphanumeric / dot / underscore only)"
            )));
        }

        let target_key = {
            let documents = self.documents.read().await;
            let Some(text) = documents.get(&uri) else {
                return Ok(None);
            };
            let Some(key) = key_at_position(text, position) else {
                return Ok(None);
            };
            // Same as in `references`: resolve to the canonical FQN so we
            // match consistently everywhere, whether the user clicked on
            // a short name or the FQN.
            let idx = self.state.index();
            idx.blocks
                .get(&key)
                .or_else(|| idx.blocks.values().find(|b| b.label == key))
                .or_else(|| {
                    idx.blocks
                        .values()
                        .find(|b| b.key.as_str().ends_with(&format!(".{key}")))
                })
                .map(|b| b.key.as_str().to_owned())
                .unwrap_or(key)
        };

        let short = target_key
            .rsplit_once('.')
            .map_or_else(|| target_key.clone(), |(_, s)| s.to_owned());
        let mut needles: Vec<String> = vec![target_key.clone()];
        if short != target_key {
            needles.push(short);
        }

        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        let mut seen_uris: std::collections::HashSet<Url> = std::collections::HashSet::new();

        // Open documents first — their in-memory content may differ from
        // disk, we must edit that version.
        {
            let docs = self.documents.read().await;
            for (doc_uri, content) in docs.iter() {
                seen_uris.insert(doc_uri.clone());
                let mut locations = Vec::new();
                push_key_refs(&mut locations, doc_uri, content, &needles);
                if !locations.is_empty() {
                    changes
                        .entry(doc_uri.clone())
                        .or_default()
                        .extend(locations.into_iter().map(|loc| TextEdit {
                            range: loc.range,
                            new_text: new_name.clone(),
                        }));
                }
            }
        }

        // On-disk pages not yet opened.
        let workspace_root = self.state.workspace_root().to_path_buf();
        let pages: Vec<(PathBuf, String)> = {
            let idx = self.state.index();
            idx.pages
                .values()
                .map(|p| (workspace_root.join(&p.path), p.raw_body.clone()))
                .collect()
        };
        for (abs_path, body) in pages {
            let Ok(page_uri) = Url::from_file_path(&abs_path) else {
                continue;
            };
            if seen_uris.contains(&page_uri) {
                continue;
            }
            let mut locations = Vec::new();
            push_key_refs(&mut locations, &page_uri, &body, &needles);
            if !locations.is_empty() {
                changes
                    .entry(page_uri)
                    .or_default()
                    .extend(locations.into_iter().map(|loc| TextEdit {
                        range: loc.range,
                        new_text: new_name.clone(),
                    }));
            }
        }

        // Source files: if the `target_key` matches an annotated block
        // (not Inferred), patch the `@<doc_tag> <oldkey>` in its comment.
        // Inferred blocks have no explicit tag to rename — for those, the
        // user should rename the AST symbol (rust-analyzer / tsserver
        // job, not standardoc's).
        for (uri, edit) in self.collect_source_rename_edits(&target_key, &new_name) {
            changes.entry(uri).or_default().push(edit);
        }

        if changes.is_empty() {
            return Ok(None);
        }

        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }))
    }

    /// Semantic tokens for `.md`: colorize DSL expressions `{{ … }}`
    /// (`@doc.KEY` references, keywords `each`/`if`/`else`, function calls
    /// `:func()`, aliases). Without this the editor can't tell DSL apart
    /// from raw markdown.
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> LspResult<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let documents = self.documents.read().await;
        let Some(text) = documents.get(&uri) else {
            return Ok(None);
        };
        let tokens = build_semantic_tokens(text);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }

    /// Code actions available at a position. We support two flows:
    /// - On a source file (`.rs`/`.ts`/`.py`/…) — if the position falls
    ///   inside a block without an explicit `@doc` (origin = Inferred),
    ///   we propose "Insert @doc skeleton" which generates a doc-comment
    ///   with `@param` pre-filled from the AST signature.
    /// - Quick fixes attached to diagnostics passed by the client
    ///   (`STD003`, `STD005`) — we insert a placeholder description.
    async fn code_action(&self, params: CodeActionParams) -> LspResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();

        // 1. Quick fixes for standardoc diagnostics (STD003 = param missing
        //    description; STD005 = block without description).
        for diag in &params.context.diagnostics {
            if diag.source.as_deref() != Some("standardoc") {
                continue;
            }
            let code = match &diag.code {
                Some(tower_lsp::lsp_types::NumberOrString::String(s)) => s.as_str(),
                _ => continue,
            };
            if let Some(action) = build_quick_fix_for_diag(&uri, diag, code) {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        // 2. Insert `@doc` skeleton if we're inside a source file and the
        //    position matches an undocumented symbol.
        if let Some(action) = self.build_insert_doc_skeleton(&uri, range) {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }

    /// Workspace symbols: every block in the index, filtered by a
    /// case-insensitive substring query. Targets `VSCode`'s Ctrl+T or
    /// Neovim's `:Telescope` — the user types `Pars` and gets every key
    /// that contains `pars` somewhere (key, label, or segment).
    #[allow(deprecated)]
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> LspResult<Option<Vec<SymbolInformation>>> {
        let query = params.query.to_ascii_lowercase();
        let workspace_root = self.state.workspace_root().to_path_buf();
        let symbols: Vec<SymbolInformation> = {
            let idx = self.state.index();
            idx.blocks
                .iter()
                .filter(|(k, b)| {
                    query.is_empty()
                        || k.to_ascii_lowercase().contains(&query)
                        || b.label.to_ascii_lowercase().contains(&query)
                })
                .take(500)
                .filter_map(|(key, block)| {
                    let abs = workspace_root.join(&block.meta.path);
                    let target_uri = Url::from_file_path(&abs).ok()?;
                    let line = block.meta.line_start.saturating_sub(1);
                    Some(SymbolInformation {
                        name: key.clone(),
                        kind: lsp_kind_for(block),
                        tags: None,
                        deprecated: None,
                        location: Location {
                            uri: target_uri,
                            range: Range {
                                start: Position { line, character: 0 },
                                end: Position { line, character: 0 },
                            },
                        },
                        container_name: key.rsplit_once('.').map(|(parent, _)| parent.to_owned()),
                    })
                })
                .collect()
        };
        Ok(Some(symbols))
    }
}

impl Backend {
    /// Instance wrapper — delegates to the free function so we share code
    /// with the background invalidation task at rescan time.
    async fn publish_diagnostics(&self, doc_uri: Url) {
        publish_diagnostics_for(&self.client, &self.state, doc_uri).await;
    }

    /// For every block whose `key` matches `target_key`, if its origin is
    /// `Annotated` or `Hybrid` (an explicit `@doc` exists in the source),
    /// reads the source file, walks the lines above the symbol to find the
    /// `@<doc_tag> <target_key>` line, and emits a `TextEdit` that
    /// replaces `target_key` with `new_name`.
    ///
    /// Silently skips when:
    /// - no block matches (renaming a phantom key)
    /// - the block is `Inferred` (no tag to patch)
    /// - the source file is unreadable
    /// - the `@<doc_tag>` can't be found in the comment lines
    ///
    /// We accept **not covering everything** rather than making a wrong
    /// replacement — it's better than touching code by mistake.
    fn collect_source_rename_edits(
        &self,
        target_key: &str,
        new_name: &str,
    ) -> Vec<(Url, TextEdit)> {
        let workspace_root = self.state.workspace_root().to_path_buf();
        let doc_tag = self.state.config().doc_tag.clone();

        // Snapshot block info outside the lock — we don't hold a guard
        // across file I/O.
        let candidates: Vec<(PathBuf, u32)> = {
            let idx = self.state.index();
            idx.blocks
                .values()
                .filter(|b| b.key.as_str() == target_key)
                .filter(|b| !matches!(b.origin, standardoc_core::model::BlockOrigin::Inferred))
                .map(|b| (workspace_root.join(&b.meta.path), b.meta.line_start))
                .collect()
        };

        let mut out = Vec::new();
        for (abs_path, line_start) in candidates {
            let Ok(content) = std::fs::read_to_string(&abs_path) else {
                eprintln!(
                    "rename: skip source patch — cannot read {}",
                    abs_path.display()
                );
                continue;
            };
            let Some((line_idx, col_start)) =
                find_doc_tag_line(&content, line_start, &doc_tag, target_key)
            else {
                eprintln!(
                    "rename: skip source patch — `@{doc_tag} {target_key}` not found above line {line_start} in {}",
                    abs_path.display()
                );
                continue;
            };
            let Ok(uri) = Url::from_file_path(&abs_path) else {
                continue;
            };
            let line_u32 = u32::try_from(line_idx).unwrap_or(u32::MAX);
            let col_u32 = u32::try_from(col_start).unwrap_or(u32::MAX);
            let key_len = u32::try_from(target_key.len()).unwrap_or(u32::MAX);
            out.push((
                uri,
                TextEdit {
                    range: Range {
                        start: Position {
                            line: line_u32,
                            character: col_u32,
                        },
                        end: Position {
                            line: line_u32,
                            character: col_u32 + key_len,
                        },
                    },
                    new_text: new_name.to_owned(),
                },
            ));
        }
        out
    }

    /// Builds the "Insert @doc skeleton" action if the position targets
    /// an inferred block (no explicit doc) in the source file pointed at
    /// by `uri`. Returns `None` when:
    /// - the URI is not inside the workspace
    /// - no block covers this line
    /// - the block is already annotated (`@doc` explicit)
    ///
    /// The skeleton respects the comment convention of the target language
    /// (Rust `///`, TS `/** */`, Python `"""`).
    fn build_insert_doc_skeleton(&self, uri: &Url, range: Range) -> Option<CodeAction> {
        let abs_path = uri.to_file_path().ok()?;
        let workspace_root = self.state.workspace_root().to_path_buf();
        let relative = abs_path.strip_prefix(&workspace_root).ok()?.to_path_buf();
        let line = range.start.line + 1;

        // Snapshot block info outside the lock — we don't hold a
        // RwLockReadGuard across the subsequent awaits.
        let block_info = {
            let idx = self.state.index();
            let block = idx.blocks.values().find(|b| {
                b.meta.path == relative && line >= b.meta.line_start && line <= b.meta.line_end
            })?;
            // We mainly target inferred blocks (= no source-level @doc).
            // If the user already has an @doc, this action adds nothing
            // useful.
            if block.origin != standardoc_core::model::BlockOrigin::Inferred {
                return None;
            }
            let extension = block.meta.file_ext.clone();
            let line_start = block.meta.line_start;
            let column = block.meta.column;
            let params: Vec<(String, Option<String>)> = block
                .symbol
                .as_ref()
                .map(|s| {
                    s.params
                        .iter()
                        .map(|p| (p.name.clone(), p.type_repr.clone()))
                        .collect()
                })
                .unwrap_or_default();
            (extension, line_start, column, params)
        };
        let (extension, line_start, column, params) = block_info;

        let skeleton = render_doc_skeleton(&extension, &params, column);
        let insert_pos = Position {
            line: line_start.saturating_sub(1),
            character: 0,
        };
        let edit = TextEdit {
            range: Range {
                start: insert_pos,
                end: insert_pos,
            },
            new_text: skeleton,
        };
        let mut changes = HashMap::new();
        changes.insert(uri.clone(), vec![edit]);

        Some(CodeAction {
            title: "Insert @doc skeleton".to_owned(),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: None,
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(true),
            disabled: None,
            data: None,
        })
    }
}

/// Pushes the validator diagnostics for the open document. Note: the
/// validator's diagnostics target **source files** (not the `.md` being
/// edited). We filter on the document path so we only push the diagnostics
/// that concern it.
///
/// Free function (vs `&self` method) because the background task that
/// re-pushes diagnostics on `IndexChanged` doesn't own the `Backend` —
/// it only has cloned `Arc`s to the `Client` and `ServerState`.
///
/// **Important**: the `RwLockReadGuard` from `std::sync::RwLock` isn't
/// `Send`, so we can't hold it across an `await`. We extract the data
/// into owned types **before** dropping the guard.
async fn publish_diagnostics_for(
    client: &Client,
    state: &Arc<crate::state::ServerState>,
    doc_uri: Url,
) {
    let Ok(doc_path) = doc_uri.to_file_path() else {
        return;
    };
    let doc_relative = doc_path
        .strip_prefix(state.workspace_root())
        .unwrap_or(&doc_path)
        .to_path_buf();

    let lsp_diags: Vec<LspDiagnostic> = {
        let idx = state.index();
        let diagnostics = validate(&idx.blocks, &idx.collisions, &idx.pages, state.config());
        diagnostics
            .iter()
            .filter(|d| d.path == doc_relative)
            .map(|d| LspDiagnostic {
                range: Range {
                    start: Position {
                        line: d.range.line_start.saturating_sub(1),
                        character: d.range.column_start.saturating_sub(1),
                    },
                    end: Position {
                        line: d.range.line_end.saturating_sub(1),
                        character: d.range.column_end.saturating_sub(1),
                    },
                },
                severity: Some(map_severity(d.severity)),
                code: Some(tower_lsp::lsp_types::NumberOrString::String(
                    d.code.as_str().to_owned(),
                )),
                source: Some("standardoc".to_owned()),
                message: d.message.clone(),
                ..Default::default()
            })
            .collect()
    };
    client.publish_diagnostics(doc_uri, lsp_diags, None).await;
}

fn line_at(text: &str, line_idx: usize) -> Option<&str> {
    text.lines().nth(line_idx)
}

/// Looks around `position` for a `@doc.KEY` reference and returns the KEY
/// (without `@doc.`, without any `:` accessor that follows). Returns
/// `None` if the position isn't inside a valid reference.
fn key_at_position(text: &str, position: Position) -> Option<String> {
    key_at_position_with_range(text, position).map(|(k, _)| k)
}

/// Variant of `key_at_position` that also returns the exact `Range` of
/// the key token. Used by `prepare_rename` to tell the client which range
/// to highlight in the rename prompt.
fn key_at_position_with_range(text: &str, position: Position) -> Option<(String, Range)> {
    let line = line_at(text, position.line as usize)?;
    let col = (position.character as usize).min(line.len());

    let prefix = &line[..col];
    let key_start = prefix.rfind("@doc.")? + "@doc.".len();

    let after = &line[key_start..];
    let end = after
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_')
        .unwrap_or(after.len());
    let key = &after[..end];
    if key.is_empty() {
        return None;
    }
    let range = Range {
        start: Position {
            line: position.line,
            character: u32::try_from(key_start).unwrap_or(u32::MAX),
        },
        end: Position {
            line: position.line,
            character: u32::try_from(key_start + end).unwrap_or(u32::MAX),
        },
    };
    Some((key.to_owned(), range))
}

/// Walks the lines above `symbol_line` (1-indexed) looking for the
/// pattern `@<doc_tag> <target_key>` (lenient on whitespace and comment
/// prefixes). Returns `(line_idx_0based, col_start_0based)` of the
/// matched `target_key`, or `None` if not found within ~30 lines above
/// the symbol (heuristic: a reasonable comment doesn't span more than
/// that).
///
/// We match by looking for the substring `@<doc_tag>` then checking the
/// immediately-following token (after whitespace) equals `target_key`.
/// Robust to comment prefixes (`///`, `*`, `--`, `#`) because we don't
/// try to parse them — we just look for the tag inside the line text.
fn find_doc_tag_line(
    content: &str,
    symbol_line: u32,
    doc_tag: &str,
    target_key: &str,
) -> Option<(usize, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    let symbol_idx_0based = (symbol_line as usize).saturating_sub(1);
    let scan_start = symbol_idx_0based.saturating_sub(30);
    let needle = format!("@{doc_tag}");

    for line_idx in (scan_start..symbol_idx_0based).rev() {
        let line = lines.get(line_idx)?;
        let Some(at_pos) = line.find(&needle) else {
            continue;
        };
        let after_tag = &line[at_pos + needle.len()..];
        // Require at least one whitespace before the key — avoids
        // matching `@docs` or `@docfile` when we're looking for `@doc`.
        let trimmed = after_tag.trim_start();
        let ws_skipped = after_tag.len() - trimmed.len();
        if ws_skipped == 0 {
            continue;
        }
        // Extract the first token (until the next whitespace).
        let token_end = trimmed
            .find(|c: char| c.is_whitespace())
            .unwrap_or(trimmed.len());
        let token = &trimmed[..token_end];
        if token == target_key {
            let col_start = at_pos + needle.len() + ws_skipped;
            return Some((line_idx, col_start));
        }
    }
    None
}

/// Lexical validation of a `DocKey` name proposed by the client during a
/// rename. We accept alphanumeric + `.` + `_` (same constraints as the
/// `key_at_position` lookup). Rejecting early avoids producing broken
/// refs in `.md` after the rename is applied.
fn is_valid_doc_key(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        && !s.starts_with('.')
        && !s.ends_with('.')
}

fn first_description(block: &standardoc_core::model::DocBlock) -> Option<String> {
    block
        .tags
        .get("description")
        .and_then(|v| v.first())
        .and_then(|fields| fields.first())
        .map(|s| s.replace('\n', " ").trim().to_owned())
        .filter(|s| !s.is_empty())
}

const fn map_severity(s: standardoc_core::model::Severity) -> DiagnosticSeverity {
    use standardoc_core::model::Severity;
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

/// Maps `standardoc_core::SymbolKind` → `lsp_types::SymbolKind`. When a
/// kind has no direct equivalent we fall back to the nearest semantic
/// match (Macro→Function, `TypeAlias`→`TypeParameter`).
const fn lsp_kind_for(block: &standardoc_core::model::DocBlock) -> LspSymbolKind {
    use standardoc_core::model::SymbolKind as Core;
    let Some(symbol) = &block.symbol else {
        return LspSymbolKind::OBJECT;
    };
    match symbol.kind {
        Core::Function | Core::Macro => LspSymbolKind::FUNCTION,
        Core::Method => LspSymbolKind::METHOD,
        Core::Class => LspSymbolKind::CLASS,
        Core::Struct => LspSymbolKind::STRUCT,
        Core::Enum => LspSymbolKind::ENUM,
        Core::Trait | Core::Interface => LspSymbolKind::INTERFACE,
        Core::TypeAlias => LspSymbolKind::TYPE_PARAMETER,
        Core::Const | Core::Static => LspSymbolKind::CONSTANT,
        Core::Module => LspSymbolKind::MODULE,
        Core::Field => LspSymbolKind::FIELD,
        Core::Variant => LspSymbolKind::ENUM_MEMBER,
        Core::Other => LspSymbolKind::OBJECT,
    }
}

/// Finds every `@doc.NEEDLE` and `@docs.module(NEEDLE)` occurrence in
/// `content` and pushes a `Location` per match. NEEDLE = full key OR
/// short name — we accept both to match the common writing patterns (FQN
/// for precision, short name for brevity).
fn push_key_refs(out: &mut Vec<Location>, uri: &Url, content: &str, needles: &[String]) {
    for (line_idx, line) in content.lines().enumerate() {
        let line_u32 = u32::try_from(line_idx).unwrap_or(u32::MAX);
        for needle in needles {
            push_doc_ref_matches(out, uri, line, line_u32, needle);
            push_module_call_matches(out, uri, line, line_u32, needle);
        }
    }
}

/// Matches `@doc.<needle>` with a boundary check: we reject `@doc.Foo`
/// in the middle of `@doc.FooBar` to avoid false positives.
fn push_doc_ref_matches(
    out: &mut Vec<Location>,
    uri: &Url,
    line: &str,
    line_u32: u32,
    needle: &str,
) {
    let pattern = format!("@doc.{needle}");
    let key_offset = "@doc.".len();
    let mut search_from = 0;
    while let Some(rel) = line[search_from..].find(&pattern) {
        let absolute = search_from + rel;
        let after_idx = absolute + pattern.len();
        let is_boundary = line[after_idx..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '.' && c != '_');
        if is_boundary {
            let key_start = u32::try_from(absolute + key_offset).unwrap_or(u32::MAX);
            let key_end = u32::try_from(after_idx).unwrap_or(u32::MAX);
            out.push(Location {
                uri: uri.clone(),
                range: Range {
                    start: Position {
                        line: line_u32,
                        character: key_start,
                    },
                    end: Position {
                        line: line_u32,
                        character: key_end,
                    },
                },
            });
        }
        search_from = absolute + pattern.len();
    }
}

/// Matches `@docs.module(<needle>)`. The mandatory close-paren prevents
/// partial matches like `@docs.module(matchigo.parser` when searching for
/// `matchigo.parser`.
fn push_module_call_matches(
    out: &mut Vec<Location>,
    uri: &Url,
    line: &str,
    line_u32: u32,
    needle: &str,
) {
    let pattern = format!("@docs.module({needle})");
    let key_offset = "@docs.module(".len();
    let mut search_from = 0;
    while let Some(rel) = line[search_from..].find(&pattern) {
        let absolute = search_from + rel;
        let key_start = u32::try_from(absolute + key_offset).unwrap_or(u32::MAX);
        let key_end = u32::try_from(absolute + key_offset + needle.len()).unwrap_or(u32::MAX);
        out.push(Location {
            uri: uri.clone(),
            range: Range {
                start: Position {
                    line: line_u32,
                    character: key_start,
                },
                end: Position {
                    line: line_u32,
                    character: key_end,
                },
            },
        });
        search_from = absolute + pattern.len();
    }
}

/// Builds a flat `DocumentSymbol` (no children). The `selection_range`
/// matches `range` — we don't (yet) try to point at the header
/// separately from the body.
#[allow(deprecated)]
const fn make_doc_symbol(
    name: String,
    detail: Option<String>,
    kind: LspSymbolKind,
    line: u32,
    line_len: u32,
) -> DocumentSymbol {
    let range = Range {
        start: Position { line, character: 0 },
        end: Position {
            line,
            character: line_len,
        },
    };
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    }
}

// -------- Code actions --------

/// Builds the doc-comment block to insert just before a symbol, in the
/// style appropriate for the file's language. Indented `column - 1`
/// spaces to align on the underlying symbol (inline comments inside an
/// `impl` or a `class` are indented with their parent).
fn render_doc_skeleton(
    extension: &str,
    params: &[(String, Option<String>)],
    column: u32,
) -> String {
    let indent: String = " ".repeat(column.saturating_sub(1) as usize);
    let mut out = String::new();
    match extension {
        // Rust: `///` per line — the standard doc-comment style.
        "rs" => {
            out.push_str(&format!("{indent}/// @doc\n"));
            out.push_str(&format!("{indent}/// :description: \n"));
            for (name, ty) in params {
                let ty_str = ty.as_deref().unwrap_or("Type");
                out.push_str(&format!(
                    "{indent}/// :param: {name} {ty_str} description\n"
                ));
            }
        }
        // TypeScript / JavaScript: multi-line JSDoc with `*`.
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => {
            out.push_str(&format!("{indent}/**\n"));
            out.push_str(&format!("{indent} * @doc\n"));
            out.push_str(&format!("{indent} * :description: \n"));
            for (name, ty) in params {
                let ty_str = ty.as_deref().unwrap_or("Type");
                out.push_str(&format!("{indent} * :param: {name} {ty_str} description\n"));
            }
            out.push_str(&format!("{indent} */\n"));
        }
        // Python: triple-quoted docstring, which goes **under** the
        // signature (not above like the others). Python convention: we
        // still generate a pre-formatted option — the user moves it if
        // needed. TODO: handle below-signature insertion properly.
        "py" => {
            out.push_str(&format!("{indent}\"\"\"@doc\n"));
            out.push_str(&format!("{indent}:description: \n"));
            for (name, ty) in params {
                let ty_str = ty.as_deref().unwrap_or("Type");
                out.push_str(&format!("{indent}:param: {name} {ty_str} description\n"));
            }
            out.push_str(&format!("{indent}\"\"\"\n"));
        }
        // Generic fallback: `//` line comment.
        _ => {
            out.push_str(&format!("{indent}// @doc\n"));
            out.push_str(&format!("{indent}// :description: \n"));
            for (name, ty) in params {
                let ty_str = ty.as_deref().unwrap_or("Type");
                out.push_str(&format!("{indent}// :param: {name} {ty_str} description\n"));
            }
        }
    }
    out
}

/// Textual quick fix for a known standardoc diagnostic. We don't touch
/// the file — we just insert a `description` placeholder at the position
/// indicated by the diagnostic. The user edits it afterwards.
fn build_quick_fix_for_diag(uri: &Url, diag: &LspDiagnostic, code: &str) -> Option<CodeAction> {
    let (title, snippet) = match code {
        "STD003" => (
            "Add description for @param".to_owned(),
            " — TODO: describe this parameter".to_owned(),
        ),
        "STD005" => (
            "Add @description for this block".to_owned(),
            // We insert the line after the `@doc` — the client positions
            // the edit just before the diagnostic range to do the right
            // thing.
            "/// :description: TODO\n".to_owned(),
        ),
        _ => return None,
    };
    let edit = TextEdit {
        range: Range {
            start: diag.range.end,
            end: diag.range.end,
        },
        new_text: snippet,
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);

    Some(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(false),
        disabled: None,
        data: None,
    })
}

// -------- Semantic tokens --------

/// Ordered legend of token types we emit. The index in this slice =
/// `tokenType` u32 sent to the client. **Never reorder** without bumping
/// a protocol revision — the client receives the legend at initialize
/// time and uses it to decode.
const SEMANTIC_TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,   // 0 — each / if / else / in / /each / /if
    SemanticTokenType::NAMESPACE, // 1 — @doc / @docs / @meta / @symbol
    SemanticTokenType::PROPERTY,  // 2 — segments after the namespace (.field, .key)
    SemanticTokenType::FUNCTION,  // 3 — :func() / .func() calls
    SemanticTokenType::VARIABLE,  // 4 — alias names introduced by `each`
    SemanticTokenType::STRING,    // 5 — "string literals"
    SemanticTokenType::NUMBER,    // 6 — numeric literals
    SemanticTokenType::OPERATOR,  // 7 — == / != / {{ / }}
];

const TT_KEYWORD: u32 = 0;
const TT_NAMESPACE: u32 = 1;
const TT_PROPERTY: u32 = 2;
const TT_FUNCTION: u32 = 3;
#[allow(dead_code)]
const TT_VARIABLE: u32 = 4;
const TT_STRING: u32 = 5;
const TT_NUMBER: u32 = 6;
const TT_OPERATOR: u32 = 7;

/// Absolute token (line/char from the start of the document). Encoded
/// into deltas after sorting to comply with the LSP format.
#[derive(Debug, Clone, Copy)]
struct AbsoluteToken {
    line: u32,
    char: u32,
    length: u32,
    token_type: u32,
}

/// Scans the `.md` document and emits semantic tokens for DSL
/// expressions `{{ … }}`. Outside DSL blocks we emit nothing — `VSCode`
/// keeps its native markdown rendering.
fn build_semantic_tokens(text: &str) -> Vec<SemanticToken> {
    let mut tokens: Vec<AbsoluteToken> = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        let line_u32 = u32::try_from(line_idx).unwrap_or(u32::MAX);
        scan_line_for_dsl_tokens(line, line_u32, &mut tokens);
    }
    encode_delta_tokens(&mut tokens)
}

/// Tokenizes the `{{ … }}` blocks on a single line. Multi-line is
/// (intentionally) not supported — a DSL block spanning multiple lines
/// will only be partially colored, an acceptable v1 bug.
fn scan_line_for_dsl_tokens(line: &str, line_u32: u32, out: &mut Vec<AbsoluteToken>) {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    while cursor + 1 < bytes.len() {
        // Skip ahead to the next `{{`.
        let Some(rel) = line[cursor..].find("{{") else {
            return;
        };
        let block_start = cursor + rel;
        let inside_start = block_start + 2;
        let Some(close_rel) = line[inside_start..].find("}}") else {
            return; // Multi-line block or unterminated on this line.
        };
        let inside_end = inside_start + close_rel;

        push_operator(out, line_u32, block_start, 2); // {{
        tokenize_dsl_inner(&line[inside_start..inside_end], line_u32, inside_start, out);
        push_operator(out, line_u32, inside_end, 2); // }}

        cursor = inside_end + 2;
    }
}

fn tokenize_dsl_inner(inner: &str, line: u32, inner_offset: usize, out: &mut Vec<AbsoluteToken>) {
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];

        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'"' {
            i = consume_string_literal(bytes, i, line, inner_offset, out);
            continue;
        }
        if c.is_ascii_digit() {
            i = consume_number_literal(bytes, i, line, inner_offset, out);
            continue;
        }
        // Operators (==, !=).
        if (c == b'=' || c == b'!') && bytes.get(i + 1) == Some(&b'=') {
            push_operator(out, line, inner_offset + i, 2);
            i += 2;
            continue;
        }
        // Closing tags `/each`, `/if`.
        if c == b'/' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            push_token(out, line, inner_offset + start, i - start, TT_KEYWORD);
            continue;
        }
        // Namespace references starting with `@` (consume the chain of
        // `.field`, `:func()`, `[idx]` accessors that follow).
        if c == b'@' {
            i = consume_namespace_chain(inner, bytes, i, line, inner_offset, out);
            continue;
        }
        // Identifiers: keywords (each/if/else/in) or alias/func.
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &inner[start..i];
            let token_type = match word {
                "each" | "if" | "else" | "in" => TT_KEYWORD,
                _ => TT_VARIABLE,
            };
            push_token(out, line, inner_offset + start, i - start, token_type);
            continue;
        }
        // Unhandled character (punctuation, parens) — move forward.
        i += 1;
    }
}

fn consume_string_literal(
    bytes: &[u8],
    start: usize,
    line: u32,
    inner_offset: usize,
    out: &mut Vec<AbsoluteToken>,
) -> usize {
    let mut i = start + 1;
    while i < bytes.len() && bytes[i] != b'"' {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
        } else {
            i += 1;
        }
    }
    if i < bytes.len() {
        i += 1; // close quote
    }
    push_token(out, line, inner_offset + start, i - start, TT_STRING);
    i
}

fn consume_number_literal(
    bytes: &[u8],
    start: usize,
    line: u32,
    inner_offset: usize,
    out: &mut Vec<AbsoluteToken>,
) -> usize {
    let mut i = start;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    push_token(out, line, inner_offset + start, i - start, TT_NUMBER);
    i
}

/// Consumes `@namespace` then the chain of accessors `.seg`, `:func`,
/// `[idx]` that follow. Stops at the first opening parenthesis (the inner
/// scan resumes on the argument as a regular expression).
fn consume_namespace_chain(
    _inner: &str,
    bytes: &[u8],
    start: usize,
    line: u32,
    inner_offset: usize,
    out: &mut Vec<AbsoluteToken>,
) -> usize {
    let mut i = start + 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    push_token(out, line, inner_offset + start, i - start, TT_NAMESPACE);
    while i < bytes.len() {
        let nc = bytes[i];
        if nc == b'.' {
            i += 1;
            let seg_start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i > seg_start {
                push_token(
                    out,
                    line,
                    inner_offset + seg_start,
                    i - seg_start,
                    TT_PROPERTY,
                );
            }
        } else if nc == b':' {
            i += 1;
            let fn_start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i > fn_start {
                push_token(
                    out,
                    line,
                    inner_offset + fn_start,
                    i - fn_start,
                    TT_FUNCTION,
                );
            }
        } else if nc == b'[' {
            // Index `[0]` etc. — skip until `]` without tokenizing.
            while i < bytes.len() && bytes[i] != b']' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
        } else {
            // `(`, space, etc.: let main scanner continue.
            break;
        }
    }
    i
}

fn push_token(
    out: &mut Vec<AbsoluteToken>,
    line: u32,
    char_start: usize,
    length: usize,
    token_type: u32,
) {
    out.push(AbsoluteToken {
        line,
        char: u32::try_from(char_start).unwrap_or(u32::MAX),
        length: u32::try_from(length).unwrap_or(u32::MAX),
        token_type,
    });
}

fn push_operator(out: &mut Vec<AbsoluteToken>, line: u32, char_start: usize, length: usize) {
    push_token(out, line, char_start, length, TT_OPERATOR);
}

/// Encodes the absolute tokens into (line, char) deltas per the LSP wire
/// format. Sorts first by (line, char) — the scanner already emits in
/// order, but the sort is a safety net.
fn encode_delta_tokens(tokens: &mut [AbsoluteToken]) -> Vec<SemanticToken> {
    tokens.sort_by(|a, b| a.line.cmp(&b.line).then(a.char.cmp(&b.char)));
    let mut out = Vec::with_capacity(tokens.len());
    let mut prev_line = 0u32;
    let mut prev_char = 0u32;
    for tok in tokens {
        let delta_line = tok.line - prev_line;
        let delta_char = if delta_line == 0 {
            tok.char - prev_char
        } else {
            tok.char
        };
        out.push(SemanticToken {
            delta_line,
            delta_start: delta_char,
            length: tok.length,
            token_type: tok.token_type,
            token_modifiers_bitset: 0,
        });
        prev_line = tok.line;
        prev_char = tok.char;
    }
    out
}

#[cfg(test)]
mod semantic_tokens_tests {
    use super::*;

    /// Decodes the delta-encoded output into absolute `(line, char,
    /// length, type)` tuples so the assertions stay readable.
    fn decode(toks: &[SemanticToken]) -> Vec<(u32, u32, u32, u32)> {
        let mut out = Vec::new();
        let mut line = 0u32;
        let mut col = 0u32;
        for t in toks {
            if t.delta_line == 0 {
                col += t.delta_start;
            } else {
                line += t.delta_line;
                col = t.delta_start;
            }
            out.push((line, col, t.length, t.token_type));
        }
        out
    }

    #[test]
    fn no_dsl_means_no_tokens() {
        let toks = build_semantic_tokens("# Just markdown\n\nNo DSL here.");
        assert!(toks.is_empty());
    }

    #[test]
    fn simple_doc_ref_tokenized() {
        let toks = build_semantic_tokens("See {{ @doc.foo.bar }} here.");
        let abs = decode(&toks);
        // {{ @doc .foo .bar }}
        assert_eq!(abs[0].3, TT_OPERATOR); // {{
        assert_eq!(abs[1].3, TT_NAMESPACE); // @doc
        assert_eq!(abs[2].3, TT_PROPERTY); // foo
        assert_eq!(abs[3].3, TT_PROPERTY); // bar
        assert_eq!(abs[4].3, TT_OPERATOR); // }}
    }

    #[test]
    fn keywords_recognized() {
        let toks = build_semantic_tokens("{{ each x in @docs.all }}{{ /each }}");
        let abs = decode(&toks);
        // {{, each, x, in, @docs, all, }}, {{, /each, }}
        let kinds: Vec<u32> = abs.iter().map(|t| t.3).collect();
        assert!(kinds.contains(&TT_KEYWORD));
        assert!(kinds.contains(&TT_NAMESPACE));
    }

    #[test]
    fn function_call_recognized() {
        let toks = build_semantic_tokens("{{ @doc.x:has(param) }}");
        let abs = decode(&toks);
        let kinds: Vec<u32> = abs.iter().map(|t| t.3).collect();
        assert!(kinds.contains(&TT_FUNCTION));
    }

    #[test]
    fn string_and_number_literals() {
        let toks = build_semantic_tokens(r#"{{ if @doc.x == "foo" }}{{ /if }}"#);
        let abs = decode(&toks);
        let kinds: Vec<u32> = abs.iter().map(|t| t.3).collect();
        assert!(kinds.contains(&TT_STRING));
    }

    #[test]
    fn multi_line_independent_tokens() {
        let toks = build_semantic_tokens("{{ @doc.a }}\n{{ @doc.b }}");
        let abs = decode(&toks);
        // Tokens on lines 0 and 1.
        let lines: Vec<u32> = abs.iter().map(|t| t.0).collect();
        assert!(lines.contains(&0));
        assert!(lines.contains(&1));
    }
}

#[cfg(test)]
mod code_action_tests {
    use super::*;

    #[test]
    fn rust_skeleton_uses_triple_slash() {
        let out = render_doc_skeleton("rs", &[("name".to_owned(), Some("&str".to_owned()))], 5);
        assert!(out.contains("/// @doc"));
        assert!(out.contains("/// :description: "));
        assert!(out.contains("/// :param: name &str description"));
        // Indented 4 spaces (column 5 → 4 spaces).
        assert!(out.starts_with("    "));
    }

    #[test]
    fn ts_skeleton_uses_jsdoc() {
        let out = render_doc_skeleton("ts", &[("x".to_owned(), Some("number".to_owned()))], 1);
        assert!(out.contains("/**"));
        assert!(out.contains(" * @doc"));
        assert!(out.contains(" * :param: x number description"));
        assert!(out.contains(" */"));
    }

    #[test]
    fn python_skeleton_uses_triple_quote() {
        let out = render_doc_skeleton("py", &[], 1);
        assert!(out.contains("\"\"\"@doc"));
        assert!(out.ends_with("\"\"\"\n"));
    }

    #[test]
    fn unknown_extension_falls_back_to_double_slash() {
        let out = render_doc_skeleton("zig", &[], 1);
        assert!(out.contains("// @doc"));
        assert!(out.contains("// :description: "));
    }

    #[test]
    fn skeleton_without_params_omits_param_lines() {
        let out = render_doc_skeleton("rs", &[], 1);
        assert!(!out.contains("@param"));
        assert!(!out.contains(":param:"));
    }

    #[test]
    fn skeleton_with_unknown_param_type_uses_placeholder() {
        let out = render_doc_skeleton("rs", &[("foo".to_owned(), None)], 1);
        assert!(out.contains(":param: foo Type description"));
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn valid_doc_keys_pass() {
        assert!(is_valid_doc_key("foo"));
        assert!(is_valid_doc_key("foo.bar"));
        assert!(is_valid_doc_key("foo.bar.baz_qux"));
        assert!(is_valid_doc_key("Module1.Sub2._private"));
    }

    #[test]
    fn invalid_doc_keys_rejected() {
        assert!(!is_valid_doc_key(""));
        assert!(!is_valid_doc_key(".leading_dot"));
        assert!(!is_valid_doc_key("trailing_dot."));
        assert!(!is_valid_doc_key("has space"));
        assert!(!is_valid_doc_key("has-dash"));
        assert!(!is_valid_doc_key("has/slash"));
    }

    #[test]
    fn key_at_position_with_range_returns_token_range() {
        let text = "See {{ @doc.foo.bar }} here.";
        // Position in the middle of "foo.bar" → we should get "foo.bar"
        // back and a range covering exactly those 7 characters.
        let (key, range) = key_at_position_with_range(
            text,
            Position {
                line: 0,
                character: 14,
            },
        )
        .unwrap();
        assert_eq!(key, "foo.bar");
        assert_eq!(range.start.line, 0);
        assert_eq!(range.end.line, 0);
        assert_eq!(range.end.character - range.start.character, 7);
    }

    #[test]
    fn key_at_position_outside_doc_ref_returns_none() {
        let text = "Just plain markdown.";
        let result = key_at_position_with_range(
            text,
            Position {
                line: 0,
                character: 5,
            },
        );
        assert!(result.is_none());
    }

    // -------- find_doc_tag_line (source rename) --------

    #[test]
    fn finds_doc_tag_in_rust_doc_comment() {
        let src = "/// @doc math.add\n/// :description: adds two numbers\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
        // Symbol on line 3 (1-indexed). We expect to find the @doc line at
        // line index 0 (1st line), col after `/// @doc ` = 9.
        let (line, col) = find_doc_tag_line(src, 3, "doc", "math.add").unwrap();
        assert_eq!(line, 0);
        assert_eq!(col, 9);
        // Verify the slice we'd patch.
        let line_text = src.lines().nth(line).unwrap();
        assert_eq!(&line_text[col..col + "math.add".len()], "math.add");
    }

    #[test]
    fn finds_doc_tag_in_jsdoc_comment() {
        let src =
            "/**\n * @doc users.create\n * @param user User\n */\nfunction createUser(user) {}\n";
        let (line, col) = find_doc_tag_line(src, 5, "doc", "users.create").unwrap();
        assert_eq!(line, 1);
        let line_text = src.lines().nth(line).unwrap();
        assert_eq!(&line_text[col..col + "users.create".len()], "users.create");
    }

    #[test]
    fn does_not_match_unrelated_keys() {
        let src = "/// @doc other.key\npub fn f() {}\n";
        let result = find_doc_tag_line(src, 2, "doc", "math.add");
        assert!(result.is_none());
    }

    #[test]
    fn does_not_match_partial_tag_name() {
        // `@docs` should NOT match when we're searching for `@doc`.
        let src = "/// @docs.module(foo)\npub fn f() {}\n";
        let result = find_doc_tag_line(src, 2, "doc", "foo");
        assert!(result.is_none());
    }

    #[test]
    fn respects_custom_doc_tag() {
        let src = "/// @standardoc widget.click\npub fn click() {}\n";
        let (line, _col) = find_doc_tag_line(src, 2, "standardoc", "widget.click").unwrap();
        assert_eq!(line, 0);
    }

    #[test]
    fn returns_none_when_no_comment_block() {
        let src = "pub fn lonely() {}\n";
        let result = find_doc_tag_line(src, 1, "doc", "lonely");
        assert!(result.is_none());
    }
}
