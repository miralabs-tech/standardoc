use std::path::Path;

use full_moon::ast::{
    Assignment, Expression, FunctionBody, FunctionCall, FunctionDeclaration, FunctionName,
    LocalAssignment, LocalFunction, Parameter, Prefix, Stmt, Suffix, Var,
};
use full_moon::node::Node;
use full_moon::tokenizer::{Position, TokenReference, TokenType};
use standardoc_core::ExtractError;
use standardoc_ir::{
    Blake3Hash, BuiltinTier, DeclKind, EdgeKind, ExtractedFile, Kind, Language, LanguageKind,
    Modifiers, Param, RawCallArg, RawCallSite, RawEdge, RawSymbol, ResolvedOrUnresolved, Signature,
    SignatureMeta, Site, SourceOrigin, SymbolLocation, TypeRef, Visibility,
};

use super::extract_doc;
use super::helpers::{compute_module_path, ident_text, string_literal_text};
use super::resolver::resolve_require;
use super::walk::{LuaWalkContext, walk};
use crate::builtins::global as global_builtin_registry;
use crate::utils::{file_span, hash_bytes, last_segment, parent_module};

/// Parse a Lua source file with `full_moon`, walk it, and return an
/// `ExtractedFile` ready for the pipeline.
pub(crate) fn extract_file(
    content: &str,
    workspace_relative_path: &str,
    package_name: &str,
    package_relative: &str,
    from_file_abs_path: &Path,
    package_root: &Path,
) -> Result<ExtractedFile, ExtractError> {
    let ast = full_moon::parse(content).map_err(|errors| ExtractError::Parse {
        file: workspace_relative_path.into(),
        detail: errors
            .iter()
            .map(|e| e.error_message().to_string())
            .collect::<Vec<_>>()
            .join("; "),
    })?;

    let module_path = compute_module_path(package_relative);
    let module_fqdn = if module_path.is_empty() {
        package_name.to_string()
    } else {
        format!("{package_name}::{module_path}")
    };

    let content_hash = hash_bytes(content.as_bytes());

    let module_symbol = RawSymbol {
        decl_kind: Some(DeclKind::Module),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: last_segment(&module_fqdn).to_string(),
        fqdn: module_fqdn.clone(),
        kind: Kind::Module,
        language_kind: LanguageKind::from("module"),
        module: parent_module(&module_fqdn),
        visibility: Visibility::Public,
        location: file_span(workspace_relative_path, content),
        signature: None,
        body_hash: Some(content_hash),
        attributes: vec![],
        flags: vec![],
    };

    let mut ctx = LuaWalkContext::new(
        workspace_relative_path.to_string(),
        package_name.to_string(),
        module_fqdn.clone(),
        from_file_abs_path.to_path_buf(),
        package_root.to_path_buf(),
    );
    ctx.push_symbol(module_symbol);

    // Module-level doc capture from the first stmt's leading trivia.
    if let Some(first_stmt) = ast.nodes().stmts().next()
        && let Some(first_token) = first_stmt_first_token(first_stmt)
        && let Some(doc) = extract_doc::capture_module_doc(&module_fqdn, first_token)
    {
        ctx.push_document(doc);
    }

    walk(content, &ast, &mut ctx);

    // Post-walk pass: enrich Lua signatures (typing-free at AST level) with
    // any EmmyLua / LuaCATS annotations carried by the symbol's RawDocument.
    // The raw doc text stays intact on the documents side; this just lifts
    // the typed pieces into the structured Signature.
    enrich_signatures_from_emmylua(&mut ctx.core.symbols, &ctx.core.documents);

    // G6: extract LuaJIT `ffi.cdef[[...]]` declarations as virtual
    // `RawSymbol` + `RawFfiBinding` pairs so cross-language `ffi_resolve`
    // can pair them with C-side exports.
    let (ffi_symbols, ffi_bindings) =
        super::ffi_tagger::extract_ffi_bindings(&ast, &module_fqdn, workspace_relative_path);
    ctx.core.symbols.extend(ffi_symbols);

    // G4-lua: build the AOT ModuleLookup so cross-workspace edge
    // strengthening can resolve peer-Lua imports. Mirrors the
    // c::lookup contract — root-scope bindings only, no scope-aware
    // visitor needed for the resolver to do its job.
    let module_lookup =
        super::lookup::build_lua_lookup(&ctx.core.symbols, &ctx.core.edges, &module_fqdn);

    Ok(ExtractedFile {
        file: workspace_relative_path.into(),
        language: Language::Lua,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash,
        byte_size: u64::try_from(content.len()).unwrap_or(u64::MAX),
        symbols: ctx.core.symbols,
        edges: ctx.core.edges,
        call_sites: ctx.core.call_sites,
        documents: ctx.core.documents,
        // G6 covers the Lua-side cdef extraction. C-side `lua_register`
        // / `luaL_register` / `luaL_setfuncs` detection (the Export
        // counterpart) is tracked separately — those emissions live on
        // the C provider and are not part of this Lua-side extraction.
        ffi_bindings,
        module_lookup: Some(module_lookup),
    })
}

