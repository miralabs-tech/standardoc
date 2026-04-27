use serde::{Deserialize, Serialize};

/// Confidence tier for virtual annotations synthesized from AST + heuristics.
///
/// Producers emit the highest tier across all signals that contributed; consumers
/// (UI, agents) can filter or color virtual content based on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VirtualConfidence {
    Low,
    Medium,
    High,
}
