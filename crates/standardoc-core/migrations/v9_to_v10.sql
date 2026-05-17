-- IR-4-f (post-1.0 prep): persist `RawCallSite` records emitted by the
-- extractors since IR-4-b/c/d. The IR populates a vec per
-- `ExtractedFile`, but `pipeline::batch::apply_upsert_file` dropped it
-- silently — the TODO posted in IR-4-e (commit d54f394) called this
-- out explicitly. This migration adds the dedicated table the plugin
-- layer reads to interpret textual call patterns without re-parsing
-- source.
--
-- Schema choices:
-- - `from_fqdn TEXT` (no FK to `symbols.fqdn`): a call_site's enclosing
--   FQDN may not have a symbol row yet (top-level expressions, synthetic
--   module scopes) — same rationale as `edges.to_unresolved`. FK would
--   force write-ordering constraints that observational data shouldn't
--   carry.
-- - `args_json` / `receiver_chain_json` TEXT: inline JSON arrays keep
--   the Vec<RawCallArg> / Vec<String> shapes round-trippable without a
--   second-level table. Queries that need to filter ON receiver-chain
--   segments use `LIKE '%segment%'` or `json_each` until indexing
--   demand forces a v2 normalization.
-- - `file_path` FK with `ON DELETE CASCADE` mirrors `symbols.file_path`
--   — file-level delete / re-extract drops the dependent call_sites in
--   one stroke. Same lifecycle as edge_sites.
--
-- Indexes:
-- - `idx_call_sites_from_fqdn`: plugin layer asks "who calls into X?"
--   queries by enclosing FQDN.
-- - `idx_call_sites_callee_text`: "what does X call?" plus dedupe / hot-
--   callee aggregation.
-- - `idx_call_sites_file_path`: file-scoped delete on re-extract +
--   per-file diagnostics.

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

UPDATE schema_meta SET value = '10' WHERE key = 'schema_version';
