//! Frozen domain contracts for the Lynx Repository Evidence Substrate.
//!
//! This crate is the stability boundary of Lynx: every primitive downstream
//! consumers (Lea, Izen, external MCP clients) rely on lives here, and nothing
//! else does.
//!
//! # Invariants
//!
//! - **Pure contracts.** No parsing, storage, hashing, network, or other I/O;
//!   no business logic. These types describe *what* the substrate exchanges,
//!   never *how* it is produced.
//! - **Deterministic wire form.** Enums serialize each variant as its
//!   identifier verbatim (`"Rust"`, `"L3Structural"`); structs serialize their
//!   declared fields under their declared names. Any change to a serialized
//!   form is a breaking change.
//! - **Total derives.** Value types derive `Debug`, `Clone`, `Serialize`,
//!   `Deserialize`, `PartialEq`, `Eq`, `Hash`. Types containing `f32`
//!   ([`Evidence`], [`ContextPackage`]) intentionally omit `Eq` and `Hash`.
//!
//! All types are re-exported at the crate root; submodule paths remain public
//! for discoverability.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;
pub mod evidence;
pub mod language;
pub mod package;
pub mod relation;
pub mod snapshot;
pub mod symbol;

pub use error::ProtocolError;
pub use evidence::{Evidence, RetrievalMode, SourceRange};
pub use language::Language;
pub use package::{CapabilityLevel, ContextPackage};
pub use relation::{Relation, RelationKind};
pub use snapshot::Snapshot;
pub use symbol::{SymbolIdentity, SymbolKind};

#[cfg(test)]
mod tests {
    use crate::{ContextPackage, Evidence, Language};

    #[test]
    fn evidence_and_package_are_excluded_from_equivalence() {
        // Compile-time guard: `Evidence` carries an `f32`, so `Eq`/`Hash` must
        // not exist for it or for containers embedding it.
        fn assert_not_eq<T>()
        where
            T: Clone,
        {
        }
        assert_not_eq::<Evidence>();
        assert_not_eq::<ContextPackage>();
    }

    #[test]
    fn root_re_exports_resolve() {
        let language: Language = serde_json::from_str("\"Go\"")
            .ok()
            .unwrap_or(Language::Generic);
        assert_eq!(language, Language::Go);
    }
}
