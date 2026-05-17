-- Stage 3b-7-b Layer 3a: relax the `symbols.fqdn UNIQUE` constraint to a
-- composite `UNIQUE(workspace_id, fqdn)` so peer-extracted rows can
-- coexist with primary rows that happen to share an FQDN (the common
-- case once Layer 3b's autonomous indexer starts walking peer source).
--
-- SQLite has no in-place way to drop a column-level UNIQUE constraint,
-- so this is the canonical 12-step "table rebuild" pattern documented at
-- https://www.sqlite.org/lang_altertable.html#otheralter:
--
--   1. PRAGMA foreign_keys=OFF   (outside the txn — pragma is a no-op
--                                 inside a transaction)
--   2. BEGIN
--   3. CREATE TABLE new_symbols  with the relaxed UNIQUE
--   4. INSERT … SELECT preserving every id (FK refs depend on this:
--      edges.from_symbol_id, edges.to_symbol_id, documents.symbol_id,
--      enrichments.symbol_id, enrichment_rejections.symbol_id all
--      point at symbols.id and must stay valid through the swap)
--   5. DROP old   (cascades indexes + triggers automatically; we
--                  recreate them explicitly post-rename)
--   6. RENAME new_symbols → symbols
--   7. Recreate indexes + FTS triggers
--   8. Fix up sqlite_sequence so AUTOINCREMENT keeps growing from
--      MAX(id) instead of restarting at the post-rename name's count
--   9. UPDATE schema_meta
--   10. COMMIT
--   11. PRAGMA foreign_keys=ON
--
-- The migration runner (`apply_upgrade`) wraps each migration in its
-- own `execute_batch` — PRAGMAs and BEGIN/COMMIT are honored as-is.
--
-- Notes:
-- - The post-v10 `idx_symbols_workspace_id_fqdn` explicit index is NOT
--   recreated: the new `UNIQUE(workspace_id, fqdn)` constraint auto-
--   creates an `sqlite_autoindex_symbols_*` covering exactly those
--   columns, which the query planner uses transparently. Keeping both
--   would be redundant storage.
-- - `symbols_fts` (the FTS5 virtual table with `content='symbols'`)
--   does NOT need a rebuild: it stores its content-table reference by
--   string name (still 'symbols' post-rename) and by rowid (preserved
--   because we copy `id` explicitly). The three triggers that fan
--   inserts/deletes/updates into `symbols_fts` DO need recreation
--   because they were attached to the dropped table.

PRAGMA foreign_keys=OFF;

BEGIN TRANSACTION;

CREATE TABLE new_symbols (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  fqdn            TEXT    NOT NULL,
  name            TEXT    NOT NULL,
  kind            TEXT    NOT NULL CHECK (kind IN
                    ('function', 'type', 'value', 'module', 'macro')),
  language_kind   TEXT    NOT NULL,
  language        TEXT    NOT NULL,
  module          TEXT,
  visibility      TEXT    NOT NULL DEFAULT 'public' CHECK (visibility IN
                    ('public', 'private', 'crate', 'protected')),
  file_path       TEXT    NOT NULL REFERENCES files(path) ON DELETE CASCADE,
  start_line      INTEGER NOT NULL,
  end_line        INTEGER NOT NULL,
  start_col       INTEGER NOT NULL,
  end_col         INTEGER NOT NULL,
  signature_json  TEXT,
  body_hash       TEXT,
  is_external     INTEGER NOT NULL DEFAULT 0 CHECK (is_external IN (0, 1)),
  source_origin   TEXT    NOT NULL DEFAULT 'workspace' CHECK (source_origin IN
                    ('workspace', 'cargo_registry', 'node_modules_dts',
                     'manual_external')),
  last_modified_revision INTEGER NOT NULL DEFAULT 0,
  flags                  TEXT    NOT NULL DEFAULT '[]',
  workspace_id           TEXT    NOT NULL DEFAULT 'primary',
  UNIQUE (workspace_id, fqdn)
);

INSERT INTO new_symbols (
  id, fqdn, name, kind, language_kind, language, module, visibility,
  file_path, start_line, end_line, start_col, end_col,
  signature_json, body_hash, is_external, source_origin,
  last_modified_revision, flags, workspace_id
)
SELECT
  id, fqdn, name, kind, language_kind, language, module, visibility,
  file_path, start_line, end_line, start_col, end_col,
  signature_json, body_hash, is_external, source_origin,
  last_modified_revision, flags, workspace_id
FROM symbols;

DROP TABLE symbols;

ALTER TABLE new_symbols RENAME TO symbols;

CREATE INDEX idx_symbols_language               ON symbols(language);
CREATE INDEX idx_symbols_kind                   ON symbols(kind);
CREATE INDEX idx_symbols_name                   ON symbols(name);
CREATE INDEX idx_symbols_file_path              ON symbols(file_path);
CREATE INDEX idx_symbols_module                 ON symbols(module) WHERE module IS NOT NULL;
CREATE INDEX idx_symbols_is_external            ON symbols(is_external);
CREATE INDEX idx_symbols_last_modified_revision ON symbols(last_modified_revision);

CREATE TRIGGER symbols_fts_insert AFTER INSERT ON symbols BEGIN
  INSERT INTO symbols_fts(rowid, name, fqdn)
  VALUES (new.id, new.name, new.fqdn);
END;

CREATE TRIGGER symbols_fts_delete AFTER DELETE ON symbols BEGIN
  INSERT INTO symbols_fts(symbols_fts, rowid, name, fqdn)
  VALUES ('delete', old.id, old.name, old.fqdn);
END;

CREATE TRIGGER symbols_fts_update AFTER UPDATE OF name, fqdn ON symbols BEGIN
  INSERT INTO symbols_fts(symbols_fts, rowid, name, fqdn)
  VALUES ('delete', old.id, old.name, old.fqdn);
  INSERT INTO symbols_fts(rowid, name, fqdn)
  VALUES (new.id, new.name, new.fqdn);
END;

-- AUTOINCREMENT bookkeeping. SQLite tracks the next rowid in the
-- `sqlite_sequence` table keyed by table name. After the DROP+RENAME,
-- the pre-migration `symbols` entry is gone and a `new_symbols` entry
-- was created when we explicit-id-INSERTed into the rebuilt table;
-- we re-key it to `symbols` so the next AUTOINCREMENT continues from
-- MAX(id) rather than restarting at 1.
DELETE FROM sqlite_sequence WHERE name = 'symbols';
UPDATE sqlite_sequence SET name = 'symbols' WHERE name = 'new_symbols';

UPDATE schema_meta SET value = '12' WHERE key = 'schema_version';

COMMIT;

PRAGMA foreign_keys=ON;
