-- Stage 1c: header / impl declaration twin support for the C provider.
--
-- The `symbol_decl_location` table holds an optional secondary source
-- location for symbols whose declaration lives in a different file than
-- their definition. Today this is consumed exclusively by the C
-- `LanguageProvider` join pass: when `.h` declares `void foo(int);` and
-- `.c` defines `void foo(int x) { ... }`, the post-extraction join keeps
-- the function_def row (with its body location in `symbols.start_line`/
-- `end_line`) and stores the matching `.h` location here.
--
-- The 1:1 PRIMARY KEY on symbol_id makes the row optional by default — a
-- symbol without a decl twin simply has no entry. ON DELETE CASCADE
-- keeps the table consistent when symbols are evicted (file removal,
-- workspace unlink, peer refresh).
--
-- Indexed on `file` so the watcher can reverse-lookup "which symbols
-- have a decl in this header?" when a `.h` change invalidates joins.

CREATE TABLE IF NOT EXISTS symbol_decl_location (
    symbol_id  INTEGER PRIMARY KEY,
    file       TEXT    NOT NULL,
    start_line INTEGER NOT NULL,
    end_line   INTEGER NOT NULL,
    start_col  INTEGER NOT NULL,
    end_col    INTEGER NOT NULL,
    FOREIGN KEY (symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_symbol_decl_location_file
    ON symbol_decl_location(file);

UPDATE schema_meta SET value = '14' WHERE key = 'schema_version';
