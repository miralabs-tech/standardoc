use standardoc_ir::{
	BindingSource, IdentResolution, ImportRecord, Language, LocalDeclKind, ModuleLookup,
	ScopeKind, ScopeRange,
};
use syn::visit::Visit;
use syn::{
	File, FnArg, GenericParam, ImplItem, ItemConst, ItemEnum, ItemFn, ItemImpl, ItemMacro,
	ItemMod, ItemStatic, ItemStruct, ItemTrait, ItemType, ItemUnion, ItemUse, Local, Pat,
	UseTree,
};

/// Build the AOT identifier-resolution table for a Rust module (parity
/// with [`crate::ts::lookup::build_ts_lookup`]).
///
/// Two-pass design:
/// 1. `hoist_items` — top-level items (fn / struct / enum / trait /
///    type alias / const / static / union / macro / nested mod) plus
///    `use` declarations populate the ROOT scope. Rust hoisting is
///    file-wide so forward refs across items are legal.
/// 2. `syn::visit::visit_file` — full traversal for nested scopes
///    (fn body blocks, impl/trait methods, closures) and let bindings.
///
/// Imports flatten into `ModuleLookup.imports` for Stage 3b cross-
/// workspace SQL resolution. `use a::{B, C as D}` expands to two
/// records.
pub(crate) fn build_rust_lookup(file: &File, module_fqdn: &str) -> ModuleLookup {
	let mut lookup = ModuleLookup::new(module_fqdn.to_string(), Language::Rust);
	let mut builder = LookupBuilder {
		lookup: &mut lookup,
		scope_stack: vec![ModuleLookup::ROOT_SCOPE],
	};
	builder.hoist_items(&file.items);
	syn::visit::visit_file(&mut builder, file);
	lookup
}

struct LookupBuilder<'a> {
	lookup: &'a mut ModuleLookup,
	scope_stack: Vec<u32>,
}

impl<'a> LookupBuilder<'a> {
	fn current_scope(&self) -> u32 {
		*self.scope_stack.last().unwrap_or(&ModuleLookup::ROOT_SCOPE)
	}

	fn push_scope(&mut self, kind: ScopeKind) {
		let parent = Some(self.current_scope());
		let idx = self.lookup.push_scope(ScopeRange {
			start_line: 0,
			end_line: u32::MAX,
			parent,
			kind,
		});
		self.scope_stack.push(idx);
	}

	fn pop_scope(&mut self) {
		self.scope_stack.pop();
	}

	fn add_binding(&mut self, name: String, source: BindingSource, attributes: Vec<String>) {
		let scope_idx = self.current_scope();
		self.lookup.push_binding(IdentResolution {
			name,
			source,
			resolved_fqdn: None,
			aliases_to: None,
			mutability: None,
			scope_idx,
			attributes,
			ir_kind: None,
		});
	}

