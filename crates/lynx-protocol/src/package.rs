//! Downstream delivery payload and its capability declaration.

use serde::{Deserialize, Serialize};

use crate::evidence::Evidence;
use crate::relation::Relation;
use crate::snapshot::Snapshot;

/// Maximum guarantee the substrate can currently make about its output.
///
/// Levels are cumulative: `L3Structural` implies everything below it.
/// Wire form: each variant serializes as its identifier verbatim.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CapabilityLevel {
    /// Raw text handling only; unknown formats.
    L0Text,
    /// Syntax-aware chunks without symbol resolution.
    L1Parsed,
    /// Symbol definitions extracted and identified.
    L2Symbolized,
    /// Local call-graph and dependency relations available.
    L3Structural,
}

/// Consolidated payload handed to downstream agents (Lea, Izen, external MCP
/// clients) in response to a query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackage {
    /// Query the package was assembled for.
    pub target_query: String,
    /// Provenance shared by every evidence item in the package.
    pub snapshot: Snapshot,
    /// Ranked primary evidence items.
    pub primary_evidence: Vec<Evidence>,
    /// Structural edges observed among retrieved symbols.
    pub structural_relations: Vec<Relation>,
    /// Approximate token count of the serialized payload, for budgeting.
    pub estimated_tokens: usize,
    /// Capability guarantee held at assembly time.
    pub capability_level: CapabilityLevel,
}