/// Mutate each `RawSymbol` whose fqdn has a matching `RawDocument`, lifting
/// `@param` / `@return` tags from the doc text into the structured
/// `Signature`. Symbols without a signature are skipped (no place to write
/// the typed fields into).
fn enrich_signatures_from_emmylua(
    symbols: &mut [RawSymbol],
    documents: &[standardoc_ir::RawDocument],
) {
    use std::collections::HashMap;
    let docs_by_fqdn: HashMap<&str, &str> = documents
        .iter()
        .map(|d| (d.symbol_fqdn.as_str(), d.description.as_str()))
        .collect();
    for sym in symbols {
        let Some(doc_text) = docs_by_fqdn.get(sym.fqdn.as_str()) else {
            continue;
        };
        let Some(sig) = sym.signature.as_mut() else {
            continue;
        };
        super::emmylua::enrich_signature(sig, doc_text);
    }
}

// --- per-stmt extractors ---------------------------------------------------

pub(crate) fn extract_local_function(ctx: &mut LuaWalkContext, lf: &LocalFunction, content: &str) {
    let name = ident_text(lf.name()).to_string();
    if name.is_empty() {
        return;
    }
    let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
    let signature = signature_from_body(lf.body(), false);
    let location = node_location(ctx, lf, content);
    let body_hash = body_hash_for(lf, content);

    let sym = RawSymbol {
        decl_kind: Some(DeclKind::Function),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name,
        fqdn: fqdn.clone(),
        kind: Kind::Callable,
        language_kind: LanguageKind::from("local_function"),
        module: Some(ctx.core.file_module_fqdn.clone()),
        visibility: Visibility::Private,
        location,
        signature: Some(signature),
        body_hash,
        attributes: vec![],
        flags: vec![],
    };
    ctx.push_symbol(sym);

    if let Some(doc) = extract_doc::capture_doc_for_symbol(&fqdn, lf.local_token()) {
        ctx.push_document(doc);
    }
    record_calls_in_body(ctx, &fqdn, lf.body());
}

pub(crate) fn extract_function_declaration(
    ctx: &mut LuaWalkContext,
    fd: &FunctionDeclaration,
    content: &str,
) {
    let name_info = function_name_to_fqdn(ctx, fd.name());
    let Some((leaf_name, fqdn, is_method, parent_module_fqdn)) = name_info else {
        return;
    };
    let signature = signature_from_body(fd.body(), is_method);
    let location = node_location(ctx, fd, content);
    let body_hash = body_hash_for(fd, content);
    let language_kind = if is_method {
        LanguageKind::from("method")
    } else {
        LanguageKind::from("function")
    };
    let decl_kind = if is_method {
        DeclKind::Method
    } else {
        DeclKind::Function
    };

    // Default Public for both shapes:
    // * `function foo()` (global) — Public day-1.
    // * `function M.foo()` / `function M:bar()` — Public default, refined
    //   by the module-pattern post-process if `M` is a local table.
    let visibility = Visibility::Public;

    let sym = RawSymbol {
        decl_kind: Some(decl_kind),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: leaf_name,
        fqdn: fqdn.clone(),
        kind: Kind::Callable,
        language_kind,
        module: Some(parent_module_fqdn),
        visibility,
        location,
        signature: Some(signature),
        body_hash,
        attributes: vec![],
        flags: vec![],
    };
    ctx.push_symbol(sym);

    if let Some(doc) = extract_doc::capture_doc_for_symbol(&fqdn, fd.function_token()) {
        ctx.push_document(doc);
    }
    record_calls_in_body(ctx, &fqdn, fd.body());
}