	fn hoist_items(&mut self, items: &[syn::Item]) {
		for item in items {
			match item {
				syn::Item::Fn(ItemFn { sig, .. }) => self.add_binding(
					sig.ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Function,
					},
					vec![],
				),
				syn::Item::Struct(ItemStruct { ident, .. }) => self.add_binding(
					ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Struct,
					},
					vec![],
				),
				syn::Item::Enum(ItemEnum { ident, .. }) => self.add_binding(
					ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Enum,
					},
					vec![],
				),
				syn::Item::Trait(ItemTrait { ident, .. }) => self.add_binding(
					ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Trait,
					},
					vec![],
				),
				syn::Item::Type(ItemType { ident, .. }) => self.add_binding(
					ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::TypeAlias,
					},
					vec![],
				),
				syn::Item::Const(ItemConst { ident, .. }) => self.add_binding(
					ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Const,
					},
					vec![],
				),
				syn::Item::Static(ItemStatic { ident, .. }) => self.add_binding(
					ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Const,
					},
					vec!["static".into()],
				),
				syn::Item::Union(ItemUnion { ident, .. }) => self.add_binding(
					ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Struct,
					},
					vec!["union".into()],
				),
				syn::Item::Mod(ItemMod { ident, .. }) => self.add_binding(
					ident.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Module,
					},
					vec![],
				),
				syn::Item::Macro(ItemMacro { ident: Some(name), .. }) => self.add_binding(
					name.to_string(),
					BindingSource::LocalDecl {
						decl_kind: LocalDeclKind::Macro,
					},
					vec![],
				),
				syn::Item::Use(ItemUse { tree, .. }) => {
					self.walk_use_tree(tree, &mut Vec::new());
				}
				// Impl blocks have no own ident binding (the impl is
				// attached to a target type that's already hoisted).
				syn::Item::Impl(_)
				| syn::Item::ForeignMod(_)
				| syn::Item::ExternCrate(_)
				| syn::Item::Verbatim(_)
				| syn::Item::Macro(_)
				| _ => {}
			}
		}
	}

	fn walk_use_tree(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
		match tree {
			UseTree::Path(p) => {
				prefix.push(p.ident.to_string());
				self.walk_use_tree(&p.tree, prefix);
				prefix.pop();
			}
			UseTree::Name(n) => {
				let local_name = n.ident.to_string();
				let module_path = prefix.join("::");
				self.record_import(local_name.clone(), module_path, Some(local_name), false);
			}
			UseTree::Rename(r) => {
				let local_name = r.rename.to_string();
				let original = r.ident.to_string();
				let module_path = prefix.join("::");
				self.record_import(local_name, module_path, Some(original), false);
			}
			UseTree::Group(g) => {
				for item in &g.items {
					self.walk_use_tree(item, prefix);
				}
			}
			UseTree::Glob(_) => {
				// Glob imports cannot enumerate locals — punt to Stage 3b
				// cross-workspace lookup (which can resolve via the
				// origin module's exported symbols).
				let module_path = prefix.join("::");
				self.lookup.push_import(ImportRecord {
					local_name: "*".into(),
					origin_module: module_path,
					origin_symbol: None,
					is_type_only: false,
					is_re_export: false,
				});
			}
		}
	}

	fn record_import(
		&mut self,
		local_name: String,
		module_path: String,
		original: Option<String>,
		is_re_export: bool,
	) {
		self.add_binding(
			local_name.clone(),
			BindingSource::Import {
				module_path: module_path.clone(),
				original_name: original.clone(),
				is_type_only: false,
				is_re_export,
			},
			vec![],
		);
		self.lookup.push_import(ImportRecord {
			local_name,
			origin_module: module_path,
			origin_symbol: original,
			is_type_only: false,
			is_re_export,
		});
	}

	fn bind_generic_params(&mut self, generics: &syn::Generics) {
		for param in &generics.params {
			match param {
				GenericParam::Type(t) => self.add_binding(
					t.ident.to_string(),
					BindingSource::TypeParam,
					vec![],
				),
				GenericParam::Const(c) => self.add_binding(
					c.ident.to_string(),
					BindingSource::TypeParam,
					vec!["const-generic".into()],
				),
				GenericParam::Lifetime(_) => {
					// Lifetimes never appear in value/type identifier
					// position — skip.
				}
			}
		}
	}

	fn bind_fn_params(&mut self, inputs: &syn::punctuated::Punctuated<FnArg, syn::Token![,]>) {
		for input in inputs {
			match input {
				FnArg::Typed(pt) => self.bind_pat(&pt.pat, BindingSource::Param, vec![]),
				FnArg::Receiver(_) => {
					// `self` / `&self` / `&mut self` — bound implicitly
					// via Self type, no binding needed.
				}
			}
		}
	}

	fn bind_pat(&mut self, pat: &Pat, source: BindingSource, extra_attrs: Vec<String>) {
		match pat {
			Pat::Ident(ident) => {
				self.add_binding(ident.ident.to_string(), source, extra_attrs);
			}
			Pat::Tuple(t) => {
				let mut attrs = extra_attrs.clone();
				attrs.push("unhandled-destructuring".into());
				for elem in &t.elems {
					self.bind_pat(elem, source.clone(), attrs.clone());
				}
			}
			Pat::TupleStruct(ts) => {
				let mut attrs = extra_attrs.clone();
				attrs.push("unhandled-destructuring".into());
				for elem in &ts.elems {
					self.bind_pat(elem, source.clone(), attrs.clone());
				}
			}
			Pat::Struct(s) => {
				let mut attrs = extra_attrs.clone();
				attrs.push("unhandled-destructuring".into());
				for field in &s.fields {
					self.bind_pat(&field.pat, source.clone(), attrs.clone());
				}
			}
			Pat::Reference(r) => self.bind_pat(&r.pat, source, extra_attrs),
			Pat::Type(t) => self.bind_pat(&t.pat, source, extra_attrs),
			Pat::Or(o) => {
				// Bind the first arm — all arms must bind the same set
				// of names so any one works.
				if let Some(first) = o.cases.first() {
					self.bind_pat(first, source, extra_attrs);
				}
			}
			_ => {}
		}
	}
}

