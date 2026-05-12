-- External resolvers (S3-G lazy on-demand) need two storage extensions:
--
-- 1. Lockfile-hash baselines in `schema_meta` so the cold-start
--    invalidation step can purge stale external symbols when a
--    lockfile changes between daemon runs. Blank string is the
--    "unset" sentinel (consistent with the v1 init convention for
--    `workspace_root` / `created_at` / `cold_start_progress`).
--
--    - `external_cargo_lockfile_hash` : BLAKE3 hex of the workspace
--      `Cargo.lock` that produced the currently cached
--      `is_external=1 AND source_origin='cargo_registry'` symbols.
--      Diff vs. live hash triggers purge.
--    - `external_npm_lockfile_hash` : same idea for the npm-family
--      lockfile.
--    - `external_npm_lockfile_kind` : which file the npm hash was
--      taken from (`package-lock.json` | `pnpm-lock.yaml` |
--      `yarn.lock` | `.pnp.cjs`). Stored separately from the hash so
--      a kind switch (e.g. npm → pnpm) also triggers a purge even
--      when the new file happens to hash to the old value.
--    - `external_luarocks_hash` : hash of `luarocks list --porcelain`
--      output captured at boot. luarocks has no canonical lockfile so
--      the resolver hashes its own subprocess output.
--
-- 2. `files.is_external` flag so the cold-start `cleanup_unseen` step
--    can SKIP rows that point at external sources (`~/.cargo/registry/`,
--    `node_modules/`, `~/.luarocks/`). Without this flag every external
--    file would be purged on every daemon restart because the workspace
--    walk never visits external paths. The flag is also used by the
--    invalidation step's `purge_externals_by_origin` to scope DELETEs.

INSERT OR IGNORE INTO schema_meta (key, value) VALUES
  ('external_cargo_lockfile_hash', ''),
  ('external_npm_lockfile_hash',   ''),
  ('external_npm_lockfile_kind',   ''),
  ('external_luarocks_hash',       '');

ALTER TABLE files ADD COLUMN is_external INTEGER NOT NULL DEFAULT 0
  CHECK (is_external IN (0, 1));

CREATE INDEX idx_files_is_external ON files(is_external) WHERE is_external = 1;

UPDATE schema_meta SET value = '5' WHERE key = 'schema_version';
