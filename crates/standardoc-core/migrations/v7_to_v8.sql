-- Stage 3d — per-project metadata for polyglot monorepos.
--
-- A workspace usually contains multiple distinct projects (a Rust crate,
-- a Bun-powered VSCode extension, a Deno script, ...). The cold-start
-- detection layer (via the `standarbuild-detect` crate) populates the
-- `projects` table once at boot; every indexed file then carries a
-- `project_id` FK so consumers can scope queries by ownership.
--
-- `kind` stores the canonical lowercase slug from `ProjectKind::as_str`
-- (e.g. `"rust"`, `"node"`). Custom variants serialise as `"custom:<tag>"`
-- to keep the column TEXT and round-trip via `ProjectKind::from_str`.
-- No CHECK constraint on `kind` so future custom variants don't require
-- a schema bump.

CREATE TABLE projects (
  project_id  INTEGER PRIMARY KEY AUTOINCREMENT,
  label       TEXT    NOT NULL,
  kind        TEXT    NOT NULL,
  root_path   TEXT    NOT NULL UNIQUE,
  rel_path    TEXT    NOT NULL
);

CREATE INDEX idx_projects_root_path ON projects(root_path);
CREATE INDEX idx_projects_kind ON projects(kind);

-- Non-destructive ALTER: existing rows get NULL project_id until the
-- next cold-start runs detection. The watcher repopulates on file
-- ingestion.
ALTER TABLE files ADD COLUMN project_id INTEGER NULL REFERENCES projects(project_id);

CREATE INDEX idx_files_project_id ON files(project_id);

UPDATE schema_meta SET value = '8' WHERE key = 'schema_version';
