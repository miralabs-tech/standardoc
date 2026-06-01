use super::{WorkspaceFile, collect_global_returns};

fn file<'a>(crate_name: &'a str, crate_rel: &'a str, content: &'a str) -> WorkspaceFile<'a> {
    WorkspaceFile {
        crate_name,
        crate_rel,
        content,
    }
}

#[test]
fn collects_free_fn_return_type_under_crate_root() {
    let files = vec![file(
        "foo",
        "src/lib.rs",
        "pub fn get_user(id: u64) -> Option<User> { todo!() }",
    )];
    let reg = collect_global_returns(&files);
    assert_eq!(reg.lookup("foo::get_user"), Some("Option<User>"));
}

#[test]
fn collects_impl_method_return_type_under_self_ident() {
    let files = vec![file(
        "foo",
        "src/lib.rs",
        "struct Repo; impl Repo { pub fn find(&self, id: u64) -> Option<User> { todo!() } }",
    )];
    let reg = collect_global_returns(&files);
    assert_eq!(reg.lookup("foo::Repo::find"), Some("Option<User>"));
}

#[test]
fn collects_inline_module_fn_under_nested_path() {
    let files = vec![file(
        "foo",
        "src/lib.rs",
        "mod inner { pub fn make() -> Thing { todo!() } }",
    )];
    let reg = collect_global_returns(&files);
    assert_eq!(reg.lookup("foo::inner::make"), Some("Thing"));
}

#[test]
fn non_nominal_return_types_are_skipped() {
    let files = vec![file(
        "foo",
        "src/lib.rs",
        "pub fn unit() {} \
         pub fn tuple() -> (u32, String) { todo!() } \
         pub fn closure() -> Box<dyn Fn() -> u32> { todo!() }",
    )];
    let reg = collect_global_returns(&files);
    assert_eq!(reg.lookup("foo::unit"), None);
    assert_eq!(reg.lookup("foo::tuple"), None);
    // closure return is a Box<...> = nominal `Box`; tracked. The inner
    // dyn-trait arg collapses to `_` via `parametric_type` (matches the
    // per-file `ReturnTypeTable` shape — Pass 0 must use the same
    // canonical form so the lookup chain is uniform).
    assert_eq!(reg.lookup("foo::closure"), Some("Box<_>"));
}

#[test]
fn unparseable_files_are_silently_skipped() {
    let files = vec![
        file("foo", "src/lib.rs", "pub fn good() -> Thing { todo!() }"),
        file("foo", "src/broken.rs", "fn bad( ! ! { totally bogus"),
    ];
    let reg = collect_global_returns(&files);
    // The valid file still got walked.
    assert_eq!(reg.lookup("foo::good"), Some("Thing"));
}

#[test]
fn cross_file_fns_share_one_registry_keyed_by_absolute_fqdn() {
    let files = vec![
        file(
            "foo",
            "src/lib.rs",
            "pub fn get_user(id: u64) -> Option<User> { todo!() }",
        ),
        file(
            "bar",
            "src/lib.rs",
            "pub fn other() -> Result<Data, Err> { todo!() }",
        ),
    ];
    let reg = collect_global_returns(&files);
    assert_eq!(reg.lookup("foo::get_user"), Some("Option<User>"));
    assert_eq!(reg.lookup("bar::other"), Some("Result<Data, Err>"));
}

#[test]
fn impl_methods_for_parametric_self_type_use_nominal_head() {
    // `impl Foo<Bar> { fn make() -> Quux }` records under `foo::Foo::make`
    // (nominal head only) — matches the per-file `extract_call` lookup
    // convention.
    let files = vec![file(
        "foo",
        "src/lib.rs",
        "struct Foo<T>(T); impl Foo<i32> { pub fn make() -> Quux { todo!() } }",
    )];
    let reg = collect_global_returns(&files);
    assert_eq!(reg.lookup("foo::Foo::make"), Some("Quux"));
}

#[test]
fn module_path_compute_lib_rs_uses_crate_name_only() {
    // sanity: a `lib.rs` file at crate root collapses the module fqdn
    // to the crate name itself, not `crate::lib`.
    let files = vec![file(
        "mycrate",
        "src/lib.rs",
        "pub fn entry() -> bool { true }",
    )];
    let reg = collect_global_returns(&files);
    assert_eq!(reg.lookup("mycrate::entry"), Some("bool"));
    assert_eq!(reg.lookup("mycrate::lib::entry"), None);
}
