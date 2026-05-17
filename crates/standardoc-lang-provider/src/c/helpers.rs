use std::path::Path;

use standardoc_ir::SymbolLocation;
use tree_sitter::Node;

use crate::utils::strip_extension;

/// Compute the `::`-joined module portion of an FQDN from a workspace-relative
/// path (relative to the package root). Strips `.c` / `.h` extensions.
///
/// Examples (package_relative input → output):
/// * `"runtime/vm.c"` → `"runtime::vm"`
/// * `"include/lur.h"` → `"include::lur"`
/// * `"main.c"` → `"main"`
pub(crate) fn compute_module_path(package_relative: &str) -> String {
    let stem = strip_extension(package_relative, &[".c", ".h"]);
    stem.replace('/', "::").replace('\\', "::")
}

/// Workspace-directory-name fallback when no `standarbuild-detect` project
/// label is provided. Mirrors the Lua provider's fallback.
pub(crate) fn workspace_dir_name(workspace_root: &Path) -> String {
    workspace_root
        .file_name()
        .map_or_else(|| "workspace".into(), |s| s.to_string_lossy().into_owned())
}

/// Build a `SymbolLocation` from a tree-sitter `Node`. Lines 1-indexed,
/// columns 0-indexed — matches the convention used by every other provider.
pub(crate) fn location_from_node(file: &str, node: Node) -> SymbolLocation {
    let start = node.start_position();
    let end = node.end_position();
    SymbolLocation {
        file: file.into(),
        start_line: u32::try_from(start.row + 1).unwrap_or(u32::MAX),
        end_line: u32::try_from(end.row + 1).unwrap_or(u32::MAX),
        start_col: u32::try_from(start.column).unwrap_or(u32::MAX),
        end_col: u32::try_from(end.column).unwrap_or(u32::MAX),
    }
}

/// Source text covered by `node`.
pub(crate) fn node_text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}

/// Extract the bound identifier name from a C `declarator` subtree.
///
/// C declarators nest unpredictably:
/// * plain → `identifier`
/// * pointer → `pointer_declarator { ..., declarator: <inner> }`
/// * function → `function_declarator { declarator: <inner>, ... }`
/// * array → `array_declarator { declarator: <inner>, ... }`
/// * function pointer → `parenthesized_declarator { (pointer_declarator { declarator: identifier }) }`
///
/// We walk down whichever child is named `declarator` (or unwrap parens)
/// until we hit an `identifier` / `type_identifier` / `field_identifier`.
/// Returns `None` for declarators we don't yet handle.
pub(crate) fn declarator_name<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    let mut current = node;
    loop {
        match current.kind() {
            "identifier" | "type_identifier" | "field_identifier" => {
                return Some(node_text(current, src));
            }
            "parenthesized_declarator" => {
                // Drop into the parenthesised inner declarator.
                current = first_named_child(current)?;
            }
            "pointer_declarator"
            | "function_declarator"
            | "array_declarator"
            | "reference_declarator"
            | "abstract_function_declarator"
            | "abstract_pointer_declarator" => {
                current = current.child_by_field_name("declarator")?;
            }
            _ => return None,
        }
    }
}

/// Returns `true` when `node` is a `declaration` whose innermost declarator
/// is a `function_declarator` (i.e. a function prototype, no body).
pub(crate) fn declaration_is_function_prototype(node: Node) -> bool {
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return false;
    };
    contains_function_declarator(declarator)
}

fn contains_function_declarator(node: Node) -> bool {
    match node.kind() {
        "function_declarator" => true,
        "pointer_declarator"
        | "array_declarator"
        | "parenthesized_declarator"
        | "reference_declarator" => node
            .child_by_field_name("declarator")
            .is_some_and(contains_function_declarator),
        _ => false,
    }
}

/// First named child of `node`, or `None` if it has none.
pub(crate) fn first_named_child(node: Node) -> Option<Node> {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let n = cursor.node();
            if n.is_named() {
                return Some(n);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

/// Visibility hint derived from storage-class specifiers attached to a
/// declaration / function_definition. `static` → private; otherwise public.
pub(crate) fn storage_class_is_static(node: Node, src: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "storage_class_specifier" && node_text(child, src).trim() == "static" {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_module_path_strips_c_extension() {
        assert_eq!(compute_module_path("runtime/vm.c"), "runtime::vm");
        assert_eq!(compute_module_path("main.c"), "main");
    }

    #[test]
    fn compute_module_path_strips_h_extension() {
        assert_eq!(compute_module_path("include/lur.h"), "include::lur");
    }

    #[test]
    fn compute_module_path_normalises_backslashes() {
        assert_eq!(compute_module_path("runtime\\vm.c"), "runtime::vm");
    }

    #[test]
    fn workspace_dir_name_extracts_last_component() {
        assert_eq!(workspace_dir_name(Path::new("/tmp/myproj")), "myproj");
    }
}
