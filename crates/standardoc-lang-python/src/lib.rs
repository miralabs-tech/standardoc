//! Python language provider for Standardoc.
//!
//! Uses `rustpython-parser` to discover documentable symbols at the
//! **module top-level** plus class members. v1 covers:
//!
//! - `def` / `async def` (functions, methods)
//! - `class` (with their methods)
//! - PEP 257 docstrings (first expression-string in body)
//! - Type hints on params and return → `ParamType` / `ReturnType` cross-refs
//! - `class X(Base):` → `Extends -> Base` on the class
//!
//! Visibility convention: leading underscore = `Private`, anything else =
//! `Public`. Double-underscore (`__name__`) is treated as `Private` too
//! (name-mangled or dunder).
//!
//! Out of scope for v1:
//! - Nested functions/classes (only top-level + class members)
//! - `@dataclass` field auto-extraction (we'd need to special-case the decorator)
//! - Module-level constants (`X: int = 0`) — easy to add later
//! - Star imports / re-exports

#![forbid(unsafe_code)]

use rustpython_parser::ast::{
    self, Arguments, Expr, ExprName, Stmt, StmtAsyncFunctionDef, StmtClassDef, StmtFunctionDef,
};
use rustpython_parser::Parse;
use standardoc_core::lang::{DiscoveredSymbol, LanguageProvider, ParseError};
use standardoc_core::model::{
    CommentStyles, ParamInfo, RefKind, References, SourceRange, SymbolInfo, SymbolKind, TypeInfo,
    Visibility,
};
use std::path::Path;
use std::sync::OnceLock;

/// @doc lang.providers.python Python
/// @description Native Python language provider — discovers classes, functions, decorators via the `rustpython-parser` AST.
/// @crate standardoc-lang-python
/// @backend rustpython-parser
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonProvider;

impl LanguageProvider for PythonProvider {
    fn id(&self) -> &'static str {
        "python"
    }

    fn extensions(&self) -> &[&'static str] {
        &[".py", ".pyi"]
    }

    fn comment_styles(&self) -> &CommentStyles {
        python_comment_styles()
    }

    fn discover_symbols(
        &self,
        content: &str,
        path: &Path,
    ) -> Result<Vec<DiscoveredSymbol>, ParseError> {
        let module = ast::Suite::parse(content, &path.display().to_string()).map_err(|err| {
            ParseError::Syntax {
                path: path.display().to_string(),
                message: err.to_string(),
            }
        })?;

        let mut collector = Collector {
            path: Vec::new(),
            symbols: Vec::new(),
        };
        for stmt in &module {
            collector.visit_top(stmt);
        }
        Ok(collector.symbols)
    }
}

fn python_comment_styles() -> &'static CommentStyles {
    static STYLES: OnceLock<CommentStyles> = OnceLock::new();
    STYLES.get_or_init(|| CommentStyles {
        single: vec!["#".to_owned()],
        multi: None,
        // PEP 257 docstrings are string literals, not a comment style.
        // Standardoc providers retrieve docstrings directly from AST ->
        // `comment_styles` has nothing to expose here.
        doc_single: vec![],
        doc_multi: None,
    })
}

struct Collector {
    path: Vec<String>,
    symbols: Vec<DiscoveredSymbol>,
}

