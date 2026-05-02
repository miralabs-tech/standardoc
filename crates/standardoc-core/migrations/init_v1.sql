CREATE TABLE schema_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT INTO schema_meta (key, value) VALUES
  ('schema_version',       '1'),
  ('workspace_root',       ''),
  ('created_at',           ''),
  ('cold_start_progress',  ''),
  ('watcher_debounce_ms',  '500');

CREATE TABLE files (
  path             TEXT    PRIMARY KEY,
  content_hash     TEXT    NOT NULL,
  language         TEXT    NOT NULL,
  last_scanned     INTEGER NOT NULL,
  byte_size        INTEGER NOT NULL,
  last_scan_error  TEXT
);

CREATE INDEX idx_files_language ON files(language);

CREATE TABLE symbols (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  fqdn            TEXT    NOT NULL UNIQUE,
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
                     'manual_external'))
);

CREATE INDEX idx_symbols_language    ON symbols(language);
CREATE INDEX idx_symbols_kind        ON symbols(kind);
CREATE INDEX idx_symbols_name        ON symbols(name);
CREATE INDEX idx_symbols_file_path   ON symbols(file_path);
CREATE INDEX idx_symbols_module      ON symbols(module) WHERE module IS NOT NULL;
CREATE INDEX idx_symbols_is_external ON symbols(is_external);

CREATE TABLE edges (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  from_symbol_id  INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  kind            TEXT    NOT NULL CHECK (kind IN
                    ('CALLS', 'IMPORTS', 'EXTENDS', 'IMPLEMENTS',
                     'REFERENCES', 'DEFINES', 'USES_TYPE', 'EXPOSES_API')),
  to_symbol_id    INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  to_unresolved   TEXT,
  CHECK (
    (to_symbol_id IS NOT NULL AND to_unresolved IS NULL) OR
    (to_symbol_id IS NULL     AND to_unresolved IS NOT NULL)
  )
);

CREATE INDEX idx_edges_from         ON edges(from_symbol_id);
CREATE INDEX idx_edges_to           ON edges(to_symbol_id) WHERE to_symbol_id IS NOT NULL;
CREATE INDEX idx_edges_kind         ON edges(kind);
CREATE INDEX idx_edges_to_kind      ON edges(to_symbol_id, kind) WHERE to_symbol_id IS NOT NULL;
CREATE INDEX idx_edges_from_kind    ON edges(from_symbol_id, kind);
CREATE INDEX idx_edges_unresolved   ON edges(to_unresolved) WHERE to_unresolved IS NOT NULL;

CREATE TABLE edge_sites (
  edge_id   INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE,
  file_path TEXT    NOT NULL,
  line      INTEGER NOT NULL,
  col       INTEGER NOT NULL,
  PRIMARY KEY (edge_id, file_path, line, col)
);

CREATE INDEX idx_edge_sites_file ON edge_sites(file_path);

CREATE TABLE documents (
  symbol_id     INTEGER PRIMARY KEY REFERENCES symbols(id) ON DELETE CASCADE,
  description   TEXT,
  examples_json TEXT,
  tags_json     TEXT,
  params_json   TEXT,
  returns_json  TEXT,
  ai_summary    TEXT,
  last_updated  INTEGER NOT NULL
);

CREATE TABLE enrichments (
  symbol_id      INTEGER PRIMARY KEY REFERENCES symbols(id) ON DELETE CASCADE,
  description    TEXT,
  params_json    TEXT,
  returns_json   TEXT,
  modifiers_json TEXT,
  confidence     TEXT NOT NULL CHECK (confidence IN ('low', 'medium', 'high')),
  sources_json   TEXT NOT NULL,
  last_updated   INTEGER NOT NULL
);

CREATE INDEX idx_enrichments_confidence ON enrichments(confidence);

CREATE TABLE enrichment_rejections (
  symbol_id    INTEGER PRIMARY KEY REFERENCES symbols(id) ON DELETE CASCADE,
  rejected_at  INTEGER NOT NULL,
  reason       TEXT
);

CREATE VIRTUAL TABLE symbols_fts USING fts5(
  name,
  fqdn,
  content='symbols',
  content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);

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
