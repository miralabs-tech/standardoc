use standardoc_ir::{
    DeclKind, EdgeConfidence, EdgeKind, EntryPointKind, Kind, Language, Signature, SourceOrigin,
    Visibility,
};

use crate::storage::error::StorageError;

pub(crate) fn signature_to_json(sig: &Signature) -> Result<String, StorageError> {
    // IR-1 1.0 vocabulary lock: validate every `exposed_via` slug before
    // persisting the signature JSON. Refuses extractor-emitted slugs
    // that are neither built-in nor `custom:`-prefixed, surfacing the
    // error as `StorageError::BridgeKindInvalid` (via
    // `From<BridgeKindError>`).
    //
    // IR-3: `exposed_via` is `Vec<BridgeKind>` (dual-target apps may
    // expose the same symbol via multiple bridges, e.g. Tauri + wasm).
    // Validation short-circuits on the first invalid slug — a second
    // bad slug in the same vec will surface on the next walk after the
    // first is fixed, which is the simplest contract.
    for bridge in &sig.meta.exposed_via {
        bridge.try_validate()?;
    }
    Ok(serde_json::to_string(sig)?)
}

pub(crate) fn json_to_signature(s: &str) -> Result<Signature, StorageError> {
    Ok(serde_json::from_str(s)?)
}

pub(crate) const fn language_to_sql_text(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
        Language::JavaScript => "javascript",
        Language::Lua => "lua",
        Language::Vue => "vue",
        Language::Svelte => "svelte",
        Language::C => "c",
    }
}

pub(crate) fn language_from_sql_text(s: &str) -> Result<Language, StorageError> {
    match s {
        "rust" => Ok(Language::Rust),
        "typescript" => Ok(Language::TypeScript),
        "javascript" => Ok(Language::JavaScript),
        "lua" => Ok(Language::Lua),
        "vue" => Ok(Language::Vue),
        "svelte" => Ok(Language::Svelte),
        "c" => Ok(Language::C),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown language: {other:?}"),
        }),
    }
}

pub(crate) const fn kind_to_sql_text(k: Kind) -> &'static str {
    match k {
        Kind::Callable => "callable",
        Kind::Type => "type",
        Kind::Value => "value",
        Kind::Module => "module",
        Kind::Macro => "macro",
    }
}

pub(crate) fn kind_from_sql_text(s: &str) -> Result<Kind, StorageError> {
    match s {
        "callable" => Ok(Kind::Callable),
        "type" => Ok(Kind::Type),
        "value" => Ok(Kind::Value),
        "module" => Ok(Kind::Module),
        "macro" => Ok(Kind::Macro),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown kind: {other:?}"),
        }),
    }
}

/// Encodes a [`DeclKind`] as a flat SQL text. Built-in variants map
/// to their `serde(rename_all = "snake_case")` representation;
/// `Custom { lang, tag }` becomes `"custom:<lang>:<tag>"` (lang slug
/// mirrors [`language_to_sql_text`]). The flat shape is grep-friendly
/// on the SQL side — no JSON braces in the column.
pub(crate) fn decl_kind_to_sql_text(d: &DeclKind) -> String {
    match d {
        DeclKind::Module => "module".into(),
        DeclKind::Namespace => "namespace".into(),
        DeclKind::Crate => "crate".into(),
        DeclKind::Struct => "struct".into(),
        DeclKind::Enum => "enum".into(),
        DeclKind::Union => "union".into(),
        DeclKind::Class => "class".into(),
        DeclKind::Interface => "interface".into(),
        DeclKind::TypeAlias => "type_alias".into(),
        DeclKind::Function => "function".into(),
        DeclKind::Method => "method".into(),
        DeclKind::Constructor => "constructor".into(),
        DeclKind::Getter => "getter".into(),
        DeclKind::Setter => "setter".into(),
        DeclKind::Const => "const".into(),
        DeclKind::Static => "static".into(),
        DeclKind::Var => "var".into(),
        DeclKind::Field => "field".into(),
        DeclKind::EnumVariant => "enum_variant".into(),
        DeclKind::DeclarativeMacro => "declarative_macro".into(),
        DeclKind::ProcMacro => "proc_macro".into(),
        DeclKind::Decorator => "decorator".into(),
        DeclKind::Custom { lang, tag } => {
            format!("custom:{}:{}", language_to_sql_text(*lang), tag)
        }
    }
}

