//! The Evidence Compiler: assembles a [`ContextPackage`] from ranked evidence
//! and structural relations under a strict token budget.
//!
//! Token estimation uses the approximation of **4 characters = 1 token**.
//! Slicing is symbol-aware and line-boundary preserving: evidence items are
//! truncated per-symbol (never merged across symbols), cutting only at whole
//! line boundaries so the retained excerpt remains structurally coherent.

use lynx_protocol::{CapabilityLevel, ContextPackage, Evidence, Relation, Snapshot};

/// Approximate characters consumed by a single token.
const CHARS_PER_TOKEN: usize = 4;

/// Assembles [`ContextPackage`] payloads under a strict token budget.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextCompiler {
    chars_per_token: usize,
}

impl ContextCompiler {
    /// Creates a compiler using the default 4-characters-per-token heuristic.
    pub fn new() -> Self {
        Self {
            chars_per_token: CHARS_PER_TOKEN,
        }
    }

    /// Approximate token count of `text`, `chars / 4`.
    pub fn estimate_tokens(&self, text: &str) -> usize {
        text.chars().count() / self.chars_per_token
    }

    /// Compiles a [`ContextPackage`] from ranked evidence, truncating each
    /// snippet at a line boundary so the serialized payload stays within
    /// `token_budget`.
    ///
    /// The envelope overhead (query, per-evidence identity, relations) is
    /// reserved first; the remaining budget is then distributed across
    /// snippets in rank order. `estimated_tokens` is set to the actual
    /// estimated total, which is guaranteed to be `<= token_budget`.
    pub fn compile(
        &self,
        target_query: String,
        snapshot: Snapshot,
        mut evidence: Vec<Evidence>,
        structural_relations: Vec<Relation>,
        capability_level: CapabilityLevel,
        token_budget: usize,
    ) -> ContextPackage {
        let envelope_tokens = self.envelope_tokens(&target_query, &evidence, &structural_relations);
        let snippet_token_budget = token_budget.saturating_sub(envelope_tokens);
        self.slice_to_budget(&mut evidence, snippet_token_budget);

        let estimated_tokens =
            self.estimated_package_tokens(&target_query, &evidence, &structural_relations);

        ContextPackage {
            target_query,
            snapshot,
            primary_evidence: evidence,
            structural_relations,
            estimated_tokens,
            capability_level,
        }
    }

    /// Truncates evidence snippets in place so their combined token estimate
    /// stays strictly within `token_budget`.
    fn slice_to_budget(&self, evidence: &mut [Evidence], token_budget: usize) {
        let mut budget = token_budget.saturating_mul(self.chars_per_token);

        for item in evidence.iter_mut() {
            if budget == 0 {
                item.code_snippet.clear();
                continue;
            }
            let estimated = item.code_snippet.chars().count();
            if estimated <= budget {
                budget -= estimated;
                continue;
            }
            // Greedy per-symbol line-boundary truncation, capped at the
            // remaining budget so the package never exceeds it.
            let sliced = self.slice_lines(&item.code_snippet, budget);
            let sliced_chars = sliced.chars().count();
            budget = budget.saturating_sub(sliced_chars);
            item.code_snippet = sliced;
            item.range.end_byte = item.range.start_byte + item.code_snippet.len();
            item.range.end_line =
                item.range.start_line + item.code_snippet.bytes().filter(|b| *b == b'\n').count();
        }
    }

    /// Returns the longest prefix of `text` composed of whole lines whose
    /// char count fits within `budget`, keeping at most `budget` characters
    /// when the first line alone overflows (never exceeding `budget`).
    fn slice_lines(&self, text: &str, budget: usize) -> String {
        let mut kept = String::new();
        let mut used = 0usize;
        for line in text.split_inclusive('\n') {
            let line_chars = line.chars().count();
            if used + line_chars > budget {
                break;
            }
            kept.push_str(line);
            used += line_chars;
        }
        if kept.is_empty() {
            // The first line alone exceeds the budget; retain a bounded
            // fragment of exactly `budget` characters rather than nothing.
            kept = text.chars().take(budget).collect();
        }
        kept
    }