pub(crate) fn extract_local_assignment(
    ctx: &mut LuaWalkContext,
    la: &LocalAssignment,
    content: &str,
) {
    let names: Vec<&TokenReference> = la.names().iter().collect();
    let exprs: Vec<&Expression> = la.expressions().iter().collect();
    for (idx, name_token) in names.iter().enumerate() {
        let name = ident_text(name_token).to_string();
        if name.is_empty() {
            continue;
        }
        let rhs = exprs.get(idx).copied();

        // Record table candidates for module-pattern detection.
        if rhs.is_some_and(is_empty_or_table) {
            ctx.record_local_table(&name);
        }

        // Detect `local x = require("a.b.c")` and emit IMPORTS edge.
        if let Some(require_arg) = rhs.and_then(extract_require_arg) {
            let pos = Node::start_position(*name_token).unwrap_or_default();
            let to = resolve_require(&require_arg);
            let confidence = to.default_confidence();
            ctx.push_edge(RawEdge {
                from_fqdn: ctx.core.file_module_fqdn.clone(),
                kind: EdgeKind::Imports,
                to,
                sites: vec![site_for(&ctx.core.file_path, pos)],
                attributes: vec![],
                confidence,
                receiver_type: None,
            });
        }

        let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
        let location = node_location(ctx, *name_token, content);
        let body_hash = body_hash_for(la, content);

        let sym = RawSymbol {
            decl_kind: Some(DeclKind::Var),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name,
            fqdn: fqdn.clone(),
            kind: Kind::Value,
            language_kind: LanguageKind::from("local"),
            module: Some(ctx.core.file_module_fqdn.clone()),
            visibility: Visibility::Private,
            location,
            signature: None,
            body_hash,
            attributes: vec![],
            flags: vec![],
        };
        ctx.push_symbol(sym);

        // Doc only on first var (multi-var local has only one preceding
        // comment block).
        if idx == 0
            && let Some(doc) = extract_doc::capture_doc_for_symbol(&fqdn, la.local_token())
        {
            ctx.push_document(doc);
        }
    }
}

pub(crate) fn extract_assignment(ctx: &mut LuaWalkContext, a: &Assignment, content: &str) {
    let vars: Vec<&Var> = a.variables().iter().collect();
    let exprs: Vec<&Expression> = a.expressions().iter().collect();
    for (idx, var) in vars.iter().enumerate() {
        let rhs = exprs.get(idx).copied();
        let Some(dotted) = var_to_dotted_path(var) else {
            continue;
        };
        // Only handle assignments where RHS is a function literal:
        // `M.foo = function() ... end` becomes a function symbol of
        // `<file_module>::M::foo`.
        let Some(Expression::Function(anon)) = rhs else {
            continue;
        };
        let segments: Vec<&str> = dotted.split('.').collect();
        let leaf = segments.last().copied().unwrap_or("");
        if leaf.is_empty() {
            continue;
        }
        let qualified = segments.join("::");
        let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, qualified);
        let parent_fqdn = if segments.len() <= 1 {
            ctx.core.file_module_fqdn.clone()
        } else {
            format!(
                "{}::{}",
                ctx.core.file_module_fqdn,
                segments[..segments.len() - 1].join("::")
            )
        };

        let signature = signature_from_body(anon.body(), false);
        let location = node_location(ctx, *var, content);
        let body_hash = body_hash_for(*var, content);

        let sym = RawSymbol {
            decl_kind: Some(DeclKind::Function),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: leaf.to_string(),
            fqdn: fqdn.clone(),
            kind: Kind::Callable,
            language_kind: LanguageKind::from("function"),
            module: Some(parent_fqdn),
            visibility: Visibility::Public,
            location,
            signature: Some(signature),
            body_hash,
            attributes: vec![],
            flags: vec![],
        };
        ctx.push_symbol(sym);

        record_calls_in_body(ctx, &fqdn, anon.body());
    }
}

