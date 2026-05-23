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
    Modifiers, Param, RawCallArg, RawCallSite, RawEdge, RawSymbol, ResolvedOrUnresolved,
    Signature, SignatureMeta, Site, SourceOrigin, SymbolLocation, TypeRef, Visibility,
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
    let module_lookup = super::lookup::build_lua_lookup(&ctx.core.symbols, &ctx.core.edges, &module_fqdn);

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
        name,
        fqdn: fqdn.clone(),
        kind: Kind::Function,
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
        name: leaf_name,
        fqdn: fqdn.clone(),
        kind: Kind::Function,
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
            });
        }

        let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
        let location = node_location(ctx, *name_token, content);
        let body_hash = body_hash_for(la, content);

        let sym = RawSymbol {
            decl_kind: Some(DeclKind::Var),
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
            name: leaf.to_string(),
            fqdn: fqdn.clone(),
            kind: Kind::Function,
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
        let leftmost = call_name
            .split(|c: char| c == '.' || c == ':')
            .next()
            .unwrap_or("");
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
                for expr in arguments.iter() {
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
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn extract(content: &str, workspace_relative: &str, package_relative: &str) -> ExtractedFile {
        extract_file(
            content,
            workspace_relative,
            "myapp",
            package_relative,
            &PathBuf::from(format!("/tmp/pkg/{package_relative}")),
            &PathBuf::from("/tmp/pkg"),
        )
        .expect("extract ok")
    }

    #[test]
    fn empty_file_produces_module_symbol_only() {
        let r = extract("", "main.lua", "main.lua");
        assert_eq!(r.symbols.len(), 1);
        assert!(r.edges.is_empty());
        assert_eq!(r.symbols[0].kind, Kind::Module);
        assert_eq!(r.symbols[0].fqdn, "myapp::main");
    }

    #[test]
    fn module_fqdn_drops_init_segment() {
        let r = extract("", "src/utils/init.lua", "src/utils/init.lua");
        assert_eq!(r.symbols[0].fqdn, "myapp::src::utils");
    }

    #[test]
    fn local_function_extracted_as_private() {
        let src = "local function helper(a, b) return a + b end\n";
        let r = extract(src, "main.lua", "main.lua");
        let sym = r
            .symbols
            .iter()
            .find(|s| s.name == "helper")
            .expect("helper");
        assert_eq!(sym.fqdn, "myapp::main::helper");
        assert_eq!(sym.kind, Kind::Function);
        assert_eq!(sym.visibility, Visibility::Private);
        let sig = sym.signature.as_ref().expect("sig");
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.params[0].name, "a");
    }

    #[test]
    fn global_function_extracted_as_public() {
        let src = "function greet(name) print(name) end\n";
        let r = extract(src, "main.lua", "main.lua");
        let sym = r.symbols.iter().find(|s| s.name == "greet").expect("greet");
        assert_eq!(sym.fqdn, "myapp::main::greet");
        assert_eq!(sym.visibility, Visibility::Public);
    }

    #[test]
    fn dotted_function_decl_yields_nested_fqdn() {
        let src = "function M.foo() end\n";
        let r = extract(src, "lib.lua", "lib.lua");
        let sym = r.symbols.iter().find(|s| s.name == "foo").expect("foo");
        assert_eq!(sym.fqdn, "myapp::lib::M::foo");
        assert_eq!(sym.module.as_deref(), Some("myapp::lib::M"));
    }

    #[test]
    fn method_decl_yields_self_first_param() {
        let src = "function M:bar(x) end\n";
        let r = extract(src, "lib.lua", "lib.lua");
        let sym = r.symbols.iter().find(|s| s.name == "bar").expect("bar");
        assert_eq!(sym.fqdn, "myapp::lib::M::bar");
        let sig = sym.signature.as_ref().expect("sig");
        assert_eq!(sig.params[0].name, "self");
        assert_eq!(sig.params[1].name, "x");
        assert_eq!(sym.language_kind.0, "method");
    }

    #[test]
    fn local_var_extracted_as_value_private() {
        let src = "local count = 0\n";
        let r = extract(src, "main.lua", "main.lua");
        let sym = r.symbols.iter().find(|s| s.name == "count").expect("count");
        assert_eq!(sym.fqdn, "myapp::main::count");
        assert_eq!(sym.kind, Kind::Value);
        assert_eq!(sym.visibility, Visibility::Private);
    }

    #[test]
    fn empty_table_local_marks_module_candidate_then_private_without_return() {
        let src = "local M = {}\nfunction M.foo() end\n";
        let r = extract(src, "lib.lua", "lib.lua");
        let foo = r.symbols.iter().find(|s| s.name == "foo").expect("foo");
        assert_eq!(foo.visibility, Visibility::Private);
    }

    #[test]
    fn module_pattern_promotes_to_public_when_returned() {
        let src = "local M = {}\nfunction M.foo() end\nreturn M\n";
        let r = extract(src, "lib.lua", "lib.lua");
        let foo = r.symbols.iter().find(|s| s.name == "foo").expect("foo");
        assert_eq!(foo.visibility, Visibility::Public);
    }

    #[test]
    fn module_pattern_promotes_method_too() {
        let src = "local M = {}\nfunction M:bar() end\nreturn M\n";
        let r = extract(src, "lib.lua", "lib.lua");
        let bar = r.symbols.iter().find(|s| s.name == "bar").expect("bar");
        assert_eq!(bar.visibility, Visibility::Public);
    }

    #[test]
    fn assignment_with_function_value_extracts_symbol() {
        let src = "local M = {}\nM.alpha = function() end\nreturn M\n";
        let r = extract(src, "lib.lua", "lib.lua");
        let a = r.symbols.iter().find(|s| s.name == "alpha").expect("alpha");
        assert_eq!(a.fqdn, "myapp::lib::M::alpha");
        assert_eq!(a.kind, Kind::Function);
        assert_eq!(a.visibility, Visibility::Public);
    }

    #[test]
    fn require_with_parens_emits_imports_edge() {
        let src = "local strings = require(\"utils.strings\")\n";
        let r = extract(src, "main.lua", "main.lua");
        let imports: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect();
        assert_eq!(imports.len(), 1);
        match &imports[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "utils.strings"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn require_with_string_arg_no_parens_emits_imports_edge() {
        let src = "local x = require \"json\"\n";
        let r = extract(src, "main.lua", "main.lua");
        let imports: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect();
        assert_eq!(imports.len(), 1);
        match &imports[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "json"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn top_level_function_call_emits_calls_edge() {
        let src = "doStuff()\n";
        let r = extract(src, "main.lua", "main.lua");
        let calls: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].from_fqdn, "myapp::main");
        match &calls[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "doStuff"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn dotted_call_recorded_with_dotted_name() {
        let src = "M.greet(\"hi\")\n";
        let r = extract(src, "main.lua", "main.lua");
        let calls: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1);
        match &calls[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "M.greet"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn method_call_recorded_with_colon_name() {
        let src = "obj:run(1)\n";
        let r = extract(src, "main.lua", "main.lua");
        let calls: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1);
        match &calls[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "obj:run"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn nested_call_inside_function_body_records_call_from_caller() {
        let src = "local function caller() callee() end\n";
        let r = extract(src, "main.lua", "main.lua");
        let calls: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].from_fqdn, "myapp::main::caller");
        match &calls[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "callee"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn doc_block_attached_to_local_function() {
        let src = "--- doubles its argument\nlocal function dbl(x) return x*2 end\n";
        let r = extract(src, "main.lua", "main.lua");
        let doc = r
            .documents
            .iter()
            .find(|d| d.symbol_fqdn == "myapp::main::dbl")
            .expect("doc on dbl");
        assert_eq!(doc.description, "doubles its argument");
    }

    #[test]
    fn parse_error_returns_extract_error() {
        let err = extract_file(
            "local x =",
            "broken.lua",
            "myapp",
            "broken.lua",
            &PathBuf::from("/tmp/pkg/broken.lua"),
            &PathBuf::from("/tmp/pkg"),
        )
        .expect_err("must fail");
        match err {
            ExtractError::Parse { file, .. } => assert_eq!(file, "broken.lua"),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn vararg_function_yields_ellipsis_param() {
        let src = "local function f(...) end\n";
        let r = extract(src, "main.lua", "main.lua");
        let sym = r.symbols.iter().find(|s| s.name == "f").expect("f");
        let sig = sym.signature.as_ref().expect("sig");
        assert_eq!(sig.params[0].name, "...");
    }

    #[test]
    fn body_hash_present_for_function() {
        let src = "local function noop() end\n";
        let r = extract(src, "main.lua", "main.lua");
        let sym = r.symbols.iter().find(|s| s.name == "noop").expect("noop");
        assert!(sym.body_hash.is_some());
    }

    #[test]
    fn module_symbol_body_hash_equals_content_hash() {
        let src = "local x = 1\n";
        let r = extract(src, "main.lua", "main.lua");
        let module = &r.symbols[0];
        assert_eq!(module.body_hash, Some(r.content_hash));
    }

    #[test]
    fn language_is_lua() {
        let r = extract("local x = 1\n", "main.lua", "main.lua");
        assert_eq!(r.language, Language::Lua);
    }

    #[test]
    fn nested_table_field_function_only_supported_one_level_day_one() {
        // `function M.sub.foo()` is two-level — fully supported.
        let src = "function M.sub.foo() end\n";
        let r = extract(src, "lib.lua", "lib.lua");
        let foo = r.symbols.iter().find(|s| s.name == "foo").expect("foo");
        assert_eq!(foo.fqdn, "myapp::lib::M::sub::foo");
    }

    #[test]
    fn require_inside_function_body_is_recorded_against_caller() {
        let src = "local function init() local m = require(\"sys\") end\n";
        let r = extract(src, "main.lua", "main.lua");
        let imports: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].from_fqdn, "myapp::main::init");
    }

    #[test]
    fn stage3e1_lua_call_top_level_builtin_resolves_to_synthetic() {
        // `print` is registered top-level (Edge tier, Console tag) —
        // the call now produces a `Resolved` edge to the synthetic
        // FQDN with `via-builtin` + `builtin-console` attrs, replacing
        // the pre-3e-1 Unresolved fallthrough.
        let src = "print(\"hi\")\n";
        let r = extract(src, "main.lua", "main.lua");
        let calls: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1);
        match &calls[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => {
                assert!(
                    fqdn.starts_with("<builtin>::"),
                    "expected synthetic FQDN, got {fqdn}"
                );
                assert!(fqdn.ends_with("::print"), "got {fqdn}");
            }
            other => panic!("expected Resolved synthetic, got {other:?}"),
        }
        assert!(calls[0].attributes.contains(&"via-builtin".to_string()));
        assert!(calls[0].attributes.contains(&"builtin-console".to_string()));
    }

    #[test]
    fn stage3e1_lua_call_dotted_builtin_member_resolves() {
        // `table.insert` is an explicitly enumerated hot member (Iter
        // tag). Full-path lookup hits the registered entry, no fallback
        // needed.
        let src = "local t = {}\ntable.insert(t, 1)\n";
        let r = extract(src, "main.lua", "main.lua");
        let calls: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1);
        match &calls[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => {
                assert!(fqdn.ends_with("::table.insert"), "got {fqdn}");
            }
            other => panic!("expected Resolved synthetic, got {other:?}"),
        }
        assert!(calls[0].attributes.contains(&"builtin-iter".to_string()));
    }

    #[test]
    fn stage3e1_lua_call_unenumerated_module_member_falls_back_to_module() {
        // `os.tmpname()` isn't enumerated as a hot member — the full
        // lookup misses but the leftmost-segment fallback hits the
        // `os` module entry. The edge points at the module synthetic,
        // signalling "this code touches the os stdlib module". (The
        // visitor only enumerates top-level statement calls, so we
        // use the call as a statement rather than an assignment RHS.)
        let src = "os.tmpname()\n";
        let r = extract(src, "main.lua", "main.lua");
        let calls: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1);
        match &calls[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => {
                assert!(fqdn.ends_with("::os"), "got {fqdn}");
            }
            other => panic!("expected Resolved synthetic for os fallback, got {other:?}"),
        }
    }

    #[test]
    fn stage3e1_lua_call_user_dotted_call_stays_unresolved() {
        // `M.greet` is NOT a builtin (neither full nor leftmost) —
        // unchanged from pre-3e-1: emitted as Unresolved canonical.
        // Guards against the leftmost fallback over-matching.
        let src = "M.greet(\"hi\")\n";
        let r = extract(src, "main.lua", "main.lua");
        let calls: Vec<_> = r
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1);
        match &calls[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "M.greet");
            }
            other => panic!("user dotted call must stay Unresolved, got {other:?}"),
        }
        assert!(calls[0].attributes.is_empty());
    }

    // --- IR-4-d: call_sites emission (observational, separate from edges) ---

    #[test]
    fn ir4d_free_fn_call_emits_call_site_with_classified_args() {
        // `foo("hi", 42, x)` — three positional args. Args must classify
        // string-literals vs identifiers/numbers correctly.
        let src = "local function caller() local x = 0 foo(\"hi\", 42, x) end\n";
        let r = extract(src, "main.lua", "main.lua");
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text == "foo")
            .unwrap_or_else(|| panic!("expected foo call_site, got {:?}", r.call_sites));
        assert!(cs.receiver_chain.is_empty());
        assert_eq!(cs.args.len(), 3);
        assert_eq!(cs.args[0].value, "hi");
        assert!(cs.args[0].is_string_literal);
        assert!(!cs.args[1].is_string_literal);
        assert!(cs.args[1].value.contains("42"));
        assert_eq!(cs.args[2].value, "x");
        assert!(!cs.args[2].is_string_literal);
    }

    #[test]
    fn ir4d_dotted_call_emits_chain_and_dotted_callee_text() {
        // `M.api.create(payload)` — chain=["M","api"], callee_text="M.api.create".
        let src = "local function caller() M.api.create(payload) end\n";
        let r = extract(src, "main.lua", "main.lua");
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text == "M.api.create")
            .unwrap_or_else(|| panic!("expected M.api.create call_site, got {:?}", r.call_sites));
        assert_eq!(cs.receiver_chain, vec!["M".to_string(), "api".to_string()]);
        assert_eq!(cs.args.len(), 1);
        assert_eq!(cs.args[0].value, "payload");
    }

    #[test]
    fn ir4d_method_colon_call_uses_colon_in_callee_text() {
        // `obj:bar(x)` — Lua method-call syntax. callee_text="obj:bar",
        // receiver_chain=["obj"]. The colon survives into the textual
        // record so plugins can distinguish from `obj.bar(x)`.
        let src = "local function caller() obj:bar(x) end\n";
        let r = extract(src, "main.lua", "main.lua");
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text == "obj:bar")
            .unwrap_or_else(|| panic!("expected obj:bar call_site, got {:?}", r.call_sites));
        assert_eq!(cs.receiver_chain, vec!["obj".to_string()]);
    }

    #[test]
    fn ir4d_string_call_syntax_carries_single_string_arg() {
        // `require "modname"` — the `foo "literal"` shorthand surfaces
        // as a single string-literal arg. (`require` is special-cased
        // for the Imports edge BEFORE the call_site emit — so we test
        // with a non-require fn to actually see the call_site.)
        let src = "local function caller() greet \"world\" end\n";
        let r = extract(src, "main.lua", "main.lua");
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text == "greet")
            .unwrap_or_else(|| panic!("expected greet call_site, got {:?}", r.call_sites));
        assert_eq!(cs.args.len(), 1);
        assert!(cs.args[0].is_string_literal);
        assert_eq!(cs.args[0].value, "world");
    }

    #[test]
    fn ir4d_table_constructor_call_carries_single_non_literal_arg() {
        // `foo {1, 2, 3}` — table-constructor shorthand. value carries
        // the table source text; is_string_literal=false.
        let src = "local function caller() foo {1, 2, 3} end\n";
        let r = extract(src, "main.lua", "main.lua");
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text == "foo")
            .unwrap_or_else(|| panic!("expected foo call_site, got {:?}", r.call_sites));
        assert_eq!(cs.args.len(), 1);
        assert!(!cs.args[0].is_string_literal);
        assert!(cs.args[0].value.contains('1'));
    }

    #[test]
    fn ir4d_builtin_call_still_emits_call_site() {
        // `print(x)` — `print` is an Edge-tier Lua builtin (resolves
        // to a synthetic FQDN with `via-builtin`). The call_site must
        // still surface independently of the edge classification.
        let src = "local function caller() print(\"hi\") end\n";
        let r = extract(src, "main.lua", "main.lua");
        assert!(
            r.call_sites.iter().any(|c| c.callee_text == "print"),
            "print call_site must surface, got {:?}",
            r.call_sites
        );
    }

    #[test]
    fn ir4d_require_emits_import_edge_but_no_call_site() {
        // `require("mod")` — special-cased for the IMPORTS edge before
        // the call_site emit. Plugins reading require usage should
        // walk edges, not call_sites — keeping the textual record
        // empty avoids double-counting.
        let src = "local M = require(\"sibling\")\n";
        let r = extract(src, "main.lua", "main.lua");
        assert!(
            r.call_sites.iter().all(|c| c.callee_text != "require"),
            "require should not emit a call_site, got {:?}",
            r.call_sites
        );
    }

    #[test]
    fn ir4d_call_site_from_fqdn_attributes_to_enclosing_function() {
        // `function svc:run() helper() end` — the call_site for
        // `helper()` must have `from_fqdn = myapp::main::svc::run`,
        // not the module fqdn.
        let src = "function svc:run() helper() end\n";
        let r = extract(src, "main.lua", "main.lua");
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text == "helper")
            .unwrap_or_else(|| panic!("expected helper call_site, got {:?}", r.call_sites));
        assert_eq!(cs.from_fqdn, "myapp::main::svc::run");
    }

    fn decl_kind_of(file: &ExtractedFile, fqdn: &str) -> Option<DeclKind> {
        file.symbols
            .iter()
            .find(|s| s.fqdn == fqdn)
            .unwrap_or_else(|| panic!("symbol {fqdn} not found in {:?}", file.symbols))
            .decl_kind
            .clone()
    }

    #[test]
    fn decl_kind_module_for_file_module() {
        let r = extract("", "main.lua", "main.lua");
        assert_eq!(decl_kind_of(&r, "myapp::main"), Some(DeclKind::Module));
    }

    #[test]
    fn decl_kind_function_for_local_function() {
        let src = "local function helper() end\n";
        let r = extract(src, "main.lua", "main.lua");
        assert_eq!(
            decl_kind_of(&r, "myapp::main::helper"),
            Some(DeclKind::Function),
        );
    }

    #[test]
    fn decl_kind_function_for_global_function() {
        let src = "function greet() end\n";
        let r = extract(src, "main.lua", "main.lua");
        assert_eq!(
            decl_kind_of(&r, "myapp::main::greet"),
            Some(DeclKind::Function),
        );
    }

    #[test]
    fn decl_kind_function_for_dotted_function() {
        let src = "function M.foo() end\n";
        let r = extract(src, "lib.lua", "lib.lua");
        assert_eq!(
            decl_kind_of(&r, "myapp::lib::M::foo"),
            Some(DeclKind::Function),
        );
    }

    #[test]
    fn decl_kind_method_for_colon_function() {
        let src = "function M:bar() end\n";
        let r = extract(src, "lib.lua", "lib.lua");
        assert_eq!(
            decl_kind_of(&r, "myapp::lib::M::bar"),
            Some(DeclKind::Method),
        );
    }

    #[test]
    fn decl_kind_var_for_local_assignment() {
        let src = "local counter = 0\n";
        let r = extract(src, "main.lua", "main.lua");
        assert_eq!(
            decl_kind_of(&r, "myapp::main::counter"),
            Some(DeclKind::Var),
        );
    }

    #[test]
    fn decl_kind_function_for_table_assignment_with_function_rhs() {
        // `M.foo = function() end` — extract_assignment emits as Kind::Function
        // because the RHS is a function literal.
        let src = "local M = {}\nM.foo = function() end\n";
        let r = extract(src, "lib.lua", "lib.lua");
        assert_eq!(
            decl_kind_of(&r, "myapp::lib::M::foo"),
            Some(DeclKind::Function),
        );
    }
}