    /// Approximate token estimate of the assembled package: the envelope
    /// (query, identities, relations) plus snippet content.
    fn estimated_package_tokens(
        &self,
        target_query: &str,
        evidence: &[Evidence],
        relations: &[Relation],
    ) -> usize {
        let snippet_tokens: usize = evidence
            .iter()
            .map(|item| self.estimate_tokens(&item.code_snippet))
            .sum();
        snippet_tokens + self.envelope_tokens(target_query, evidence, relations)
    }

    /// Token estimate of everything except snippet bodies: the target query,
    /// one token per evidence identity envelope, and one per relation line.
    fn envelope_tokens(
        &self,
        target_query: &str,
        evidence: &[Evidence],
        relations: &[Relation],
    ) -> usize {
        let identity_tokens: usize = evidence
            .iter()
            .map(|item| item.identity.fqdn.chars().count() / self.chars_per_token + 1)
            .sum();
        self.estimate_tokens(target_query) + identity_tokens + relations.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lynx_protocol::{Language, RetrievalMode, SourceRange, SymbolIdentity, SymbolKind};
    use std::path::PathBuf;

    fn identity(fqdn: &str) -> SymbolIdentity {
        SymbolIdentity::new(
            Language::Rust,
            fqdn,
            SymbolKind::Function,
            PathBuf::from("a.rs"),
            "h",
        )
        .unwrap()
    }

    fn evidence(fqdn: &str, snippet: &str) -> Evidence {
        Evidence {
            identity: identity(fqdn),
            snapshot: snapshot(),
            range: SourceRange::new(1, 1, 0, snippet.len()).unwrap(),
            code_snippet: snippet.to_string(),
            score: 1.0,
            retrieval_mode: RetrievalMode::Hybrid,
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            commit_hash: None,
            workspace_root: PathBuf::from("/ws"),
            is_dirty: false,
            content_hash: "abc".to_string(),
        }
    }

    #[test]
    fn estimate_four_chars_per_token() {
        let compiler = ContextCompiler::new();
        assert_eq!(compiler.estimate_tokens("abcdefgh"), 2);
        assert_eq!(compiler.estimate_tokens(""), 0);
    }

    #[test]
    fn small_package_kept_untouched() {
        let compiler = ContextCompiler::new();
        let items = vec![evidence("a", "fn a() {}"), evidence("b", "fn b() {}")];
        let package = compiler.compile(
            "q".to_string(),
            snapshot(),
            items,
            vec![],
            CapabilityLevel::L3Structural,
            1000,
        );
        assert_eq!(package.primary_evidence.len(), 2);
        assert_eq!(package.primary_evidence[0].code_snippet, "fn a() {}");
        assert!(package.estimated_tokens <= 1000);
    }

    #[test]
    fn large_snippets_are_truncated_to_budget() {
        let compiler = ContextCompiler::new();
        let big = "fn one() {\n    let x = 1;\n    let y = 2;\n}\n".repeat(40);
        let items = vec![evidence("a", &big)];
        let package = compiler.compile(
            "q".to_string(),
            snapshot(),
            items,
            vec![],
            CapabilityLevel::L3Structural,
            20,
        );
        assert!(package.estimated_tokens <= 20);
        assert!(package.primary_evidence[0].code_snippet.chars().count() <= 20 * 4);
    }

    #[test]
    fn later_evidence_dropped_when_budget_exhausted() {
        let compiler = ContextCompiler::new();
        let big = "fn x() {\n    body;\n}\n".repeat(20);
        let mut items = vec![evidence("first", &big), evidence("second", &big)];
        compiler.slice_to_budget(&mut items, 12);
        // Combined snippets stay strictly within the token budget.
        let total: usize = items.iter().map(|i| i.code_snippet.chars().count()).sum();
        assert!(total <= 12 * 4);
        // Higher-ranked item is prioritized over the lower-ranked one.
        assert!(items[0].code_snippet.chars().count() >= items[1].code_snippet.chars().count());
    }
}
