-- Stage 3b-7-b Layer 1: tag every symbol row with the workspace it
-- belongs to. Prep work for the autonomous peer indexer (Layer 3)
-- which will index peer source files under primary's daemon and
-- write their symbols into this table tagged with the peer's
-- workspace_id (vs the current model where only `module_lookups` /
-- `workspace_imports` carry workspace_id and `symbols` is implicitly
-- primary-only).
--
-- Schema choices:
-- - `workspace_id TEXT NOT NULL DEFAULT 'primary'`: the literal value
--   must stay in lockstep with `storage::module_lookup::PRIMARY_WORKSPACE_ID`
--   (currently `"primary"`). Default makes the migration a no-op for
--   existing rows — every pre-v11 symbol belonged to primary, full stop.
-- - `idx_symbols_workspace_id_fqdn`: composite index supporting the
--   Layer-2 scope-aware lookups (`WHERE workspace_id = ? AND fqdn = ?`).
--   Plain `idx_symbols_fqdn` no longer exists (fqdn UNIQUE provides its
--   own implicit index), but the composite is what cross-workspace
--   queries will hit.
-- - The existing `UNIQUE (fqdn)` constraint is intentionally LEFT IN
--   PLACE for now. Layer 3 will need to relax it to `UNIQUE
--   (workspace_id, fqdn)` to allow peer rows that collide with primary,
--   but that's an invasive table rebuild (SQLite can't drop a UNIQUE
--   in-place) — bundling it with the actual peer indexing work keeps
--   the layered roll-out reviewable. Primary-only writes during Layer
--   1/2 are unaffected by the v10 UNIQUE.

ALTER TABLE symbols ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'primary';

CREATE INDEX idx_symbols_workspace_id_fqdn ON symbols(workspace_id, fqdn);

UPDATE schema_meta SET value = '11' WHERE key = 'schema_version';