impl Collector {
    fn visit_top(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => self.emit_fn(f, false),
            Stmt::AsyncFunctionDef(f) => self.emit_async_fn(f),
            Stmt::ClassDef(c) => self.emit_class(c),
            _ => {}
        }
    }

    fn fqn_for(&self, name: &str) -> Vec<String> {
        let mut fqn = self.path.clone();
        fqn.push(name.to_owned());
        fqn
    }

    fn emit_fn(&mut self, f: &StmtFunctionDef, is_method: bool) {
        let name = f.name.to_string();
        let fqn = self.fqn_for(&name);
        let kind = if is_method {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        let visibility = visibility_from_name(&name);
        let params = params_from_arguments(&f.args);
        let returns = f.returns.as_ref().map(|ty| TypeInfo {
            repr: type_to_string(ty),
        });
        let signature = format_signature(&name, &params, returns.as_ref(), false);
        let leading_comment = extract_docstring(&f.body);
        let references = collect_fn_references(&f.args, f.returns.as_deref());

        let symbol = SymbolInfo {
            kind,
            visibility,
            signature,
            params,
            returns,
            generics: vec![],
            decorators: decorator_strings(&f.decorator_list),
            is_async: false,
            is_deprecated: false,
            references,
        };
        self.symbols.push(DiscoveredSymbol {
            fqn,
            symbol,
            source_range: range_from_stmt_function(f),
            leading_comment,
            leading_comment_line_start: None,
        });
    }

    fn emit_async_fn(&mut self, f: &StmtAsyncFunctionDef) {
        let name = f.name.to_string();
        let fqn = self.fqn_for(&name);
        let visibility = visibility_from_name(&name);
        let params = params_from_arguments(&f.args);
        let returns = f.returns.as_ref().map(|ty| TypeInfo {
            repr: type_to_string(ty),
        });
        let signature = format_signature(&name, &params, returns.as_ref(), true);
        let leading_comment = extract_docstring(&f.body);
        let references = collect_fn_references(&f.args, f.returns.as_deref());

        let symbol = SymbolInfo {
            kind: SymbolKind::Function,
            visibility,
            signature,
            params,
            returns,
            generics: vec![],
            decorators: decorator_strings(&f.decorator_list),
            is_async: true,
            is_deprecated: false,
            references,
        };
        self.symbols.push(DiscoveredSymbol {
            fqn,
            symbol,
            source_range: range_from_stmt_async_function(f),
            leading_comment,
            leading_comment_line_start: None,
        });
    }

    fn emit_class(&mut self, c: &StmtClassDef) {
        let name = c.name.to_string();
        let fqn = self.fqn_for(&name);
        let visibility = visibility_from_name(&name);
        let leading_comment = extract_docstring(&c.body);

        // Extends -> add `Extends` for each base class.
        let mut references = References::default();
        for base in &c.bases {
            if let Some(target) = expr_short_name(base) {
                if !is_python_builtin(&target) {
                    references.push(RefKind::Extends, target, 0);
                }
            }
        }

        let symbol = SymbolInfo {
            kind: SymbolKind::Class,
            visibility,
            signature: format!("class {name}"),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: decorator_strings(&c.decorator_list),
            is_async: false,
            is_deprecated: false,
            references,
        };
        self.symbols.push(DiscoveredSymbol {
            fqn,
            symbol,
            source_range: range_from_stmt_class(c),
            leading_comment,
            leading_comment_line_start: None,
        });

        // Walk members (methods and nested classes).
        self.path.push(name);
        for stmt in &c.body {
            match stmt {
                Stmt::FunctionDef(f) => self.emit_fn(f, true),
                Stmt::AsyncFunctionDef(f) => self.emit_async_fn(f),
                Stmt::ClassDef(nested) => self.emit_class(nested),
                _ => {}
            }
        }
        self.path.pop();
    }
}

fn visibility_from_name(name: &str) -> Visibility {
    if name.starts_with('_') {
        Visibility::Private
    } else {
        Visibility::Public
    }
}

fn params_from_arguments(args: &Arguments) -> Vec<ParamInfo> {
    let mut out: Vec<ParamInfo> = Vec::new();
    for arg in &args.posonlyargs {
        out.push(param_from_arg(&arg.def, false, false));
    }
    for arg in &args.args {
        out.push(param_from_arg(&arg.def, false, false));
    }
    if let Some(vararg) = &args.vararg {
        out.push(param_from_arg(vararg, true, false));
    }
    for arg in &args.kwonlyargs {
        out.push(param_from_arg(&arg.def, false, false));
    }
    if let Some(kwarg) = &args.kwarg {
        out.push(param_from_arg(kwarg, false, true));
    }
    out
}

fn param_from_arg(arg: &ast::Arg, is_variadic: bool, is_kwargs: bool) -> ParamInfo {
    let name = if is_kwargs {
        format!("**{}", arg.arg)
    } else if is_variadic {
        format!("*{}", arg.arg)
    } else {
        arg.arg.to_string()
    };
    let type_repr = arg.annotation.as_ref().map(|ty| type_to_string(ty));
    ParamInfo {
        name,
        type_repr,
        default: None,
        is_optional: is_kwargs,
        is_variadic,
    }
}

fn collect_fn_references(args: &Arguments, returns: Option<&Expr>) -> References {
    let mut refs = References::default();
    let mut all_args: Vec<&ast::Arg> = Vec::new();
    for arg in &args.posonlyargs {
        all_args.push(&arg.def);
    }
    for arg in &args.args {
        all_args.push(&arg.def);
    }
    if let Some(v) = &args.vararg {
        all_args.push(v);
    }
    for arg in &args.kwonlyargs {
        all_args.push(&arg.def);
    }
    if let Some(k) = &args.kwarg {
        all_args.push(k);
    }
    for arg in all_args {
        if let Some(ann) = &arg.annotation {
            for name in collect_type_idents(ann) {
                refs.push(RefKind::ParamType, name, 0);
            }
        }
    }
    if let Some(ret) = returns {
        for name in collect_type_idents(ret) {
            refs.push(RefKind::ReturnType, name, 0);
        }
    }
    refs
}

