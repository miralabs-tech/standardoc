-- Stage 3e-1b: surface computed semantic flags on every `RawSymbol`
-- row. Distinct from `attributes` (source-level decorators like
-- `#[derive(...)]` / TS class decorators) and from `signature.modifiers`
-- (syntactic keywords like `async`, `const`, `unsafe`). The new column
-- stores a JSON array of opaque tag strings — e.g. `["async"]` when a
-- fn's return type is `Promise<T>` / `Future<T>`, `["iter"]` when an
-- Iterator-family trait is touched, plus arbitrary UST-language custom
-- tags (`lua:coroutine-yielding`, …) registered post-1.0.
--
-- Stored as TEXT to keep the IR's `Vec<String>` shape open-ended: new
-- flag taxonomies arrive without a schema migration. SQLite's
-- `json_each` / `LIKE` cover the query story until we need real
-- indexing.
ALTER TABLE symbols ADD COLUMN flags TEXT NOT NULL DEFAULT '[]';

UPDATE schema_meta SET value = '9' WHERE key = 'schema_version';