pub(crate) fn record_call_edge_from_module(ctx: &mut LuaWalkContext, fc: &FunctionCall) {
    let from_fqdn = ctx.core.file_module_fqdn.clone();
    record_call_or_require(ctx, &from_fqdn, fc);
}

// --- nested call site walker (1-deep through function bodies) -------------

fn record_calls_in_body(ctx: &mut LuaWalkContext, caller_fqdn: &str, body: &FunctionBody) {
    record_calls_in_block(ctx, caller_fqdn, body.block());
}

fn record_calls_in_block(
    ctx: &mut LuaWalkContext,
    caller_fqdn: &str,
    block: &full_moon::ast::Block,
) {
    for stmt in block.stmts() {
        match stmt {
            Stmt::FunctionCall(fc) => record_call_or_require(ctx, caller_fqdn, fc),
            Stmt::LocalAssignment(la) => {
                for expr in la.expressions() {
                    if let Some(req) = extract_require_arg(expr) {
                        let pos = Node::start_position(la.local_token()).unwrap_or_default();
                        let to = resolve_require(&req);
                        let confidence = to.default_confidence();
                        ctx.push_edge(RawEdge {
                            from_fqdn: caller_fqdn.to_string(),
                            kind: EdgeKind::Imports,
                            to,
                            sites: vec![site_for(&ctx.core.file_path, pos)],
                            attributes: vec![],
                            confidence,
                            receiver_type: None,
                        });
                    }
                }
            }
            Stmt::Assignment(a) => {
                for expr in a.expressions() {
                    if let Some(req) = extract_require_arg(expr) {
                        let to = resolve_require(&req);
                        let confidence = to.default_confidence();
                        ctx.push_edge(RawEdge {
                            from_fqdn: caller_fqdn.to_string(),
                            kind: EdgeKind::Imports,
                            to,
                            sites: vec![],
                            attributes: vec![],
                            confidence,
                            receiver_type: None,
                        });
                    }
                }
            }
            Stmt::If(if_stmt) => {
                record_calls_in_block(ctx, caller_fqdn, if_stmt.block());
                for elseif in if_stmt.else_if().into_iter().flatten() {
                    record_calls_in_block(ctx, caller_fqdn, elseif.block());
                }
                if let Some(else_block) = if_stmt.else_block() {
                    record_calls_in_block(ctx, caller_fqdn, else_block);
                }
            }
            Stmt::While(w) => record_calls_in_block(ctx, caller_fqdn, w.block()),
            Stmt::Repeat(r) => record_calls_in_block(ctx, caller_fqdn, r.block()),
            Stmt::NumericFor(nf) => record_calls_in_block(ctx, caller_fqdn, nf.block()),
            Stmt::GenericFor(gf) => record_calls_in_block(ctx, caller_fqdn, gf.block()),
            Stmt::Do(d) => record_calls_in_block(ctx, caller_fqdn, d.block()),
            _ => {}
        }
    }
}

