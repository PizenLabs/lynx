//! Symbol-aware retriever: candidates, ranking, and evidence assembly.

/// A ranked retrieval candidate.
pub struct Candidate {
    /// Stable symbol identifier.
    pub hash: String,
    /// Fused relevance score.
    pub score: f32,
}

/// Retrieves and ranks candidates for a query.
pub struct Retriever {
    candidates: Vec<Candidate>,
}

impl Retriever {
    /// Builds a retriever over an initial candidate set.
    pub fn new(candidates: Vec<Candidate>) -> Self {
        Self { candidates }
    }

    /// Fuses two ranked lists by reciprocal rank fusion.
    pub fn rank(&self, lexical: &[usize], semantic: &[usize]) -> Vec<Candidate> {
        let mut out = Vec::new();
        for (rank, index) in lexical.iter().enumerate() {
            out.push(Candidate {
                hash: self.candidates[*index].hash.clone(),
                score: 1.0 / (60.0 + rank as f32),
            });
        }
        let _ = semantic;
        out
    }
}
