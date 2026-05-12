-- Persist the workspace revision counter in `schema_meta` so writes
-- performed by the primary LSP daemon are visible from a secondary
-- daemon (MCP daemon spawned alongside, historically named "--readonly"
-- but actually R/W under SQLite WAL to support `resolve_external`).
--
-- v1 through v5 stored the counter in a per-process `AtomicU64` that
-- never crossed process boundaries — so `current_revision` always
-- returned 0 on the MCP daemon while the LSP daemon bumped its own
-- counter. The agent-facing `check_stale` tool was effectively dead.
--
-- Storing the counter in SQLite leverages WAL's serialised writer
-- locking: concurrent `bump_revision` calls from LSP and MCP daemons
-- serialise correctly and both daemons observe the same monotonic
-- counter on read.

INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('revision', '0');

UPDATE schema_meta SET value = '6' WHERE key = 'schema_version';