pub(crate) fn decl_kind_from_sql_text(s: &str) -> Result<DeclKind, StorageError> {
    match s {
        "module" => Ok(DeclKind::Module),
        "namespace" => Ok(DeclKind::Namespace),
        "crate" => Ok(DeclKind::Crate),
        "struct" => Ok(DeclKind::Struct),
        "enum" => Ok(DeclKind::Enum),
        "union" => Ok(DeclKind::Union),
        "class" => Ok(DeclKind::Class),
        "interface" => Ok(DeclKind::Interface),
        "type_alias" => Ok(DeclKind::TypeAlias),
        "function" => Ok(DeclKind::Function),
        "method" => Ok(DeclKind::Method),
        "constructor" => Ok(DeclKind::Constructor),
        "getter" => Ok(DeclKind::Getter),
        "setter" => Ok(DeclKind::Setter),
        "const" => Ok(DeclKind::Const),
        "static" => Ok(DeclKind::Static),
        "var" => Ok(DeclKind::Var),
        "field" => Ok(DeclKind::Field),
        "enum_variant" => Ok(DeclKind::EnumVariant),
        "declarative_macro" => Ok(DeclKind::DeclarativeMacro),
        "proc_macro" => Ok(DeclKind::ProcMacro),
        "decorator" => Ok(DeclKind::Decorator),
        other => match other.strip_prefix("custom:") {
            Some(rest) => {
                let (lang_s, tag) =
                    rest.split_once(':')
                        .ok_or_else(|| StorageError::InvalidStoredData {
                            detail: format!("custom decl_kind missing tag: {other:?}"),
                        })?;
                let lang = language_from_sql_text(lang_s)?;
                Ok(DeclKind::Custom {
                    lang,
                    tag: tag.to_string(),
                })
            }
            None => Err(StorageError::InvalidStoredData {
                detail: format!("unknown decl_kind: {other:?}"),
            }),
        },
    }
}

/// Phase 3 (Flow) — Encodes an [`EntryPointKind`] as flat SQL text
/// (`binary_main` / `public_api` / `ffi_export`). Matches the SQL
/// CHECK in `init_v0.sql`.
pub(crate) const fn entry_point_to_sql_text(e: EntryPointKind) -> &'static str {
    match e {
        EntryPointKind::BinaryMain => "binary_main",
        EntryPointKind::PublicApi => "public_api",
        EntryPointKind::FfiExport => "ffi_export",
    }
}

pub(crate) fn entry_point_from_sql_text(s: &str) -> Result<EntryPointKind, StorageError> {
    match s {
        "binary_main" => Ok(EntryPointKind::BinaryMain),
        "public_api" => Ok(EntryPointKind::PublicApi),
        "ffi_export" => Ok(EntryPointKind::FfiExport),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown entry_point: {other:?}"),
        }),
    }
}

pub(crate) const fn visibility_to_sql_text(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
        Visibility::Crate => "crate",
        Visibility::Protected => "protected",
    }
}

pub(crate) fn visibility_from_sql_text(s: &str) -> Result<Visibility, StorageError> {
    match s {
        "public" => Ok(Visibility::Public),
        "private" => Ok(Visibility::Private),
        "crate" => Ok(Visibility::Crate),
        "protected" => Ok(Visibility::Protected),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown visibility: {other:?}"),
        }),
    }
}

pub(crate) const fn source_origin_to_sql_text(o: SourceOrigin) -> &'static str {
    match o {
        SourceOrigin::Workspace => "workspace",
        SourceOrigin::CargoRegistry => "cargo_registry",
        SourceOrigin::NodeModulesDts => "node_modules_dts",
        SourceOrigin::ManualExternal => "manual_external",
    }
}

pub(crate) fn source_origin_from_sql_text(s: &str) -> Result<SourceOrigin, StorageError> {
    match s {
        "workspace" => Ok(SourceOrigin::Workspace),
        "cargo_registry" => Ok(SourceOrigin::CargoRegistry),
        "node_modules_dts" => Ok(SourceOrigin::NodeModulesDts),
        "manual_external" => Ok(SourceOrigin::ManualExternal),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown source_origin: {other:?}"),
        }),
    }
}

pub(crate) const fn edge_kind_to_sql_text(k: EdgeKind) -> &'static str {
    match k {
        EdgeKind::Calls => "CALLS",
        EdgeKind::Imports => "IMPORTS",
        EdgeKind::Extends => "EXTENDS",
        EdgeKind::Implements => "IMPLEMENTS",
        EdgeKind::References => "REFERENCES",
        EdgeKind::UsesType => "USES_TYPE",
    }
}

pub(crate) fn edge_kind_from_sql_text(s: &str) -> Result<EdgeKind, StorageError> {
    match s {
        "CALLS" => Ok(EdgeKind::Calls),
        "IMPORTS" => Ok(EdgeKind::Imports),
        "EXTENDS" => Ok(EdgeKind::Extends),
        "IMPLEMENTS" => Ok(EdgeKind::Implements),
        "REFERENCES" => Ok(EdgeKind::References),
        "USES_TYPE" => Ok(EdgeKind::UsesType),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown edge kind: {other:?}"),
        }),
    }
}

pub(crate) const fn edge_confidence_to_sql_text(c: EdgeConfidence) -> &'static str {
    match c {
        EdgeConfidence::Extracted => "extracted",
        EdgeConfidence::Inferred => "inferred",
        EdgeConfidence::Ambiguous => "ambiguous",
    }
}

pub(crate) fn edge_confidence_from_sql_text(s: &str) -> Result<EdgeConfidence, StorageError> {
    match s {
        "extracted" => Ok(EdgeConfidence::Extracted),
        "inferred" => Ok(EdgeConfidence::Inferred),
        "ambiguous" => Ok(EdgeConfidence::Ambiguous),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown edge confidence: {other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests;
