//! Atomic evidence primitives: coordinates, retrieval channels, and the
//! evidence item itself.

use serde::{Deserialize, Serialize};

use crate::error::ProtocolError;
use crate::snapshot::Snapshot;
use crate::symbol::SymbolIdentity;

/// Precise coordinate of a span within a source file.
///
/// Invariant: `end_line >= start_line` and `end_byte >= start_byte`.
/// Line numbers are inclusive and conventionally 1-based; byte offsets index
/// into the UTF-8 source text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SourceRange {
    /// First line of the span, inclusive.
    pub start_line: usize,
    /// Last line of the span, inclusive.
    pub end_line: usize,
    /// Byte offset of the first byte of the span.
    pub start_byte: usize,
    /// Byte offset just past the last byte of the span; `>= start_byte`.
    pub end_byte: usize,
}

impl SourceRange {
    /// Builds a range, enforcing the monotonic-endpoint invariant.
    ///
    /// Direct struct literal construction bypasses validation and remains
    /// available to producers reading trusted data back from storage.
    pub fn new(
        start_line: usize,
        end_line: usize,
        start_byte: usize,
        end_byte: usize,
    ) -> Result<Self, ProtocolError> {
        if end_line < start_line || end_byte < start_byte {
            return Err(ProtocolError::InvalidRange {
                start_line,
                end_line,
                start_byte,
                end_byte,
            });
        }
        Ok(Self {
            start_line,
            end_line,
            start_byte,
            end_byte,
        })
    }
}

/// Retrieval channel that produced an [`Evidence`] item.
///
/// Wire form: each variant serializes as its identifier verbatim.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RetrievalMode {
    /// Keyword match over an inverted index.
    Lexical,
    /// Embedding-similarity match.
    Semantic,
    /// Traversal over call-graph or dependency relations.
    Structural,
    /// Fused ranking across lexical, semantic, and structural channels.
    Hybrid,
}

/// Atomic evidence item delivered by Lynx: one symbol, located, scored, and
/// pinned to workspace provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// Entity this item attests to.
    pub identity: SymbolIdentity,
    /// Provenance of the workspace the excerpt was drawn from.
    pub snapshot: Snapshot,
    /// Location of [`Self::code_snippet`] within `identity.file_path`.
    pub range: SourceRange,
    /// Verbatim source excerpt covering [`Self::range`].
    pub code_snippet: String,
    /// Relevance score assigned by the retrieval channel.
    pub score: f32,
    /// Channel that produced this item.
    pub retrieval_mode: RetrievalMode,
}

#[cfg(test)]
mod tests {
    use super::{RetrievalMode, SourceRange};
    use crate::ProtocolError;

    #[test]
    fn accepts_monotonic_range() -> Result<(), ProtocolError> {
        let range = SourceRange::new(10, 42, 128, 4096)?;
        assert_eq!(range.start_line, 10);
        assert_eq!(range.end_byte, 4096);
        Ok(())
    }

    #[test]
    fn accepts_degenerate_single_point_range() -> Result<(), ProtocolError> {
        SourceRange::new(7, 7, 100, 100)?;
        Ok(())
    }

    #[test]
    fn rejects_regressing_lines() {
        let err = match SourceRange::new(5, 2, 0, 10) {
            Err(err) => err,
            Ok(_) => panic!("regressing lines accepted"),
        };
        assert!(
            matches!(err, ProtocolError::InvalidRange { .. }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn rejects_regressing_bytes() {
        let err = match SourceRange::new(1, 2, 50, 10) {
            Err(err) => err,
            Ok(_) => panic!("regressing bytes accepted"),
        };
        assert!(
            matches!(err, ProtocolError::InvalidRange { .. }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn retrieval_modes_survive_a_wire_roundtrip() -> Result<(), serde_json::Error> {
        for mode in [
            RetrievalMode::Lexical,
            RetrievalMode::Semantic,
            RetrievalMode::Structural,
            RetrievalMode::Hybrid,
        ] {
            let json = serde_json::to_string(&mode)?;
            let decoded: RetrievalMode = serde_json::from_str(&json)?;
            assert_eq!(decoded, mode);
        }
        Ok(())
    }
}
