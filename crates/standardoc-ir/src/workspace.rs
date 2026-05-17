use serde::{Deserialize, Serialize};

/// Stage 3e-3 — workspace-level organizer kind, mirror of
/// `standarbuild_detect::WorkspaceKindId`. Decoupled from the external
/// crate's enum so the IR shape stays stable across detector-lib
/// version bumps. The conversion `from_kind_id` lives in
/// `standardoc-core::pipeline::projects`.
///
/// A "workspace kind" labels how the workspace ORGANIZES its projects —
/// Cargo `[workspace]`, npm `workspaces` field, pnpm-workspace.yaml,
/// `go.work`, etc. Multiple kinds can coexist at the same root (Tauri
/// = Cargo + npm); the storage layer picks ONE primary kind via
/// detection-order priority and persists it in `schema_meta`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "tag")]
pub enum WorkspaceKind {
	/// Cargo workspace — root `Cargo.toml` with `[workspace]`.
	Cargo,
	/// npm `workspaces` field in root `package.json`.
	Npm,
	/// `pnpm-workspace.yaml`.
	Pnpm,
	/// Yarn workspaces (`.yarnrc.yml` / `.yarn/` + `package.json` workspaces).
	Yarn,
	/// Bun workspaces (`bun.lock[b]` + `package.json` workspaces).
	Bun,
	/// Deno workspace (`deno.json[c]` `workspace` field).
	Deno,
	/// Go workspace (`go.work`).
	Go,
	/// Lerna monorepo (`lerna.json`).
	Lerna,
	/// Nx monorepo (`nx.json`).
	Nx,
	/// Turborepo (`turbo.json`).
	Turborepo,
	/// Mira self-hosting build manifest (`*.sxb` at root).
	Mira,
	/// Open-ended slot for custom monorepo tools (Bazel, Pants, Buck,
	/// custom DSLs). Mirrors `WorkspaceKindId::Custom`.
	Custom(String),
}

impl WorkspaceKind {
	/// Canonical lowercase slug for storage (`schema_meta.workspace_kind`).
	/// `Custom` variants render as `"custom:<tag>"` so they survive
	/// round-trip through the DB.
	pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
		match self {
			Self::Cargo => std::borrow::Cow::Borrowed("cargo"),
			Self::Npm => std::borrow::Cow::Borrowed("npm"),
			Self::Pnpm => std::borrow::Cow::Borrowed("pnpm"),
			Self::Yarn => std::borrow::Cow::Borrowed("yarn"),
			Self::Bun => std::borrow::Cow::Borrowed("bun"),
			Self::Deno => std::borrow::Cow::Borrowed("deno"),
			Self::Go => std::borrow::Cow::Borrowed("go"),
			Self::Lerna => std::borrow::Cow::Borrowed("lerna"),
			Self::Nx => std::borrow::Cow::Borrowed("nx"),
			Self::Turborepo => std::borrow::Cow::Borrowed("turborepo"),
			Self::Mira => std::borrow::Cow::Borrowed("mira"),
			Self::Custom(tag) => std::borrow::Cow::Owned(format!("custom:{tag}")),
		}
	}

	/// Inverse of [`Self::as_str`]. Returns `Self::Custom` for any slug
	/// not matched against the built-in variants (mirrors
	/// `WorkspaceKindId::from_slug` shape — total function).
	pub fn from_str(s: &str) -> Self {
		match s {
			"cargo" => Self::Cargo,
			"npm" => Self::Npm,
			"pnpm" => Self::Pnpm,
			"yarn" => Self::Yarn,
			"bun" => Self::Bun,
			"deno" => Self::Deno,
			"go" => Self::Go,
			"lerna" => Self::Lerna,
			"nx" => Self::Nx,
			"turborepo" => Self::Turborepo,
			"mira" => Self::Mira,
			other => other
				.strip_prefix("custom:")
				.map(|tag| Self::Custom(tag.to_string()))
				.unwrap_or_else(|| Self::Custom(other.to_string())),
		}
	}

	/// True for every named variant — `false` only for [`Self::Custom`].
	pub fn is_builtin(&self) -> bool {
		!matches!(self, Self::Custom(_))
	}
}

impl std::fmt::Display for WorkspaceKind {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.as_str())
	}
}

/// Direction of the linkage between the primary workspace and a peer
/// registered in `workspace_catalog`. Encoded as a small integer in the
/// SQLite column (`link_direction INTEGER CHECK IN (0, 1, 2)`).
///
/// - `In` (0) — the linked workspace's symbols are consumed by us.
///   Use case: an external project we want to reference (e.g. our
///   project depends on it).
/// - `Out` (1) — the linked workspace consumes our symbols. Use case:
///   we own a library whose consumers we want to track.
/// - `Bidirectional` (2) — both workspaces consume each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirection {
	In,
	Out,
	Bidirectional,
}

impl LinkDirection {
	pub fn as_i64(self) -> i64 {
		match self {
			Self::In => 0,
			Self::Out => 1,
			Self::Bidirectional => 2,
		}
	}

	pub fn from_i64(value: i64) -> Option<Self> {
		match value {
			0 => Some(Self::In),
			1 => Some(Self::Out),
			2 => Some(Self::Bidirectional),
			_ => None,
		}
	}
}