impl<'ast> Visit<'ast> for LookupBuilder<'_> {
	fn visit_item_fn(&mut self, node: &'ast ItemFn) {
		self.push_scope(ScopeKind::Function);
		self.bind_generic_params(&node.sig.generics);
		self.bind_fn_params(&node.sig.inputs);
		syn::visit::visit_item_fn(self, node);
		self.pop_scope();
	}

	fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
		self.push_scope(ScopeKind::TypeContainer);
		self.bind_generic_params(&node.generics);
		syn::visit::visit_item_struct(self, node);
		self.pop_scope();
	}

	fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
		self.push_scope(ScopeKind::TypeContainer);
		self.bind_generic_params(&node.generics);
		for variant in &node.variants {
			self.add_binding(
				variant.ident.to_string(),
				BindingSource::LocalDecl {
					decl_kind: LocalDeclKind::Const,
				},
				vec!["enum-variant".into()],
			);
		}
		syn::visit::visit_item_enum(self, node);
		self.pop_scope();
	}

	fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
		self.push_scope(ScopeKind::TypeContainer);
		self.bind_generic_params(&node.generics);
		syn::visit::visit_item_trait(self, node);
		self.pop_scope();
	}

	fn visit_item_type(&mut self, node: &'ast ItemType) {
		self.push_scope(ScopeKind::TypeContainer);
		self.bind_generic_params(&node.generics);
		syn::visit::visit_item_type(self, node);
		self.pop_scope();
	}

	fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
		self.push_scope(ScopeKind::TypeContainer);
		self.bind_generic_params(&node.generics);
		syn::visit::visit_item_impl(self, node);
		self.pop_scope();
	}

	fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
		self.push_scope(ScopeKind::Function);
		self.bind_generic_params(&node.sig.generics);
		self.bind_fn_params(&node.sig.inputs);
		syn::visit::visit_impl_item_fn(self, node);
		self.pop_scope();
	}

	fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
		self.push_scope(ScopeKind::Function);
		self.bind_generic_params(&node.sig.generics);
		self.bind_fn_params(&node.sig.inputs);
		syn::visit::visit_trait_item_fn(self, node);
		self.pop_scope();
	}

	fn visit_item_mod(&mut self, node: &'ast ItemMod) {
		self.push_scope(ScopeKind::Module);
		if let Some((_, items)) = &node.content {
			self.hoist_items(items);
		}
		syn::visit::visit_item_mod(self, node);
		self.pop_scope();
	}

	fn visit_block(&mut self, node: &'ast syn::Block) {
		self.push_scope(ScopeKind::Block);
		syn::visit::visit_block(self, node);
		self.pop_scope();
	}

	fn visit_local(&mut self, node: &'ast Local) {
		self.bind_pat(
			&node.pat,
			BindingSource::LocalDecl {
				decl_kind: LocalDeclKind::Let,
			},
			vec![],
		);
		syn::visit::visit_local(self, node);
	}

	fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
		self.push_scope(ScopeKind::Function);
		for input in &node.inputs {
			self.bind_pat(input, BindingSource::Param, vec![]);
		}
		syn::visit::visit_expr_closure(self, node);
		self.pop_scope();
	}
}

// `ImplItem` is unused after the visitor split — re-export to keep
// the syn surface tidy in case Stage 4 expands here.
#[allow(dead_code)]
const _: Option<&ImplItem> = None;

#[cfg(test)]
mod tests {
	use super::*;

	fn parse(src: &str) -> File {
		syn::parse_file(src).expect("parse ok")
	}

	#[test]
	fn module_lookup_carries_module_fqdn_and_language() {
		let f = parse("fn f() {}\n");
		let lookup = build_rust_lookup(&f, "my_crate::module");
		assert_eq!(lookup.module_fqdn, "my_crate::module");
		assert_eq!(lookup.language, Language::Rust);
	}

	#[test]
	fn top_level_items_hoisted_to_root() {
		let f = parse(
			"fn f() {}\nstruct S;\nenum E { A }\ntrait T {}\ntype Ty = u32;\nconst C: u32 = 1;\nstatic ST: u32 = 0;\n",
		);
		let lookup = build_rust_lookup(&f, "m");
		for name in ["f", "S", "E", "T", "Ty", "C", "ST"] {
			let b = lookup
				.bindings
				.get(name)
				.and_then(|v| v.first())
				.unwrap_or_else(|| panic!("{name} binding"));
			assert_eq!(b.scope_idx, ModuleLookup::ROOT_SCOPE, "{name} at root");
		}
	}

	#[test]
	fn use_simple_binds_last_segment() {
		let f = parse("use std::collections::HashMap;\n");
		let lookup = build_rust_lookup(&f, "m");
		let b = lookup
			.bindings
			.get("HashMap")
			.and_then(|v| v.first())
			.expect("HashMap binding");
		match &b.source {
			BindingSource::Import {
				module_path,
				original_name,
				..
			} => {
				assert_eq!(module_path, "std::collections");
				assert_eq!(original_name.as_deref(), Some("HashMap"));
			}
			other => panic!("expected Import, got {other:?}"),
		}
		assert_eq!(lookup.imports.len(), 1);
	}

