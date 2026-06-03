use std::collections::HashSet;
use std::path::PathBuf;

use full_moon::ast::{Ast, Expression, LastStmt, Stmt, Var};
use standardoc_ir::{Language, RawCallSite, RawDocument, RawEdge, RawSymbol};

use crate::walk_core::WalkContextCore;

use super::extract;
use super::helpers::ident_text;

/// Per-file walker state for the Lua provider.
///
/// Composes `WalkContextCore` (file path / module FQDN / symbols / edges /
/// defined_fqdns) with Lua-specific resolution state: the owning package
/// name (FQDN namespace), the absolute file paths needed for `require()`
/// resolution, and the module-pattern detection state (table candidates +
/// the table identifier returned at file end, if any).
///
/// `package_name`, `from_file_abs_path`, `package_root` are not consumed
/// by the day-1 walk yet — they are stashed for the visibility refinement
/// track (Rust pub use + TS re-exports) which will need cross-file FQDN
/// resolution to decide effective public surface for module-table fields
/// re-exported across files.
#[allow(dead_code)]
pub(crate) struct LuaWalkContext {
    pub(crate) core: WalkContextCore,
    pub(crate) package_name: String,
    pub(crate) from_file_abs_path: PathBuf,
    pub(crate) package_root: PathBuf,
    /// The full file source, used to convert full_moon's byte offsets into
    /// UTF-16 columns when stamping locations. Set once at the top of `walk`.
    pub(crate) content: String,
    /// Identifiers declared as `local <ident> = {}` at top level. Tracked
    /// during walk so the post-pass can decide visibility for symbols
    /// nested under one of these tables.
    pub(crate) local_table_idents: HashSet<String>,
    /// Identifier returned at the end of the file (`return M`), if any.
    /// `None` means the file does not follow the module-pattern.
    pub(crate) exported_table: Option<String>,
}

impl LuaWalkContext {
    pub(crate) fn new(
        file_path: String,
        package_name: String,
        file_module_fqdn: String,
        from_file_abs_path: PathBuf,
        package_root: PathBuf,
    ) -> Self {
        Self {
            core: WalkContextCore::new(file_path, file_module_fqdn, Language::Lua),
            package_name,
            from_file_abs_path,
            package_root,
            content: String::new(),
            local_table_idents: HashSet::new(),
            exported_table: None,
        }
    }

    pub(crate) fn push_symbol(&mut self, sym: RawSymbol) {
        self.core.push_symbol(sym);
    }

    pub(crate) fn push_edge(&mut self, edge: RawEdge) {
        self.core.push_edge(edge);
    }

    pub(crate) fn push_document(&mut self, doc: RawDocument) {
        self.core.push_document(doc);
    }

    pub(crate) fn push_call_site(&mut self, cs: RawCallSite) {
        self.core.push_call_site(cs);
    }

    /// Marks a top-level `local <ident> = {}` so the post-pass can decide
    /// whether fields nested under `<ident>` are public (table is returned)
    /// or private (table stays file-local).
    pub(crate) fn record_local_table(&mut self, ident: &str) {
        self.local_table_idents.insert(ident.to_string());
    }
}

/// Walk a parsed Lua AST and populate `ctx` with symbols / edges /
/// documents. After dispatching all top-level statements, runs the
/// module-pattern detection (looks at `Block::last_stmt`) and refines
/// the visibility of symbols nested under the exported table.
pub(crate) fn walk(content: &str, ast: &Ast, ctx: &mut LuaWalkContext) {
    content.clone_into(&mut ctx.content);
    let block = ast.nodes();

    // P1 — symbol + edge pass.
    for stmt in block.stmts() {
        match stmt {
            Stmt::LocalFunction(lf) => extract::extract_local_function(ctx, lf, content),
            Stmt::FunctionDeclaration(fd) => {
                extract::extract_function_declaration(ctx, fd, content);
            }
            Stmt::LocalAssignment(la) => extract::extract_local_assignment(ctx, la, content),
            Stmt::Assignment(a) => extract::extract_assignment(ctx, a, content),
            Stmt::FunctionCall(fc) => extract::record_call_edge_from_module(ctx, fc),
            // Other stmts (Do, If, While, NumericFor, GenericFor, Repeat,
            // Goto, Label, CompoundAssignment, *TypeDeclaration, etc.)
            // do not produce top-level symbols. Day-1 ignores them; the
            // Annotation parser track and future refinements may dive in.
            _ => {}
        }
    }

    // Module-pattern detection — only when the file ends with `return X`
    // and `X` is one of the local tables we recorded in P1.
    if let Some(LastStmt::Return(ret)) = block.last_stmt()
        && let Some(first_expr) = ret.returns().iter().next()
        && let Some(name) = returned_name(first_expr)
        && ctx.local_table_idents.contains(&name)
    {
        ctx.exported_table = Some(name);
    }

    // P2 — visibility refinement based on module-pattern.
    apply_module_pattern_visibility(ctx);
}

/// Extract the bare identifier name when the returned expression is just
/// `Var::Name(ident)`. Returns `None` for any composite shape.
fn returned_name(expr: &Expression) -> Option<String> {
    let Expression::Var(var) = expr else {
        return None;
    };
    let Var::Name(token) = var else {
        return None;
    };
    let text = ident_text(token);
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Re-walk the symbols collected in P1 and downgrade visibility for
/// fields nested under a local table that is NOT the exported one.
///
/// Decision matrix for a symbol whose FQDN is
/// `<file_module>::<table>::<field>`:
/// - `<table>` not in `local_table_idents` → leave as-is (global table
///   reference; we have no info to refine).
/// - `<table>` in `local_table_idents` and matches `exported_table` →
///   set Public.
/// - `<table>` in `local_table_idents` and does NOT match
///   `exported_table` (or no export) → set Private.
fn apply_module_pattern_visibility(ctx: &mut LuaWalkContext) {
    use standardoc_ir::Visibility;

    let module_fqdn = ctx.core.file_module_fqdn.clone();
    let exported = ctx.exported_table.clone();
    let local_tables = ctx.local_table_idents.clone();

    for sym in &mut ctx.core.symbols {
        let Some(rest) = sym.fqdn.strip_prefix(&module_fqdn) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("::") else {
            continue;
        };
        // We expect at least <table>::<field> — a single segment is the
        // module-direct symbol (already finalized).
        let Some((table, _field)) = rest.split_once("::") else {
            continue;
        };
        if !local_tables.contains(table) {
            continue;
        }
        sym.visibility = if exported.as_deref() == Some(table) {
            Visibility::Public
        } else {
            Visibility::Private
        };
    }
}