fn record_call_or_require(ctx: &mut LuaWalkContext, from_fqdn: &str, fc: &FunctionCall) {
    // Distinguish `require("x.y")` (IMPORTS) from regular calls (CALLS).
    if let Some(req) = require_arg_from_call(fc) {
        let to = resolve_require(&req);
        let confidence = to.default_confidence();
        ctx.push_edge(RawEdge {
            from_fqdn: from_fqdn.to_string(),
            kind: EdgeKind::Imports,
            to,
            sites: vec![site_for(&ctx.core.file_path, call_start(fc))],
            attributes: vec![],
            confidence,
            receiver_type: None,
        });
        return;
    }
    let Some(call_name) = call_target_name(fc) else {
        return;
    };
    // IR-4-d: observational call_site — emitted alongside the Calls
    // edge so the plugin layer sees the textual shape regardless of
    // whether `call_name` resolved against the builtin registry or
    // landed as Unresolved. The push happens BEFORE the Drop/Attribute
    // tier short-circuit below (Lua has no such builtins today, but
    // the contract should hold if any are added in the future).
    if let Some((callee_text, receiver_chain)) = call_target_decomposed(fc) {
        let args = args_from_function_call(fc);
        ctx.push_call_site(RawCallSite {
            from_fqdn: from_fqdn.to_string(),
            callee_text,
            args,
            receiver_chain,
            site: site_for(&ctx.core.file_path, call_start(fc)),
        });
    }
    // Stage 3e-1: consult the Lua builtin registry. Lua has no Drop /
    // Attribute classifications today (see `builtins/lua.rs` rationale)
    // so every match is Edge — emit a synthetic FQDN with `via-builtin`
    // / `builtin-<slug>` attrs. We try the full dotted/colon path first
    // (`table.insert`, `string.format`, `os.time` are explicitly
    // enumerated as hot members), then fall back to the leftmost
    // segment for un-enumerated members of standard-library modules
    // (`os.tmpname()` falls back to `os`, still tagged as a stdlib
    // touch). Locals like `myTable.foo` miss both lookups and stay
    // `Unresolved`.
    let registry = global_builtin_registry();
    let entry_opt = registry.lookup(&call_name, Language::Lua).or_else(|| {
        let leftmost = call_name.split(['.', ':']).next().unwrap_or("");
        if leftmost.is_empty() || leftmost == call_name {
            return None;
        }
        registry.lookup(leftmost, Language::Lua)
    });
    let (to, attributes) = match entry_opt {
        // Reserve the tier match for forward-compat: if a Drop /
        // Attribute Lua builtin is ever added, mirror JS/TS/Rust and
        // skip the edge silently.
        Some(entry) => match entry.tier {
            BuiltinTier::Drop | BuiltinTier::Attribute => return,
            BuiltinTier::Edge => (
                ResolvedOrUnresolved::Resolved {
                    fqdn: entry.synthetic_fqdn.clone(),
                },
                vec![
                    "via-builtin".to_string(),
                    format!("builtin-{}", entry.tag.slug()),
                ],
            ),
        },
        None => (ResolvedOrUnresolved::Unresolved { name: call_name }, vec![]),
    };
    let confidence = to.default_confidence();
    ctx.push_edge(RawEdge {
        from_fqdn: from_fqdn.to_string(),
        kind: EdgeKind::Calls,
        to,
        sites: vec![site_for(&ctx.core.file_path, call_start(fc))],
        attributes,
        confidence,
        receiver_type: None,
    });
}

// --- name / FQDN computation ----------------------------------------------

/// Resolve a `FunctionDeclaration::name()` into:
/// `(leaf_name, full_fqdn, is_method, parent_module_fqdn)`.
fn function_name_to_fqdn(
    ctx: &LuaWalkContext,
    fname: &FunctionName,
) -> Option<(String, String, bool, String)> {
    let dotted: Vec<String> = fname
        .names()
        .iter()
        .map(|t| ident_text(t).to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if dotted.is_empty() {
        return None;
    }
    let method = fname.method_name().map(|t| ident_text(t).to_string());

    let (leaf, segments, is_method) = match method {
        Some(m) if !m.is_empty() => {
            let mut segs = dotted;
            segs.push(m.clone());
            (m, segs, true)
        }
        _ => {
            let leaf = dotted.last().cloned().unwrap_or_default();
            (leaf, dotted, false)
        }
    };

    if segments.is_empty() || leaf.is_empty() {
        return None;
    }
    let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, segments.join("::"));
    let parent = if segments.len() <= 1 {
        ctx.core.file_module_fqdn.clone()
    } else {
        format!(
            "{}::{}",
            ctx.core.file_module_fqdn,
            segments[..segments.len() - 1].join("::")
        )
    };
    Some((leaf, fqdn, is_method, parent))
}