/// Walk an Expr node (used as type) and collect identifier names
/// non-builtins. `Optional[Foo]`, `List[Bar]`, `Dict[str, Baz]` → on garde
/// non-builtin types (`Foo`, `Bar`, `Baz`).
fn collect_type_idents(expr: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    walk_type(expr, &mut out);
    out.into_iter().filter(|n| !is_python_builtin(n)).collect()
}

fn walk_type(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Name(ExprName { id, .. }) => {
            out.push(id.to_string());
        }
        Expr::Attribute(attr) => {
            // Pour `module.Type`, on garde juste `Type`.
            out.push(attr.attr.to_string());
        }
        Expr::Subscript(sub) => {
            walk_type(&sub.value, out);
            walk_type(&sub.slice, out);
        }
        Expr::Tuple(t) => {
            for elt in &t.elts {
                walk_type(elt, out);
            }
        }
        Expr::List(l) => {
            for elt in &l.elts {
                walk_type(elt, out);
            }
        }
        Expr::BinOp(bin) => {
            // Pour `Foo | Bar` (union types Python 3.10+).
            walk_type(&bin.left, out);
            walk_type(&bin.right, out);
        }
        _ => {}
    }
}

fn expr_short_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(n.id.to_string()),
        Expr::Attribute(a) => Some(a.attr.to_string()),
        Expr::Call(c) => expr_short_name(&c.func),
        _ => None,
    }
}

fn is_python_builtin(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "float"
            | "str"
            | "bool"
            | "bytes"
            | "bytearray"
            | "None"
            | "type"
            | "object"
            | "list"
            | "tuple"
            | "dict"
            | "set"
            | "frozenset"
            | "complex"
            | "range"
            | "slice"
            | "Any"
            | "Optional"
            | "Union"
            | "List"
            | "Dict"
            | "Tuple"
            | "Set"
            | "FrozenSet"
            | "Iterable"
            | "Iterator"
            | "Generator"
            | "AsyncIterable"
            | "AsyncIterator"
            | "Awaitable"
            | "Coroutine"
            | "Callable"
            | "Mapping"
            | "MutableMapping"
            | "Sequence"
            | "MutableSequence"
            | "Type"
            | "ClassVar"
            | "Final"
            | "Literal"
            | "TypeVar"
            | "Generic"
            | "Self"
            | "Ellipsis"
    )
}

fn extract_docstring(body: &[Stmt]) -> Option<String> {
    let first = body.first()?;
    let Stmt::Expr(expr_stmt) = first else {
        return None;
    };
    let Expr::Constant(c) = expr_stmt.value.as_ref() else {
        return None;
    };
    if let ast::Constant::Str(s) = &c.value {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    } else {
        None
    }
}

fn decorator_strings(decorators: &[Expr]) -> Vec<String> {
    decorators
        .iter()
        .filter_map(|d| match d {
            Expr::Name(n) => Some(format!("@{}", n.id)),
            Expr::Attribute(a) => Some(format!("@{}", a.attr)),
            Expr::Call(c) => expr_short_name(&c.func).map(|n| format!("@{n}")),
            _ => None,
        })
        .collect()
}

fn type_to_string(expr: &Expr) -> String {
    match expr {
        Expr::Name(n) => n.id.to_string(),
        Expr::Attribute(a) => format!("{}.{}", type_to_string(&a.value), a.attr),
        Expr::Subscript(sub) => {
            format!(
                "{}[{}]",
                type_to_string(&sub.value),
                type_to_string(&sub.slice)
            )
        }
        Expr::Tuple(t) => t
            .elts
            .iter()
            .map(type_to_string)
            .collect::<Vec<_>>()
            .join(", "),
        Expr::Constant(c) => match &c.value {
            ast::Constant::None => "None".to_owned(),
            ast::Constant::Str(s) => format!("\"{s}\""),
            _ => "<const>".to_owned(),
        },
        Expr::BinOp(bin) => format!(
            "{} | {}",
            type_to_string(&bin.left),
            type_to_string(&bin.right)
        ),
        _ => "<expr>".to_owned(),
    }
}

