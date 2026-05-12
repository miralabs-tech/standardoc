-- Adds `last_modified_revision` to `symbols` for per-symbol staleness tracking.
-- The MCP server uses this to detect when fqdns previously fetched by a session
-- have changed since their last read, so it can emit `stale_warnings` in tool
-- responses without false positives from unrelated edits elsewhere in the graph.
-- Default 0 marks rows written before the migration as "never modified at a
-- tracked revision"; they will be flagged as stale against any later fetched_at
-- revision, which is the conservative behavior (correct over precise).
ALTER TABLE symbols ADD COLUMN last_modified_revision INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_symbols_last_modified_revision ON symbols(last_modified_revision);

UPDATE schema_meta SET value = '4' WHERE key = 'schema_version';
