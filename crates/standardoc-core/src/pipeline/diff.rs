use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use standardoc_ir::RawSymbol;

use crate::storage::error::StorageError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DbSymbolRow {
    pub id: i64,
    pub fqdn: String,
    pub body_hash: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct DiffPlan<'a> {
    pub inserts: Vec<&'a RawSymbol>,
    pub modifications: Vec<(i64, &'a RawSymbol)>,
    pub position_updates: Vec<(i64, &'a RawSymbol)>,
    pub deletions: Vec<i64>,
}

pub(crate) fn fetch_existing_symbols(
    conn: &Connection,
    file_path: &str,
) -> Result<Vec<DbSymbolRow>, StorageError> {
    let mut stmt = conn.prepare("SELECT id, fqdn, body_hash FROM symbols WHERE file_path = ?1")?;
    let rows = stmt
        .query_map([file_path], |r| {
            Ok(DbSymbolRow {
                id: r.get(0)?,
                fqdn: r.get(1)?,
                body_hash: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(crate) fn diff_symbols<'a>(
    existing: &[DbSymbolRow],
    extracted: &'a [RawSymbol],
) -> DiffPlan<'a> {
    let mut by_fqdn: HashMap<&str, &DbSymbolRow> = HashMap::with_capacity(existing.len());
    for row in existing {
        by_fqdn.insert(row.fqdn.as_str(), row);
    }

    let mut plan = DiffPlan::default();
    let mut seen_fqdns: HashSet<&str> = HashSet::with_capacity(extracted.len());

    for sym in extracted {
        seen_fqdns.insert(sym.fqdn.as_str());
        let Some(row) = by_fqdn.get(sym.fqdn.as_str()) else {
            plan.inserts.push(sym);
            continue;
        };
        let new_hash = sym
            .body_hash
            .as_ref()
            .map(standardoc_ir::Blake3Hash::to_hex);
        if new_hash == row.body_hash {
            plan.position_updates.push((row.id, sym));
        } else {
            plan.modifications.push((row.id, sym));
        }
    }

    for row in existing {
        if !seen_fqdns.contains(row.fqdn.as_str()) {
            plan.deletions.push(row.id);
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::symbols::insert_symbol;
    use crate::storage::test_utils::{fresh_conn, sample_symbol, seed_file, symbol_ctx};
    use standardoc_ir::Blake3Hash;

    fn rs(fqdn: &str, hash_byte: u8) -> RawSymbol {
        let mut s = sample_symbol(fqdn.rsplit("::").next().unwrap_or(fqdn), fqdn);
        s.body_hash = Some(Blake3Hash::new([hash_byte; 32]));
        s
    }

    #[test]
    fn diff_all_new_when_existing_empty() {
        let extracted = vec![rs("crate::a", 0x01), rs("crate::b", 0x02)];
        let plan = diff_symbols(&[], &extracted);
        assert_eq!(plan.inserts.len(), 2);
        assert!(plan.modifications.is_empty());
        assert!(plan.position_updates.is_empty());
        assert!(plan.deletions.is_empty());
    }

    #[test]
    fn diff_all_unchanged_when_hashes_match() {
        let existing = vec![DbSymbolRow {
            id: 7,
            fqdn: "crate::a".into(),
            body_hash: Some(Blake3Hash::new([0x01; 32]).to_hex()),
        }];
        let extracted = vec![rs("crate::a", 0x01)];
        let plan = diff_symbols(&existing, &extracted);
        assert!(plan.inserts.is_empty());
        assert!(plan.modifications.is_empty());
        assert_eq!(plan.position_updates.len(), 1);
        assert_eq!(plan.position_updates[0].0, 7);
        assert!(plan.deletions.is_empty());
    }

    #[test]
    fn diff_modified_when_body_hash_differs() {
        let existing = vec![DbSymbolRow {
            id: 7,
            fqdn: "crate::a".into(),
            body_hash: Some(Blake3Hash::new([0x01; 32]).to_hex()),
        }];
        let extracted = vec![rs("crate::a", 0x99)];
        let plan = diff_symbols(&existing, &extracted);
        assert_eq!(plan.modifications.len(), 1);
        assert_eq!(plan.modifications[0].0, 7);
        assert!(plan.position_updates.is_empty());
    }

    #[test]
    fn diff_modified_when_db_hash_null_but_provider_set() {
        let existing = vec![DbSymbolRow {
            id: 7,
            fqdn: "crate::a".into(),
            body_hash: None,
        }];
        let extracted = vec![rs("crate::a", 0x01)];
        let plan = diff_symbols(&existing, &extracted);
        assert_eq!(plan.modifications.len(), 1);
    }

    #[test]
    fn diff_disappeared_when_extracted_misses_existing_fqdn() {
        let existing = vec![
            DbSymbolRow {
                id: 7,
                fqdn: "crate::a".into(),
                body_hash: Some(Blake3Hash::new([0x01; 32]).to_hex()),
            },
            DbSymbolRow {
                id: 8,
                fqdn: "crate::b".into(),
                body_hash: Some(Blake3Hash::new([0x02; 32]).to_hex()),
            },
        ];
        let extracted = vec![rs("crate::a", 0x01)];
        let plan = diff_symbols(&existing, &extracted);
        assert_eq!(plan.deletions, vec![8]);
        assert_eq!(plan.position_updates.len(), 1);
    }

    #[test]
    fn diff_mixed_buckets() {
        let existing = vec![
            DbSymbolRow {
                id: 1,
                fqdn: "crate::keep".into(),
                body_hash: Some(Blake3Hash::new([0x01; 32]).to_hex()),
            },
            DbSymbolRow {
                id: 2,
                fqdn: "crate::edit".into(),
                body_hash: Some(Blake3Hash::new([0x02; 32]).to_hex()),
            },
            DbSymbolRow {
                id: 3,
                fqdn: "crate::gone".into(),
                body_hash: Some(Blake3Hash::new([0x03; 32]).to_hex()),
            },
        ];
        let extracted = vec![
            rs("crate::keep", 0x01),
            rs("crate::edit", 0xee),
            rs("crate::brand_new", 0x42),
        ];
        let plan = diff_symbols(&existing, &extracted);
        assert_eq!(plan.inserts.len(), 1);
        assert_eq!(plan.inserts[0].fqdn, "crate::brand_new");
        assert_eq!(plan.modifications.len(), 1);
        assert_eq!(plan.modifications[0].0, 2);
        assert_eq!(plan.position_updates.len(), 1);
        assert_eq!(plan.position_updates[0].0, 1);
        assert_eq!(plan.deletions, vec![3]);
    }

    #[test]
    fn fetch_existing_symbols_filters_by_file_path() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        seed_file(&conn, "src/other.rs");
        insert_symbol(
            &conn,
            &sample_symbol("foo", "crate::foo"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        insert_symbol(
            &conn,
            &sample_symbol("bar", "crate::bar"),
            symbol_ctx("src/other.rs"),
        )
        .unwrap();

        let rows = fetch_existing_symbols(&conn, "src/main.rs").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].fqdn, "crate::foo");
    }

    #[test]
    fn fetch_existing_symbols_empty_for_unknown_file() {
        let conn = fresh_conn();
        let rows = fetch_existing_symbols(&conn, "nope.rs").unwrap();
        assert!(rows.is_empty());
    }
}
