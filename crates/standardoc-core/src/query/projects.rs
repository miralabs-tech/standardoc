//! Stage 3d — query-layer façade for the `projects` table.
//!
//! Public surface consumed by MCP/LSP and the cold-start sweep. Storage
//! ops stay `pub(crate)`; this module is the only entry point exposed
//! to the daemon's request paths.

use standardoc_ir::{ProjectInfo, ProjectKind};

use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::projects;

/// UPSERT a project (used by the cold-start `discover()` sweep and the
/// watcher's manifest-change handler). Returns the project_id.
pub fn upsert_project(
    handle: &IndexHandle,
    label: &str,
    kind: &ProjectKind,
    root_path: &str,
    rel_path: &str,
) -> Result<u32, StorageError> {
    let conn = handle.conn()?;
    projects::upsert_project(&conn, label, kind, root_path, rel_path)
}

pub fn list_projects(handle: &IndexHandle) -> Result<Vec<ProjectInfo>, StorageError> {
    let conn = handle.conn()?;
    projects::list_projects(&conn)
}

/// Resolve a file's owning project. `file_abs_path` must be canonical.
pub fn project_for_file(
    handle: &IndexHandle,
    file_abs_path: &str,
) -> Result<Option<ProjectInfo>, StorageError> {
    let conn = handle.conn()?;
    projects::find_for_file_path(&conn, file_abs_path)
}
