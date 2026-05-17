-- Stage 2 — generic FFI binding table.
--
-- Symbols that participate in a cross-language Foreign-Function-Interface
-- binding are tagged here with `(abi, direction, abi_name)`. The resolve
-- pass joins Exports to Imports across the workspace (and across
-- providers / projects / languages) by matching on `(abi, abi_name)`,
-- emitting an `IMPORTS` edge with attribute `ffi:<abi>`.
--
-- Why this shape instead of a flag on `symbols`:
--   * A single symbol can wear multiple FFI hats — e.g. a Rust fn
--     declared `#[no_mangle] pub extern "C" fn` that is ALSO referenced
--     by an `extern "C" { fn foo; }` block elsewhere (export + import
--     against itself, legal and observed in plugin glue).
--   * Conventions vary: `convention` is a free-form hint (`"lua-module"`,
--     `"jni-Java_pkg_Class"`, `"napi-register"`) that future taggers can
--     populate without schema churn.
--   * Resolve queries want a covering `(abi, abi_name, direction)`
--     index — easier to maintain on a dedicated table.
--
-- Composite PRIMARY KEY (symbol_id, abi, direction, abi_name) tolerates
-- the multi-hat case while preventing exact duplicates. ON DELETE
-- CASCADE drops every binding when its parent symbol is evicted.
--
-- Direction is constrained to ('export', 'import') at storage layer so
-- a buggy writer can't smuggle in an unknown direction the resolve
-- pass wouldn't handle.

CREATE TABLE IF NOT EXISTS symbol_ffi_binding (
    symbol_id  INTEGER NOT NULL,
    abi        TEXT    NOT NULL,
    direction  TEXT    NOT NULL CHECK (direction IN ('export', 'import')),
    abi_name   TEXT    NOT NULL,
    convention TEXT,
    PRIMARY KEY (symbol_id, abi, direction, abi_name),
    FOREIGN KEY (symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_symbol_ffi_binding_lookup
    ON symbol_ffi_binding(abi, abi_name, direction);

CREATE INDEX IF NOT EXISTS idx_symbol_ffi_binding_symbol
    ON symbol_ffi_binding(symbol_id);

UPDATE schema_meta SET value = '15' WHERE key = 'schema_version';
