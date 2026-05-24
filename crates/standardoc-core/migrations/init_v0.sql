-- Standardoc storage schema — v0 baseline (post-reboot, beta era).
--
-- Single-source-of-truth schema. The v1→v15 migration chain inherited
-- from earlier iterations was consolidated into this file once the
-- shape stabilised. From now on, breaking changes bump
-- SUPPORTED_SCHEMA_VERSION and any mismatch on boot triggers a
-- destructive rebuild (the index is a derived cache — no user data).
--
-- FK ordering is significant: tables referenced by FK constraints
-- must precede their dependants. Order below: schema_meta → projects
-- → files → symbols → edges → edge_sites → per-symbol satellites →
-- workspace layer → FTS virtual + triggers.

CREATE TABLE schema_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT INTO schema_meta (key, value) VALUES
  ('schema_version',              '4'),
  ('workspace_root',              ''),
  ('created_at',                  ''),
  ('cold_start_progress',         ''),
  ('watcher_debounce_ms',         '500'),
  ('revision',                    '0'),
  ('external_cargo_lockfile_hash',   ''),
  ('external_npm_lockfile_hash',     ''),
  ('external_npm_lockfile_kind',     ''),
  ('external_luarocks_hash',         '');

-- Polyglot monorepo metadata. One row per detected project root
-- (Rust crate / Bun pkg / Deno script / ...). `files.project_id`
-- joins back here for grouped queries.
CREATE TABLE projects (
  project_id INTEGER PRIMARY KEY AUTOINCREMENT,
  label      TEXT    NOT NULL,
  kind       TEXT    NOT NULL,
  root_path  TEXT    NOT NULL UNIQUE,
  rel_path   TEXT    NOT NULL
);

CREATE INDEX idx_projects_kind ON projects(kind);

CREATE TABLE files (
  path             TEXT    PRIMARY KEY,
  content_hash     TEXT    NOT NULL,
  language         TEXT    NOT NULL,
  last_scanned     INTEGER NOT NULL,
  byte_size        INTEGER NOT NULL,
  last_scan_error  TEXT,
  is_external      INTEGER NOT NULL DEFAULT 0 CHECK (is_external IN (0, 1)),
  project_id       INTEGER REFERENCES projects(project_id)
);

CREATE INDEX idx_files_is_external ON files(is_external) WHERE is_external = 1;
CREATE INDEX idx_files_project_id  ON files(project_id)  WHERE project_id IS NOT NULL;

CREATE TABLE symbols (
  id                     INTEGER PRIMARY KEY AUTOINCREMENT,
  fqdn                   TEXT    NOT NULL,
  name                   TEXT    NOT NULL,
  kind                   TEXT    NOT NULL CHECK (kind IN
                           ('callable', 'type', 'value', 'module', 'macro')),
  language_kind          TEXT    NOT NULL,
  language               TEXT    NOT NULL,
  module                 TEXT,
  visibility             TEXT    NOT NULL DEFAULT 'public' CHECK (visibility IN
                           ('public', 'private', 'crate', 'protected')),
  file_path              TEXT    NOT NULL REFERENCES files(path) ON DELETE CASCADE,
  start_line             INTEGER NOT NULL,
  end_line               INTEGER NOT NULL,
  start_col              INTEGER NOT NULL,
  end_col                INTEGER NOT NULL,
  signature_json         TEXT,
  body_hash              TEXT,
  decl_kind              TEXT,
  implements_trait       TEXT,
  receiver_type          TEXT,
  entry_point            TEXT    CHECK (entry_point IS NULL OR entry_point IN
                           ('binary_main', 'public_api', 'ffi_export')),
  is_external            INTEGER NOT NULL DEFAULT 0 CHECK (is_external IN (0, 1)),
  source_origin          TEXT    NOT NULL DEFAULT 'workspace' CHECK (source_origin IN
                           ('workspace', 'cargo_registry', 'node_modules_dts',
                            'manual_external')),
  last_modified_revision INTEGER NOT NULL DEFAULT 0,
  flags                  TEXT    NOT NULL DEFAULT '[]',
  workspace_id           TEXT    NOT NULL DEFAULT 'primary',
  UNIQUE (workspace_id, fqdn)
);

-- Composite (file_path, start_line, start_col) covers the
-- LSP doc-symbols hot query: WHERE file_path = ? ORDER BY
-- start_line, start_col. Replaces the older idx_symbols_file_path.
CREATE INDEX idx_symbols_file_pos    ON symbols(file_path, start_line, start_col);
CREATE INDEX idx_symbols_name        ON symbols(name);
CREATE INDEX idx_symbols_module      ON symbols(module) WHERE module IS NOT NULL;
CREATE INDEX idx_symbols_is_external ON symbols(is_external) WHERE is_external = 1;

CREATE TABLE edges (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  from_symbol_id  INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  kind            TEXT    NOT NULL CHECK (kind IN
                    ('CALLS', 'IMPORTS', 'EXTENDS', 'IMPLEMENTS',
                     'REFERENCES', 'USES_TYPE')),
  to_symbol_id    INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  to_unresolved   TEXT,
  attributes      TEXT    NOT NULL DEFAULT '[]',
  confidence      TEXT    NOT NULL DEFAULT 'extracted' CHECK (confidence IN
                    ('extracted', 'inferred', 'ambiguous')),
  CHECK (
    (to_symbol_id IS NOT NULL AND to_unresolved IS NULL) OR
    (to_symbol_id IS NULL     AND to_unresolved IS NOT NULL)
  )
);