fn format_signature(
    name: &str,
    params: &[ParamInfo],
    returns: Option<&TypeInfo>,
    is_async: bool,
) -> String {
    let prefix = if is_async { "async def" } else { "def" };
    let params_str = params
        .iter()
        .map(|p| {
            p.type_repr
                .as_ref()
                .map_or_else(|| p.name.clone(), |t| format!("{}: {t}", p.name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let return_str = returns
        .map(|r| format!(" -> {}", r.repr))
        .unwrap_or_default();
    format!("{prefix} {name}({params_str}){return_str}")
}

// -------- Source ranges --------
//
// `rustpython-parser` produces positions under generic wrappers.
// We extract lines for `SourceRange`. If a future platform lacks the
// `source-position` feature, fallback is `(0, 0)`.

const fn range_from_stmt_function(f: &StmtFunctionDef) -> SourceRange {
    range_from_pos(f.range)
}

const fn range_from_stmt_async_function(f: &StmtAsyncFunctionDef) -> SourceRange {
    range_from_pos(f.range)
}

const fn range_from_stmt_class(c: &StmtClassDef) -> SourceRange {
    range_from_pos(c.range)
}

const fn range_from_pos(range: rustpython_parser::text_size::TextRange) -> SourceRange {
    // Les `TextRange` de rustpython sont en byte offsets. On fait un fallback
    // line 1 if mapping fails. Precise line:column mapping would require a
    // separate `LineIndex` — TODO if needed.
    let _ = range;
    SourceRange {
        line_start: 1,
        line_end: 1,
        column_start: 1,
        column_end: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discover(src: &str) -> Vec<DiscoveredSymbol> {
        let p = PythonProvider;
        p.discover_symbols(src, Path::new("test.py")).unwrap()
    }

    #[test]
    fn discovers_top_level_function() {
        let src = "def hello(name: str) -> str:\n    return f'hi {name}'\n";
        let symbols = discover(src);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].fqn, vec!["hello"]);
        assert_eq!(symbols[0].symbol.kind, SymbolKind::Function);
        assert_eq!(symbols[0].symbol.visibility, Visibility::Public);
        assert_eq!(symbols[0].symbol.params.len(), 1);
        assert_eq!(symbols[0].symbol.params[0].name, "name");
    }

    #[test]
    fn private_name_with_underscore_prefix() {
        let src = "def _internal():\n    pass\n";
        let symbols = discover(src);
        assert_eq!(symbols[0].symbol.visibility, Visibility::Private);
    }

    #[test]
    fn extracts_docstring_as_leading_comment() {
        let src = "def greet():\n    \"\"\"Says hi.\"\"\"\n    pass\n";
        let symbols = discover(src);
        assert_eq!(symbols[0].leading_comment.as_deref(), Some("Says hi."));
    }

    #[test]
    fn class_and_methods() {
        let src = "class Calculator:\n    \"\"\"A demo calculator.\"\"\"\n    def add(self, a: int, b: int) -> int:\n        return a + b\n";
        let symbols = discover(src);
        let class = symbols
            .iter()
            .find(|s| s.symbol.kind == SymbolKind::Class)
            .unwrap();
        assert_eq!(class.fqn, vec!["Calculator"]);
        assert_eq!(class.leading_comment.as_deref(), Some("A demo calculator."));
        let method = symbols
            .iter()
            .find(|s| s.symbol.kind == SymbolKind::Method)
            .unwrap();
        assert_eq!(method.fqn, vec!["Calculator", "add"]);
    }

    #[test]
    fn async_function_marked_async() {
        let src = "async def fetch() -> str:\n    return ''\n";
        let symbols = discover(src);
        assert!(symbols[0].symbol.is_async);
    }

    #[test]
    fn cross_ref_param_type() {
        let src = "class User: pass\ndef store(u: User) -> bool:\n    return True\n";
        let symbols = discover(src);
        let store = symbols.iter().find(|s| s.fqn == vec!["store"]).unwrap();
        assert!(store
            .symbol
            .references
            .outgoing
            .iter()
            .any(|r| r.kind == RefKind::ParamType && r.target == "User"));
    }

    #[test]
    fn cross_ref_extends_on_class() {
        let src = "class Base: pass\nclass Derived(Base):\n    pass\n";
        let symbols = discover(src);
        let derived = symbols.iter().find(|s| s.fqn == vec!["Derived"]).unwrap();
        assert!(derived
            .symbol
            .references
            .outgoing
            .iter()
            .any(|r| r.kind == RefKind::Extends && r.target == "Base"));
    }

    #[test]
    fn primitives_are_filtered_from_param_refs() {
        let src = "def add(a: int, b: int) -> int:\n    return a + b\n";
        let symbols = discover(src);
        // `int` should NOT appear as a ParamType ref.
        assert!(symbols[0]
            .symbol
            .references
            .outgoing
            .iter()
            .all(|r| r.target != "int"));
    }

    #[test]
    fn signature_format_includes_async_and_returns() {
        let src = "async def fetch(url: str) -> bytes:\n    return b''\n";
        let symbols = discover(src);
        assert!(symbols[0].symbol.signature.contains("async def fetch"));
        assert!(symbols[0].symbol.signature.contains("url: str"));
        assert!(symbols[0].symbol.signature.contains("-> bytes"));
    }
}
