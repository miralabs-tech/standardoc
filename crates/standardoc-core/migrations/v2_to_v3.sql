-- Adds the `confidence` column to `edges`. Three-tier scoring inspired by
-- EXTRACTED/INFERRED/AMBIGUOUS model. Default 'extracted'
-- keeps the column neutral for every edge written before the migration — the
-- baseline assumes the AST gave the target explicitly. Resolvers with finer
-- knowledge (alias lookup, module-local fallback, multi-candidate) override
-- at emit time via `RawEdge.confidence` or `ResolvedOrUnresolved::default_confidence()`.
ALTER TABLE edges ADD COLUMN confidence TEXT NOT NULL DEFAULT 'extracted'
  CHECK (confidence IN ('extracted', 'inferred', 'ambiguous'));

CREATE INDEX idx_edges_confidence ON edges(confidence);

UPDATE schema_meta SET value = '3' WHERE key = 'schema_version';