CREATE INDEX idx_edges_from_kind  ON edges(from_symbol_id, kind);
CREATE INDEX idx_edges_to_kind    ON edges(to_symbol_id, kind) WHERE to_symbol_id IS NOT NULL;
CREATE INDEX idx_edges_unresolved ON edges(to_unresolved)      WHERE to_unresolved IS NOT NULL;

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

-- Scaffold for future AI-inference layer. Read in context_for_symbol
-- via LEFT JOIN (LSP hover / MCP get_context); writer not yet wired.
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

-- Pair to enrichments — explicit opt-out so the future writer can
-- skip symbols flagged by the user.
CREATE TABLE enrichment_rejections (
  symbol_id    INTEGER PRIMARY KEY REFERENCES symbols(id) ON DELETE CASCADE,
  rejected_at  INTEGER NOT NULL,
  reason       TEXT
);

CREATE TABLE call_sites (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  from_fqdn           TEXT    NOT NULL,
  callee_text         TEXT    NOT NULL,
  args_json           TEXT    NOT NULL DEFAULT '[]',
  receiver_chain_json TEXT    NOT NULL DEFAULT '[]',
  file_path           TEXT    NOT NULL REFERENCES files(path) ON DELETE CASCADE,
  line                INTEGER NOT NULL,
  col                 INTEGER NOT NULL
);

CREATE INDEX idx_call_sites_from_fqdn   ON call_sites(from_fqdn);
CREATE INDEX idx_call_sites_callee_text ON call_sites(callee_text);
CREATE INDEX idx_call_sites_file_path   ON call_sites(file_path);

-- C provider: header (.h) declaration twin for symbols whose
-- definition lives in a .c file. One row per symbol that has a
-- secondary location to surface.
CREATE TABLE symbol_decl_location (
  symbol_id  INTEGER PRIMARY KEY REFERENCES symbols(id) ON DELETE CASCADE,
  file       TEXT    NOT NULL,
  start_line INTEGER NOT NULL,
  end_line   INTEGER NOT NULL,
  start_col  INTEGER NOT NULL,
  end_col    INTEGER NOT NULL
);

CREATE INDEX idx_symbol_decl_location_file ON symbol_decl_location(file);

-- Generic FFI binding: symbols on either side of a cross-language
-- export/import join. Resolved at extract time by the FFI tagger.
CREATE TABLE symbol_ffi_binding (
  symbol_id  INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  abi        TEXT    NOT NULL,
  direction  TEXT    NOT NULL CHECK (direction IN ('export', 'import')),
  abi_name   TEXT    NOT NULL,
  convention TEXT,
  PRIMARY KEY (symbol_id, abi, direction, abi_name)
);

CREATE INDEX idx_symbol_ffi_binding_lookup ON symbol_ffi_binding(abi, abi_name, direction);

-- Linked workspaces. Each row is a peer workspace registered for
-- cross-workspace queries. link_direction: 0=in, 1=out, 2=bidir.
CREATE TABLE workspace_catalog (
  workspace_id    TEXT    PRIMARY KEY,
  root_path       TEXT    NOT NULL,
  link_direction  INTEGER NOT NULL DEFAULT 0 CHECK (link_direction IN (0, 1, 2)),
  linked_at       INTEGER NOT NULL,
  last_indexed_at INTEGER,
  status          TEXT    NOT NULL DEFAULT 'active' CHECK (status IN
                    ('active', 'paused', 'archived')),
  indexing_mode   TEXT    NOT NULL DEFAULT 'blob_import' CHECK (indexing_mode IN
                    ('blob_import', 'extract'))
);

CREATE INDEX idx_workspace_catalog_root_path ON workspace_catalog(root_path);

-- Bincode payload of the AOT ModuleLookup per (workspace_id, module_fqdn).
CREATE TABLE module_lookups (
  module_fqdn  TEXT    NOT NULL,
  workspace_id TEXT    NOT NULL DEFAULT 'primary',
  language     TEXT    NOT NULL,
  built_at     INTEGER NOT NULL,
  payload      BLOB    NOT NULL,
  PRIMARY KEY (workspace_id, module_fqdn)
);

CREATE INDEX idx_module_lookups_language ON module_lookups(language);

-- Flat unrolled mirror of every ImportRecord inside each ModuleLookup
-- payload. Kept separate so cross-workspace resolution can SQL-join
-- on origin_module without deserialising every blob.
--
-- PK includes origin_module because a single module can legitimately
-- emit multiple imports with the same local_name when origins differ:
--   - `use foo::*; use bar::*;`           (two globs, local_name='*')
--   - `#[cfg(unix)] use a::Foo; #[cfg(win)] use b::Foo;` (cfg duals)
-- The extractor doesn't prune cfg, so both arms reach the SQL layer.
CREATE TABLE workspace_imports (
  workspace_id  TEXT    NOT NULL DEFAULT 'primary',
  module_fqdn   TEXT    NOT NULL,
  local_name    TEXT    NOT NULL,
  origin_module TEXT    NOT NULL,
  origin_symbol TEXT,
  is_type_only  INTEGER NOT NULL DEFAULT 0 CHECK (is_type_only IN (0, 1)),
  is_re_export  INTEGER NOT NULL DEFAULT 0 CHECK (is_re_export IN (0, 1)),
  PRIMARY KEY (workspace_id, module_fqdn, local_name, origin_module)
);

CREATE INDEX idx_workspace_imports_origin_module ON workspace_imports(origin_module);

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
