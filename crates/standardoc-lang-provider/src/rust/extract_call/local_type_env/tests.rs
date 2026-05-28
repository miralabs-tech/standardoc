use super::*;

fn parse_local(src: &str) -> Local {
    let stmt: syn::Stmt = syn::parse_str(src).expect("parse Stmt");
    match stmt {
        syn::Stmt::Local(l) => l,
        _ => panic!("not a Local: {src}"),
    }
}

fn parse_inputs(sig: &str) -> Punctuated<FnArg, Token![,]> {
    let item: syn::ItemFn = syn::parse_str(&format!("{sig} {{}}")).expect("parse ItemFn");
    item.sig.inputs
}

#[test]
fn from_fn_params_captures_annotated_ident_args() {
    let inputs = parse_inputs("fn f(x: Vec<u8>, y: &Foo, z: HashMap<K, V>)");
    let env = LocalTypeEnv::from_fn_params(&inputs);
    // Bug E-3 ext P-E3.2: bindings now parametric — generics preserved
    // so closure-arg substitution can resolve `T = u8` etc.
    assert_eq!(env.lookup("x"), Some("Vec<u8>"));
    assert_eq!(env.lookup("y"), Some("Foo"));
    assert_eq!(env.lookup("z"), Some("HashMap<K, V>"));
}

#[test]
fn closure_scope_shadows_bindings_when_pushed() {
    let inputs = parse_inputs("fn f(x: Vec<u8>)");
    let mut env = LocalTypeEnv::from_fn_params(&inputs);
    assert_eq!(env.lookup("x"), Some("Vec<u8>"));
    let mut frame = HashMap::new();
    frame.insert("x".to_string(), "u8".to_string());
    env.push_closure_scope(frame);
    assert_eq!(env.lookup("x"), Some("u8"));
    env.pop_closure_scope();
    assert_eq!(env.lookup("x"), Some("Vec<u8>"));
}

#[test]
fn closure_scopes_stack_innermost_first() {
    let mut env = LocalTypeEnv::default();
    let mut outer = HashMap::new();
    outer.insert("v".to_string(), "Outer".to_string());
    env.push_closure_scope(outer);
    let mut inner = HashMap::new();
    inner.insert("v".to_string(), "Inner".to_string());
    env.push_closure_scope(inner);
    assert_eq!(env.lookup("v"), Some("Inner"));
    env.pop_closure_scope();
    assert_eq!(env.lookup("v"), Some("Outer"));
    env.pop_closure_scope();
    assert_eq!(env.lookup("v"), None);
}

#[test]
fn from_fn_params_skips_self_receiver() {
    let inputs = parse_inputs("fn m(&self, other: &Self)");
    let env = LocalTypeEnv::from_fn_params(&inputs);
    assert_eq!(env.lookup("self"), None);
    assert_eq!(env.lookup("other"), Some("Self"));
}

#[test]
fn from_fn_params_skips_non_nominal_types() {
    let inputs = parse_inputs("fn f(closure: impl Fn(u8) -> u8, tup: (u8, u16), slice: &[u8])");
    let env = LocalTypeEnv::from_fn_params(&inputs);
    assert_eq!(env.lookup("closure"), None);
    assert_eq!(env.lookup("tup"), None);
    assert_eq!(env.lookup("slice"), None);
}

#[test]
fn record_local_annotated_wins() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let x: PathBuf = something();");
    env.record_local(&l);
    assert_eq!(env.lookup("x"), Some("PathBuf"));
}

#[test]
fn record_local_constructor_new() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let v = Vec::new();");
    env.record_local(&l);
    assert_eq!(env.lookup("v"), Some("Vec"));
}

#[test]
fn record_local_constructor_from() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let s = String::from(\"hi\");");
    env.record_local(&l);
    assert_eq!(env.lookup("s"), Some("String"));
}

#[test]
fn record_local_constructor_qualified_path() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let h = HashMap::new();");
    env.record_local(&l);
    assert_eq!(env.lookup("h"), Some("HashMap"));
}

#[test]
fn record_local_default() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let d = Foo::default();");
    env.record_local(&l);
    assert_eq!(env.lookup("d"), Some("Foo"));
}

#[test]
fn record_local_non_constructor_call_is_ignored() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let r = foo::bar();");
    env.record_local(&l);
    assert_eq!(env.lookup("r"), None);
}

#[test]
fn record_local_short_path_ignored() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let r = new();");
    env.record_local(&l);
    assert_eq!(env.lookup("r"), None);
}

#[test]
fn record_local_destructure_skipped() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let (a, b) = pair();");
    env.record_local(&l);
    assert_eq!(env.lookup("a"), None);
    assert_eq!(env.lookup("b"), None);
}

#[test]
fn record_local_no_init_no_annotation_yields_none() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let x;");
    env.record_local(&l);
    assert_eq!(env.lookup("x"), None);
}

#[test]
fn record_local_reference_init_unwraps() {
    let mut env = LocalTypeEnv::default();
    let l = parse_local("let v = &Vec::new();");
    env.record_local(&l);
    assert_eq!(env.lookup("v"), Some("Vec"));
}

#[test]
fn record_local_overwrites_on_reassignment_via_let() {
    let mut env = LocalTypeEnv::default();
    env.record_local(&parse_local("let x = Vec::new();"));
    assert_eq!(env.lookup("x"), Some("Vec"));
    env.record_local(&parse_local("let x = String::new();"));
    assert_eq!(env.lookup("x"), Some("String"));
}
