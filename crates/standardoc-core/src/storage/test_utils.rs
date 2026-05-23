use rusqlite::Connection;
use standardoc_ir::{
    Blake3Hash, Kind, Language, LanguageKind, RawSymbol, SourceOrigin, SymbolLocation, Visibility,
};

use crate::storage::files::{FileInput, upsert_file};
use crate::storage::migrate::ensure_schema;
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;
use crate::storage::symbols::SymbolInsertContext;

pub(crate) fn fresh_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable FK pragma");
    ensure_schema(&conn).expect("ensure schema (init + upgrades)");
    conn
}

pub(crate) fn seed_file(conn: &Connection, path: &str) {
    upsert_file(
        conn,
        &FileInput {
            path: path.into(),
            content_hash: Blake3Hash::default(),
            language: Language::Rust,
            byte_size: 100,
            last_scanned: 0,
            last_scan_error: None,
            is_external: false,
        },
    )
    .expect("seed file row");
}

pub(crate) fn sample_symbol(name: &str, fqdn: &str) -> RawSymbol {
    RawSymbol {
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        name: name.into(),
        fqdn: fqdn.into(),
        kind: Kind::Function,
        language_kind: LanguageKind::from("fn_item"),
        module: None,
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: "src/main.rs".into(),
            start_line: 1,
            end_line: 5,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: Some(Blake3Hash::new([0xab; 32])),
        attributes: vec![],
        flags: vec![],
    }
}

pub(crate) fn symbol_ctx(file_path: &str) -> SymbolInsertContext<'_> {
    SymbolInsertContext {
        file_path,
        language: Language::Rust,
        is_external: false,
        source_origin: SourceOrigin::Workspace,
        revision: 0,
        workspace_id: PRIMARY_WORKSPACE_ID,
    }
}
