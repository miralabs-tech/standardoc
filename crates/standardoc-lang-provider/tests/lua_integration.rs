use std::fs;
use std::path::Path;

use standardoc_core::{ExtractContext, LanguageProvider};
use standardoc_ir::{EdgeKind, Kind, Language, ResolvedOrUnresolved, Visibility};
use standardoc_lang_provider::LuaProvider;
use tempfile::tempdir;

fn write(root: &Path, rel: &str, content: &str) {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(abs, content).unwrap();
}

#[test]
fn extract_with_rockspec_uses_rockspec_package_name_in_fqdn() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "mylib-1.0-1.rockspec",
        "package = \"mylib\"\nversion = \"1.0-1\"\n",
    );
    let src = "local function helper() end\n";
    write(root, "src/util.lua", src);

    let provider = LuaProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider
        .extract(src, "src/util.lua", &ctx)
        .expect("extract ok");

    let module = &extracted.symbols[0];
    assert_eq!(module.fqdn, "mylib::src::util");
    let helper = extracted
        .symbols
        .iter()
        .find(|s| s.name == "helper")
        .expect("helper");
    assert_eq!(helper.fqdn, "mylib::src::util::helper");
    assert_eq!(extracted.language, Language::Lua);
}

#[test]
fn extract_without_rockspec_falls_back_to_workspace_dir_name() {
    let dir = tempdir().unwrap();
    // Create a nested dir whose basename becomes the FQDN root.
    let root = dir.path().join("myapp");
    fs::create_dir_all(&root).unwrap();
    let src = "local function go() end\n";
    write(&root, "main.lua", src);

    let provider = LuaProvider::new();
    let ctx = ExtractContext {
        workspace_root: &root,
            cross_workspace: None,
        };
    let extracted = provider.extract(src, "main.lua", &ctx).expect("extract ok");

    let module = &extracted.symbols[0];
    assert_eq!(module.fqdn, "myapp::main");
}

#[test]
fn rockspec_is_cached_across_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "rocks.rockspec", "package = \"alpha\"\n");
    write(root, "a.lua", "local function a() end\n");
    write(root, "b.lua", "local function b() end\n");

    let provider = LuaProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };

    let _ = provider
        .extract("local function a() end\n", "a.lua", &ctx)
        .unwrap();

    // Mutate the rockspec on disk; a cache hit must keep returning "alpha".
    write(root, "rocks.rockspec", "package = \"DIFFERENT\"\n");

    let extracted = provider
        .extract("local function b() end\n", "b.lua", &ctx)
        .unwrap();
    assert_eq!(extracted.symbols[0].fqdn, "alpha::b");
}

#[test]
fn module_pattern_full_flow_with_rockspec() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "lib.rockspec", "package = \"strings\"\n");
    let src = "\
local M = {}

local function private_helper(x)
    return x
end

--- trim whitespace from both ends
function M.trim(s)
    return s
end

function M:uppercase()
    return self
end

return M
";
    write(root, "src/strings.lua", src);

    let provider = LuaProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(src, "src/strings.lua", &ctx).unwrap();

    let trim = extracted
        .symbols
        .iter()
        .find(|s| s.name == "trim")
        .expect("trim");
    assert_eq!(trim.fqdn, "strings::src::strings::M::trim");
    assert_eq!(trim.visibility, Visibility::Public);
    assert_eq!(trim.kind, Kind::Callable);

    let uppercase = extracted
        .symbols
        .iter()
        .find(|s| s.name == "uppercase")
        .expect("uppercase");
    assert_eq!(uppercase.visibility, Visibility::Public);
    let sig = uppercase.signature.as_ref().unwrap();
    assert_eq!(sig.params[0].name, "self");

    let private = extracted
        .symbols
        .iter()
        .find(|s| s.name == "private_helper")
        .expect("private_helper");
    assert_eq!(private.visibility, Visibility::Private);

    // The trim doc must be attached to its FQDN.
    let trim_doc = extracted
        .documents
        .iter()
        .find(|d| d.symbol_fqdn == trim.fqdn)
        .expect("trim doc");
    assert!(trim_doc.description.contains("trim whitespace"));
}

#[test]
fn require_imports_emits_unresolved_edge_for_dotted_path() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "main.rockspec", "package = \"app\"\n");
    let src = "local strings = require(\"utils.strings\")\nlocal function go() return strings.trim(\"  x  \") end\n";
    write(root, "main.lua", src);

    let provider = LuaProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(src, "main.lua", &ctx).unwrap();

    let imports: Vec<_> = extracted
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .collect();
    assert_eq!(imports.len(), 1);
    match &imports[0].to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "utils.strings");
        }
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn unparseable_rockspec_falls_back_to_workspace_dir() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    // `package = ` is invalid Lua, parse_rockspec_name returns None →
    // provider must not fail the file, just fall back to dir name.
    write(&root, "broken.rockspec", "package =\n");
    let src = "local x = 1\n";
    write(&root, "main.lua", src);

    let provider = LuaProvider::new();
    let ctx = ExtractContext {
        workspace_root: &root,
            cross_workspace: None,
        };
    let extracted = provider.extract(src, "main.lua", &ctx).unwrap();
    assert_eq!(extracted.symbols[0].fqdn, "project::main");
}

#[test]
fn module_pattern_does_not_promote_when_unrelated_table_returned() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "lib.rockspec", "package = \"mixlib\"\n");
    // `M` is local table but `N` is what's returned. M.foo must stay
    // Private; N.bar must be Public.
    let src = "\
local M = {}
local N = {}
function M.foo() end
function N.bar() end
return N
";
    write(root, "lib.lua", src);

    let provider = LuaProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(src, "lib.lua", &ctx).unwrap();

    let foo = extracted.symbols.iter().find(|s| s.name == "foo").unwrap();
    let bar = extracted.symbols.iter().find(|s| s.name == "bar").unwrap();
    assert_eq!(foo.visibility, Visibility::Private);
    assert_eq!(bar.visibility, Visibility::Public);
}

#[test]
fn nested_table_methods_get_full_fqdn_chain() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("nestedapp");
    fs::create_dir_all(&root).unwrap();
    let src = "function M.sub.deep:method(x) return x end\n";
    write(&root, "lib.lua", src);

    let provider = LuaProvider::new();
    let ctx = ExtractContext {
        workspace_root: &root,
            cross_workspace: None,
        };
    let extracted = provider.extract(src, "lib.lua", &ctx).unwrap();

    let m = extracted
        .symbols
        .iter()
        .find(|s| s.name == "method")
        .expect("method");
    assert_eq!(m.fqdn, "nestedapp::lib::M::sub::deep::method");
    let sig = m.signature.as_ref().unwrap();
    assert_eq!(sig.params[0].name, "self");
    assert_eq!(sig.params[1].name, "x");
}
