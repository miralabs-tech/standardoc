use serde::{Deserialize, Serialize};

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
}