/// Lifecycle status of a linked workspace entry. Stored as TEXT in the
/// `workspace_catalog.status` column (CHECK constraint enforces).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkedWorkspaceStatus {
	Active,
	Paused,
	Archived,
}

impl LinkedWorkspaceStatus {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Active => "active",
			Self::Paused => "paused",
			Self::Archived => "archived",
		}
	}

	pub fn from_str(value: &str) -> Option<Self> {
		match value {
			"active" => Some(Self::Active),
			"paused" => Some(Self::Paused),
			"archived" => Some(Self::Archived),
			_ => None,
		}
	}
}

/// How primary indexes a linked peer workspace. Stored as TEXT in the
/// `workspace_catalog.indexing_mode` column (CHECK constraint enforces,
/// schema v13).
///
/// - `BlobImport` — Stage 3b-7-a path: primary copies the peer's
///   pre-built `module_lookups` + `workspace_imports` blobs. Cheap,
///   but assumes the peer's DB is fresh and schema-compatible. Default
///   for legacy rows + new links unless the caller opts in.
/// - `Extract` — Stage 3b-7-b path: primary walks the peer's source
///   files directly via `pipeline::peer_extract` and indexes them
///   under the peer's `workspace_id`. Authoritative, no schema-version
///   assumption on the peer side, but more expensive at cold_start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IndexingMode {
	#[default]
	BlobImport,
	Extract,
}

impl IndexingMode {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::BlobImport => "blob_import",
			Self::Extract => "extract",
		}
	}

	pub fn from_str(value: &str) -> Option<Self> {
		match value {
			"blob_import" => Some(Self::BlobImport),
			"extract" => Some(Self::Extract),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn link_direction_roundtrip_via_i64() {
		for d in [
			LinkDirection::In,
			LinkDirection::Out,
			LinkDirection::Bidirectional,
		] {
			let n = d.as_i64();
			let back = LinkDirection::from_i64(n).expect("known value");
			assert_eq!(back, d);
		}
		assert!(LinkDirection::from_i64(3).is_none());
		assert!(LinkDirection::from_i64(-1).is_none());
	}

	#[test]
	fn linked_workspace_status_roundtrip_via_str() {
		for s in [
			LinkedWorkspaceStatus::Active,
			LinkedWorkspaceStatus::Paused,
			LinkedWorkspaceStatus::Archived,
		] {
			let txt = s.as_str();
			let back = LinkedWorkspaceStatus::from_str(txt).expect("known value");
			assert_eq!(back, s);
		}
		assert!(LinkedWorkspaceStatus::from_str("bogus").is_none());
	}

	#[test]
	fn link_direction_serde_uses_snake_case() {
		let json = serde_json::to_string(&LinkDirection::Bidirectional).unwrap();
		assert_eq!(json, "\"bidirectional\"");
		let back: LinkDirection = serde_json::from_str(&json).unwrap();
		assert_eq!(back, LinkDirection::Bidirectional);
	}

	#[test]
	fn workspace_kind_roundtrip_via_as_str_for_known_variants() {
		for k in [
			WorkspaceKind::Cargo,
			WorkspaceKind::Npm,
			WorkspaceKind::Pnpm,
			WorkspaceKind::Yarn,
			WorkspaceKind::Bun,
			WorkspaceKind::Deno,
			WorkspaceKind::Go,
			WorkspaceKind::Lerna,
			WorkspaceKind::Nx,
			WorkspaceKind::Turborepo,
			WorkspaceKind::Mira,
		] {
			let s = k.as_str().into_owned();
			assert_eq!(WorkspaceKind::from_str(&s), k.clone());
			assert!(k.is_builtin());
		}
	}

	#[test]
	fn workspace_kind_custom_roundtrips_via_custom_prefix() {
		let k = WorkspaceKind::Custom("bazel".into());
		let s = k.as_str().into_owned();
		assert_eq!(s, "custom:bazel");
		assert_eq!(WorkspaceKind::from_str(&s), k);
		assert!(!k.is_builtin());
	}

	#[test]
	fn workspace_kind_unknown_slug_becomes_custom() {
		// Total function — unknown slugs are absorbed as Custom rather
		// than rejected. Mirrors `WorkspaceKindId::from_slug`.
		let k = WorkspaceKind::from_str("scala-sbt");
		assert_eq!(k, WorkspaceKind::Custom("scala-sbt".into()));
	}

	#[test]
	fn workspace_kind_legacy_single_slug_becomes_custom() {
		// The `Single` variant was dropped in the 3e-3 revert to align with
		// `standarbuild-detect 0.3` (which has no `Single`). Legacy DBs
		// persisted with `"single"` round-trip as `Custom("single")` —
		// `delete_workspace_kind` purges the row at the next cold-start
		// when no workspace manifest is detected.
		let k = WorkspaceKind::from_str("single");
		assert_eq!(k, WorkspaceKind::Custom("single".into()));
	}

	#[test]
	fn workspace_kind_serde_uses_external_tagging() {
		let k = WorkspaceKind::Custom("bazel".into());
		let json = serde_json::to_string(&k).unwrap();
		let back: WorkspaceKind = serde_json::from_str(&json).unwrap();
		assert_eq!(back, k);
	}
}
