//! Standardoc workspace configuration.
//!
//! Today this module exposes the `.sxd` loader (see [`sxd`]). The
//! `.sxd` config consolidates the legacy `.stdignore` file with new
//! project / group definitions used by the viz Overview. Back-compat
//! with `.stdignore` lives in `pipeline::filters` until the migration
//! path lands.

pub mod sxd;

pub use sxd::{
    GroupBlock, IgnoreBlock, McpBlock, ProjectBlock, SXD_CONFIG_FILENAME, SxdConfig,
    SxdConfigError, VizBlock, ensure_sxd_seed_at, load_workspace_config, parse_sxd_source,
};
