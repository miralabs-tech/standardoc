use rusqlite::Connection;

use crate::storage::error::StorageError;

const INIT_V0_SQL: &str = include_str!("../../migrations/init_v0.sql");

pub(crate) fn run_init_schema(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(INIT_V0_SQL)?;
    Ok(())
}

#[cfg(test)]
mod tests;
