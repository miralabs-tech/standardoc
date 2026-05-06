-- Adds the `attributes` JSON column to `edges`. Used by template-extraction
-- providers (Vue / Svelte / React JSX) to tag emitted edges with semantic
-- categories (e.g. "template-event", "template-bind"). Default '[]' keeps
-- the column neutral for every edge written before the migration.
ALTER TABLE edges ADD COLUMN attributes TEXT NOT NULL DEFAULT '[]';

UPDATE schema_meta SET value = '2' WHERE key = 'schema_version';
