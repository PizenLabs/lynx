//! Pins the serialized wire form of the frozen contract.
//!
//! Any diff against these goldens is a breaking change for every downstream
//! consumer and requires a deliberate protocol version bump.

use pizen_lynx_protocol::{
    CapabilityLevel, ContextPackage, Evidence, Language, Relation, RelationKind, RetrievalMode,
    Snapshot, SourceRange, SymbolIdentity, SymbolKind,
};
use std::path::PathBuf;

const EXPECTED_PACKAGE_JSON: &str = r#"{"target_query":"validate_session","snapshot":{"commit_hash":"9f3b1c4","workspace_root":"/tmp/demo-ws","is_dirty":false,"content_hash":"a5d8f0e2b9"},"primary_evidence":[{"identity":{"language":"Go","fqdn":"auth::service::AuthService::validate","kind":"Method","file_path":"internal/auth/service.go","content_hash":"deadbeef42"},"snapshot":{"commit_hash":"9f3b1c4","workspace_root":"/tmp/demo-ws","is_dirty":false,"content_hash":"a5d8f0e2b9"},"range":{"start_line":10,"end_line":42,"start_byte":128,"end_byte":4096},"code_snippet":"func (s *AuthService) validate(session string) bool","score":1.0,"retrieval_mode":"Hybrid"}],"structural_relations":[{"source_id":"auth::service::AuthService::validate","target_id":"auth::token::verify","kind":"Calls"}],"estimated_tokens":2048,"capability_level":"L2Symbolized"}"#;

fn sample_snapshot() -> Snapshot {
    Snapshot {
        commit_hash: Some("9f3b1c4".to_string()),
        workspace_root: PathBuf::from("/tmp/demo-ws"),
        is_dirty: false,
        content_hash: "a5d8f0e2b9".to_string(),
    }
}

fn sample_identity() -> Result<SymbolIdentity, pizen_lynx_protocol::ProtocolError> {
    SymbolIdentity::new(
        Language::Go,
        "auth::service::AuthService::validate",
        SymbolKind::Method,
        PathBuf::from("internal/auth/service.go"),
        "deadbeef42",
    )
}

#[test]
fn context_package_serializes_to_the_frozen_wire_form() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = sample_snapshot();
    let package = ContextPackage {
        target_query: "validate_session".to_string(),
        snapshot: snapshot.clone(),
        primary_evidence: vec![Evidence {
            identity: sample_identity()?,
            snapshot: snapshot.clone(),
            range: SourceRange::new(10, 42, 128, 4096)?,
            code_snippet: "func (s *AuthService) validate(session string) bool".to_string(),
            score: 1.0,
            retrieval_mode: RetrievalMode::Hybrid,
        }],
        structural_relations: vec![Relation {
            source_id: "auth::service::AuthService::validate".to_string(),
            target_id: "auth::token::verify".to_string(),
            kind: RelationKind::Calls,
        }],
        estimated_tokens: 2048,
        capability_level: CapabilityLevel::L2Symbolized,
    };

    let json = serde_json::to_string(&package)?;
    assert_eq!(json, EXPECTED_PACKAGE_JSON);

    let decoded: ContextPackage = serde_json::from_str(&json)?;
    assert_eq!(decoded.target_query, "validate_session");
    assert_eq!(decoded.snapshot, snapshot);
    assert_eq!(decoded.primary_evidence.len(), 1);
    assert_eq!(
        decoded.primary_evidence[0].retrieval_mode,
        RetrievalMode::Hybrid
    );
    assert_eq!(decoded.structural_relations.len(), 1);
    assert_eq!(decoded.estimated_tokens, 2048);
    assert_eq!(decoded.capability_level, CapabilityLevel::L2Symbolized);
    Ok(())
}

#[test]
fn every_enum_variant_serializes_verbatim() -> Result<(), Box<dyn std::error::Error>> {
    let kinds = [
        (SymbolKind::Function, "\"Function\""),
        (SymbolKind::Method, "\"Method\""),
        (SymbolKind::Struct, "\"Struct\""),
        (SymbolKind::Class, "\"Class\""),
        (SymbolKind::Trait, "\"Trait\""),
        (SymbolKind::Interface, "\"Interface\""),
        (SymbolKind::Enum, "\"Enum\""),
        (SymbolKind::Module, "\"Module\""),
        (SymbolKind::Variable, "\"Variable\""),
        (SymbolKind::Constant, "\"Constant\""),
    ];
    let relations = [
        (RelationKind::Calls, "\"Calls\""),
        (RelationKind::CalledBy, "\"CalledBy\""),
        (RelationKind::Implements, "\"Implements\""),
        (RelationKind::ImplementedBy, "\"ImplementedBy\""),
        (RelationKind::Imports, "\"Imports\""),
        (RelationKind::Contains, "\"Contains\""),
    ];
    let levels = [
        (CapabilityLevel::L0Text, "\"L0Text\""),
        (CapabilityLevel::L1Parsed, "\"L1Parsed\""),
        (CapabilityLevel::L2Symbolized, "\"L2Symbolized\""),
        (CapabilityLevel::L3Structural, "\"L3Structural\""),
    ];

    for (kind, expected) in kinds {
        assert_eq!(serde_json::to_string(&kind)?, expected);
    }
    for (kind, expected) in relations {
        assert_eq!(serde_json::to_string(&kind)?, expected);
        let decoded: RelationKind = serde_json::from_str(expected)?;
        assert_eq!(decoded, kind);
    }
    for (level, expected) in levels {
        assert_eq!(serde_json::to_string(&level)?, expected);
        let decoded: CapabilityLevel = serde_json::from_str(expected)?;
        assert_eq!(decoded, level);
    }
    Ok(())
}

#[test]
fn identities_and_ranges_are_hash_key_safe() -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashSet;

    let mut identities = HashSet::new();
    identities.insert(sample_identity()?);
    assert_eq!(identities.len(), 1);

    let mut ranges = HashSet::new();
    ranges.insert(SourceRange::new(1, 3, 0, 96)?);
    ranges.insert(SourceRange::new(1, 3, 0, 96)?);
    assert_eq!(ranges.len(), 1);
    Ok(())
}
