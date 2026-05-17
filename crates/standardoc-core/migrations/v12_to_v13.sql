-- Stage 3b-7-b Layer 3c: per-peer indexing mode opt-in.
--
-- Adds an `indexing_mode` column to `workspace_catalog` so each linked
-- peer can independently pick between the two extraction paths shipped
-- in 3b-7-a / 3b-7-b:
--
--   - 'blob_import' (default for legacy rows + new links unless opted
--     in) — primary copies the peer's pre-built `module_lookups` +
--     `workspace_imports` blobs. Cheap, but assumes peer's DB is fresh
--     and schema-compatible. Trusted-peer / monorepo-with-paired-daemons
--     scenarios stay on this path.
--   - 'extract' — primary walks the peer's source files directly and
--     indexes them under the peer's workspace_id via the
--     `pipeline::peer_extract` module (Layer 3b). Authoritative,
--     no schema-version assumption on the peer side, but more
--     expensive at cold_start time.
--
-- Default = 'blob_import' so the migration is a behavioural no-op for
-- existing peers — the 3b-7-a flow keeps running until users explicitly
-- opt a peer into 'extract' via the link_workspace MCP API.
--
-- CHECK constraint enforces the closed enum at storage layer so a buggy
-- writer can't smuggle in an unknown mode that the dispatcher in
-- `cold_start` wouldn't handle.

ALTER TABLE workspace_catalog ADD COLUMN indexing_mode TEXT
  NOT NULL DEFAULT 'blob_import'
  CHECK (indexing_mode IN ('blob_import', 'extract'));

UPDATE schema_meta SET value = '13' WHERE key = 'schema_version';
