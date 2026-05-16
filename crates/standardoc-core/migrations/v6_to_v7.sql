-- Stage 3b — AOT module lookup persistence + cross-workspace catalog.
--
-- Stage 3a built ModuleLookup tables in memory per-extract. Stage 3b
-- persists them so:
--   1. Subsequent extracts can avoid rebuilding (cached AOT resolution).
--   2. Cross-workspace import resolution can SQL-join against another
--      workspace's exports without re-parsing.
--
-- `module_lookups` stores the bincode-serialised `ModuleLookup` blob
-- keyed by (workspace_id, module_fqdn). The 'primary' sentinel is
-- reserved for the current workspace; linked workspaces are assigned
-- UUID v4 ids registered in `workspace_catalog`.
--
-- `workspace_imports` is a flat unrolled mirror of every
-- `ImportRecord` inside each ModuleLookup payload — kept separate so
-- cross-workspace resolution can SQL-join on `origin_module` without
-- deserialising every blob.
--
-- `workspace_catalog` tracks linked workspaces. `link_direction`:
--   0 = the linked workspace's symbols are consumed by us (one-way in)
--   1 = our workspace's symbols are consumed by the linked one (one-way out)
--   2 = bidirectional.

CREATE TABLE workspace_catalog (
  workspace_id    TEXT    PRIMARY KEY,
  root_path       TEXT    NOT NULL,
  link_direction  INTEGER NOT NULL DEFAULT 0 CHECK (link_direction IN (0, 1, 2)),
  linked_at       INTEGER NOT NULL,
  last_indexed_at INTEGER,
  status          TEXT    NOT NULL DEFAULT 'active' CHECK (status IN
                    ('active', 'paused', 'archived'))
);

CREATE INDEX idx_workspace_catalog_root_path ON workspace_catalog(root_path);

CREATE TABLE module_lookups (
  module_fqdn  TEXT    NOT NULL,
  workspace_id TEXT    NOT NULL DEFAULT 'primary',
  language     TEXT    NOT NULL,
  built_at     INTEGER NOT NULL,
  payload      BLOB    NOT NULL,
  PRIMARY KEY (workspace_id, module_fqdn)
);

CREATE INDEX idx_module_lookups_language ON module_lookups(language);

CREATE TABLE workspace_imports (
  workspace_id  TEXT    NOT NULL DEFAULT 'primary',
  module_fqdn   TEXT    NOT NULL,
  local_name    TEXT    NOT NULL,
  origin_module TEXT    NOT NULL,
  origin_symbol TEXT,
  is_type_only  INTEGER NOT NULL DEFAULT 0 CHECK (is_type_only IN (0, 1)),
  is_re_export  INTEGER NOT NULL DEFAULT 0 CHECK (is_re_export IN (0, 1)),
  PRIMARY KEY (workspace_id, module_fqdn, local_name)
);

CREATE INDEX idx_workspace_imports_origin_module ON workspace_imports(origin_module);
CREATE INDEX idx_workspace_imports_workspace_id ON workspace_imports(workspace_id);

UPDATE schema_meta SET value = '7' WHERE key = 'schema_version';