	#[test]
	fn use_rename_binds_alias_with_original() {
		let f = parse("use std::collections::HashMap as Map;\n");
		let lookup = build_rust_lookup(&f, "m");
		assert!(!lookup.bindings.contains_key("HashMap"));
		let b = lookup
			.bindings
			.get("Map")
			.and_then(|v| v.first())
			.expect("Map binding");
		match &b.source {
			BindingSource::Import {
				module_path,
				original_name,
				..
			} => {
				assert_eq!(module_path, "std::collections");
				assert_eq!(original_name.as_deref(), Some("HashMap"));
			}
			other => panic!("expected Import, got {other:?}"),
		}
	}

	#[test]
	fn use_group_binds_each_member() {
		let f = parse("use std::collections::{HashMap, HashSet, BTreeMap};\n");
		let lookup = build_rust_lookup(&f, "m");
		for name in ["HashMap", "HashSet", "BTreeMap"] {
			let b = lookup
				.bindings
				.get(name)
				.and_then(|v| v.first())
				.unwrap_or_else(|| panic!("{name} binding"));
			match &b.source {
				BindingSource::Import { module_path, .. } => {
					assert_eq!(module_path, "std::collections");
				}
				other => panic!("expected Import for {name}, got {other:?}"),
			}
		}
		assert_eq!(lookup.imports.len(), 3);
	}

	#[test]
	fn use_glob_records_star_import() {
		let f = parse("use std::collections::*;\n");
		let lookup = build_rust_lookup(&f, "m");
		assert_eq!(lookup.imports.len(), 1);
		assert_eq!(lookup.imports[0].local_name, "*");
		assert_eq!(lookup.imports[0].origin_module, "std::collections");
	}

	#[test]
	fn type_param_bound_in_function_scope() {
		let f = parse("fn f<T, U: Clone>(x: T) -> U { todo!() }\n");
		let lookup = build_rust_lookup(&f, "m");
		for name in ["T", "U"] {
			let b = lookup
				.bindings
				.get(name)
				.and_then(|v| v.first())
				.unwrap_or_else(|| panic!("{name} type-param binding"));
			assert!(matches!(b.source, BindingSource::TypeParam));
			assert_ne!(b.scope_idx, ModuleLookup::ROOT_SCOPE);
		}
	}

	#[test]
	fn fn_body_let_binding_scoped_below_root() {
		let f = parse("fn f() { let inner = 42; }\n");
		let lookup = build_rust_lookup(&f, "m");
		let inner = lookup
			.bindings
			.get("inner")
			.and_then(|v| v.first())
			.expect("inner binding");
		assert_ne!(inner.scope_idx, ModuleLookup::ROOT_SCOPE);
	}

	#[test]
	fn enum_variants_bound_inside_enum_scope() {
		let f = parse("enum Color { Red, Green, Blue }\n");
		let lookup = build_rust_lookup(&f, "m");
		assert!(lookup.bindings.contains_key("Color"));
		for v in ["Red", "Green", "Blue"] {
			let b = lookup
				.bindings
				.get(v)
				.and_then(|v| v.first())
				.unwrap_or_else(|| panic!("{v} variant binding"));
			assert_ne!(b.scope_idx, ModuleLookup::ROOT_SCOPE);
			assert!(b.attributes.iter().any(|a| a == "enum-variant"));
		}
	}

	#[test]
	fn impl_block_generics_bind_in_impl_scope() {
		let f = parse("impl<T: Clone> MyType<T> { fn method(&self) -> T { todo!() } }\n");
		let lookup = build_rust_lookup(&f, "m");
		let t = lookup
			.bindings
			.get("T")
			.and_then(|v| v.first())
			.expect("T binding");
		assert!(matches!(t.source, BindingSource::TypeParam));
	}

	#[test]
	fn resolve_local_walks_chain_to_root_use() {
		let f = parse("use std::vec::Vec;\nfn f() { let v: Vec<u32> = Vec::new(); }\n");
		let lookup = build_rust_lookup(&f, "m");
		let v_scope = lookup
			.bindings
			.get("v")
			.and_then(|v| v.first())
			.unwrap()
			.scope_idx;
		let vec_t = lookup
			.resolve_local("Vec", v_scope)
			.expect("Vec reachable via parent");
		assert!(matches!(vec_t.source, BindingSource::Import { .. }));
		assert_eq!(vec_t.scope_idx, ModuleLookup::ROOT_SCOPE);
	}
}