/// Convert a `Var` (`x` / `M.foo` / `M.foo.bar` / `M:bar`) into a dotted
/// path like `"M.foo"`. Returns `None` for shapes we don't recognise
/// (call results, indexed access via brackets, etc.).
fn var_to_dotted_path(var: &Var) -> Option<String> {
    match var {
        Var::Name(t) => {
            let n = ident_text(t);
            if n.is_empty() {
                None
            } else {
                Some(n.to_string())
            }
        }
        Var::Expression(ve) => {
            let head = match ve.prefix() {
                Prefix::Name(t) => ident_text(t).to_string(),
                _ => return None,
            };
            if head.is_empty() {
                return None;
            }
            let mut parts = vec![head];
            for sfx in ve.suffixes() {
                match sfx {
                    Suffix::Index(idx) => {
                        if let Some(name) = index_dot_name(idx) {
                            parts.push(name);
                        } else {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
            Some(parts.join("."))
        }
        _ => None,
    }
}

fn index_dot_name(idx: &full_moon::ast::Index) -> Option<String> {
    if let full_moon::ast::Index::Dot { name, .. } = idx {
        let n = ident_text(name);
        if n.is_empty() {
            None
        } else {
            Some(n.to_string())
        }
    } else {
        None
    }
}

/// Inspect the head of a `FunctionCall` chain and return a flat name like
/// `"foo"`, `"M.foo"`, `"obj:method"`. Returns `None` if the call target
/// isn't a recognisable bare name chain.
fn call_target_name(fc: &FunctionCall) -> Option<String> {
    let head = match fc.prefix() {
        Prefix::Name(t) => ident_text(t).to_string(),
        _ => return None,
    };
    if head.is_empty() {
        return None;
    }
    let mut parts = vec![head];
    let mut method_suffix: Option<String> = None;
    for sfx in fc.suffixes() {
        match sfx {
            Suffix::Index(idx) => {
                if let Some(n) = index_dot_name(idx) {
                    parts.push(n);
                } else {
                    return None;
                }
            }
            Suffix::Call(call) => {
                // The call itself is fine — but if it's a MethodCall
                // (`obj:method(args)`), capture the method name.
                if let full_moon::ast::Call::MethodCall(mc) = call {
                    method_suffix = Some(ident_text(mc.name()).to_string());
                }
                // Stop accumulating after the first call site to avoid
                // chaining (`a.b().c().d()` collapses to "a.b").
                break;
            }
            _ => return None,
        }
    }
    let mut name = parts.join(".");
    if let Some(method) = method_suffix
        && !method.is_empty()
    {
        name.push(':');
        name.push_str(&method);
    }
    Some(name)
}

fn call_start(fc: &FunctionCall) -> Position {
    fc.prefix().start_position().unwrap_or_default()
}

/// IR-4-d: decompose a `FunctionCall` into `(callee_text, receiver_chain)`
/// for the [`RawCallSite`] record. Mirrors [`call_target_name`]'s walk
/// but splits the final segment off as the called function — the
/// preceding segments form the receiver chain (Lua doesn't truly have
/// methods on a function-call basis, but `obj.field.foo()` is the
/// shape plugins care about so we expose it the same way Rust and TS do).
///
/// Shape per pattern:
/// - `foo(x)`           → callee_text=`foo`,        chain=[]
/// - `M.foo(x)`         → callee_text=`M.foo`,      chain=[`M`]
/// - `M.a.b.foo(x)`     → callee_text=`M.a.b.foo`,  chain=[`M`,`a`,`b`]
/// - `obj:bar(x)`       → callee_text=`obj:bar`,    chain=[`obj`]
/// - `M.api:create(x)`  → callee_text=`M.api:create`, chain=[`M`,`api`]
///
/// Returns `None` for shapes [`call_target_name`] rejects (call-result
/// callees, computed indexing, …).
fn call_target_decomposed(fc: &FunctionCall) -> Option<(String, Vec<String>)> {
    let head = match fc.prefix() {
        Prefix::Name(t) => ident_text(t).to_string(),
        _ => return None,
    };
    if head.is_empty() {
        return None;
    }
    let mut parts = vec![head];
    let mut method_suffix: Option<String> = None;
    for sfx in fc.suffixes() {
        match sfx {
            Suffix::Index(idx) => {
                if let Some(n) = index_dot_name(idx) {
                    parts.push(n);
                } else {
                    return None;
                }
            }
            Suffix::Call(call) => {
                if let full_moon::ast::Call::MethodCall(mc) = call {
                    method_suffix = Some(ident_text(mc.name()).to_string());
                }
                break;
            }
            _ => return None,
        }
    }
    // `parts` is the dotted prefix; the final segment becomes the
    // function name unless this is a `:method()` form (then the method
    // is appended via `:`).
    let (callee_text, receiver_chain) = if let Some(method) = method_suffix
        && !method.is_empty()
    {
        let chain = parts.clone();
        let prefix = parts.join(".");
        (format!("{prefix}:{method}"), chain)
    } else if parts.len() == 1 {
        (parts.remove(0), Vec::new())
    } else {
        let func = parts.pop().expect("len > 1");
        let chain = parts.clone();
        (format!("{}.{}", parts.join("."), func), chain)
    };
    Some((callee_text, receiver_chain))
}

/// IR-4-d: extract positional args from a `FunctionCall`'s single
/// `Suffix::Call`. Handles all three Lua argument shapes:
/// - `foo(a, b)`     → punctuated list of expressions
/// - `foo "literal"` → single string literal arg (special syntax)
/// - `foo {1,2,3}`   → single table-constructor arg
///
/// Each arg is classified via [`arg_from_expression`]; string-literal
/// args strip surrounding quotes and set `is_string_literal=true`.
fn args_from_function_call(fc: &FunctionCall) -> Vec<RawCallArg> {
    use full_moon::ast::Call as FullMoonCall;
    use full_moon::ast::FunctionArgs;
    let mut out = Vec::new();
    for sfx in fc.suffixes() {
        let Suffix::Call(call) = sfx else { continue };
        let args = match call {
            FullMoonCall::AnonymousCall(a) => a,
            FullMoonCall::MethodCall(mc) => mc.args(),
            _ => continue,
        };
        match args {
            FunctionArgs::Parentheses { arguments, .. } => {
                for expr in arguments {
                    out.push(arg_from_expression(expr));
                }
            }
            FunctionArgs::String(token) => {
                if let TokenType::StringLiteral { literal, .. } = token.token_type() {
                    out.push(RawCallArg {
                        value: literal.to_string(),
                        is_string_literal: true,
                    });
                }
            }
            FunctionArgs::TableConstructor(tbl) => {
                out.push(RawCallArg {
                    value: tbl.to_string(),
                    is_string_literal: false,
                });
            }
            _ => {}
        }
        break;
    }
    out
}

fn arg_from_expression(expr: &Expression) -> RawCallArg {
    if let Some(text) = string_literal_text(expr) {
        return RawCallArg {
            value: text,
            is_string_literal: true,
        };
    }
    if let Expression::Var(Var::Name(t)) = expr {
        let n = ident_text(t);
        if !n.is_empty() {
            return RawCallArg {
                value: n.to_string(),
                is_string_literal: false,
            };
        }
    }
    // Fallback: use full_moon's Display impl which prints the AST node
    // back to source-faithful text (including any pre-trivia we don't
    // care about — `.trim()` keeps the value compact).
    RawCallArg {
        value: expr.to_string().trim().to_string(),
        is_string_literal: false,
    }
}

// --- require detection ----------------------------------------------------

fn extract_require_arg(expr: &Expression) -> Option<String> {
    let Expression::FunctionCall(fc) = expr else {
        return None;
    };
    require_arg_from_call(fc)
}

fn require_arg_from_call(fc: &FunctionCall) -> Option<String> {
    // `require "foo"` and `require("foo")` both qualify; method form
    // (`require:foo`) does not.
    let Prefix::Name(name) = fc.prefix() else {
        return None;
    };
    if ident_text(name) != "require" {
        return None;
    }
    let mut suffixes = fc.suffixes();
    let Some(Suffix::Call(call)) = suffixes.next() else {
        return None;
    };
    if suffixes.next().is_some() {
        // Chained calls after require — not the simple form.
        return None;
    }
    let full_moon::ast::Call::AnonymousCall(args) = call else {
        return None;
    };
    match args {
        full_moon::ast::FunctionArgs::Parentheses { arguments, .. } => {
            let first = arguments.iter().next()?;
            string_literal_text(first)
        }
        full_moon::ast::FunctionArgs::String(token) => {
            let TokenType::StringLiteral { literal, .. } = token.token_type() else {
                return None;
            };
            Some(literal.to_string())
        }
        _ => None,
    }
}

// --- signature ------------------------------------------------------------

fn signature_from_body(body: &FunctionBody, is_method: bool) -> Signature {
    let mut params = Vec::new();
    if is_method {
        params.push(Param {
            name: "self".into(),
            ty: TypeRef::new("any"),
            default: None,
        });
    }
    for p in body.parameters() {
        let name = match p {
            Parameter::Name(t) => ident_text(t).to_string(),
            Parameter::Ellipsis(_) => "...".to_string(),
            _ => continue,
        };
        if name.is_empty() {
            continue;
        }
        params.push(Param {
            name,
            ty: TypeRef::new("any"),
            default: None,
        });
    }
    Signature {
        params,
        returns: None,
        modifiers: Modifiers::default(),
        meta: SignatureMeta::default(),
    }
}

// --- locations / hashes ---------------------------------------------------

fn node_location<N: Node>(ctx: &LuaWalkContext, node: N, _content: &str) -> SymbolLocation {
    let start = node.start_position().unwrap_or_default();
    let end = node.end_position().unwrap_or_default();
    SymbolLocation {
        file: ctx.core.file_path.clone(),
        start_line: u32::try_from(start.line()).unwrap_or(u32::MAX),
        end_line: u32::try_from(end.line()).unwrap_or(u32::MAX),
        start_col: u32::try_from(start.character()).unwrap_or(u32::MAX),
        end_col: u32::try_from(end.character()).unwrap_or(u32::MAX),
    }
}

fn body_hash_for<N: Node>(node: N, content: &str) -> Option<Blake3Hash> {
    let start = node.start_position()?;
    let end = node.end_position()?;
    let lo = start.bytes().min(content.len());
    let hi = end.bytes().min(content.len());
    if hi <= lo {
        return None;
    }
    Some(hash_bytes(&content.as_bytes()[lo..hi]))
}

fn site_for(file: &str, pos: Position) -> Site {
    Site {
        file: file.to_string(),
        line: u32::try_from(pos.line()).unwrap_or(u32::MAX),
        col: u32::try_from(pos.character()).unwrap_or(u32::MAX),
    }
}

// --- module symbol helpers ------------------------------------------------
// `hash_bytes` / `last_segment` / `parent_module` / `file_span` /
// `content_extent` were duplicates across rust/ts/lua → moved to
// `crate::utils` (lock C-utils-44).

fn first_stmt_first_token(stmt: &Stmt) -> Option<&TokenReference> {
    match stmt {
        Stmt::LocalAssignment(la) => Some(la.local_token()),
        Stmt::LocalFunction(lf) => Some(lf.local_token()),
        Stmt::FunctionDeclaration(fd) => Some(fd.function_token()),
        _ => None,
    }
}

const fn is_empty_or_table(expr: &Expression) -> bool {
    matches!(expr, Expression::TableConstructor(_))
}

#[cfg(test)]
mod tests;
