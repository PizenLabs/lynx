//! Language adapter pipeline producing frozen protocol primitives.
//!
//! Every source is routed by file extension to a [`LanguageAdapter`]:
//!
//! - `.rs` → [`RustAdapter`]
//! - `.go` → [`GoAdapter`]
//! - `.ts`/`.tsx` → [`TypeScriptAdapter`]
//! - `.js`/`.jsx`/`.mjs`/`.cjs` → [`JavaScriptAdapter`]
//! - `.py` → [`PythonAdapter`]
//! - `.md`/`.mdx` → [`GenericAdapter`] reporting [`Language::Markdown`]
//! - `.yaml`/`.yml` → [`GenericAdapter`] reporting [`Language::Yaml`]
//! - `.json` → [`GenericAdapter`] reporting [`Language::Json`]
//! - `.toml` → [`GenericAdapter`] reporting [`Language::Toml`]
//! - anything else → [`GenericAdapter`] reporting [`Language::Generic`]
//!
//! Grammar-backed adapters run at [`CapabilityLevel::L3Structural`] (symbols
//! and intra-file relations); grammar-less generic adapters degrade to
//! [`CapabilityLevel::L1Parsed`] and never emit relations.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod adapter;
pub mod error;

use std::path::Path;

use lynx_protocol::{Language, Relation, SymbolIdentity};

pub use adapter::{
    GenericAdapter, GoAdapter, JavaScriptAdapter, LanguageAdapter, PythonAdapter, RustAdapter,
    TypeScriptAdapter,
};
pub use error::ParseError;

/// Maps a source path to the [`LanguageAdapter`] that owns its format.
///
/// Dispatch is deterministic by extension; unrecognized formats fall back to
/// [`GenericAdapter`] at [`Language::Generic`].
pub fn adapter_for_path(path: &Path) -> Box<dyn LanguageAdapter> {
    let language = language_for_path(path);
    match language {
        Language::Rust => Box::new(RustAdapter),
        Language::Go => Box::new(GoAdapter),
        Language::TypeScript => Box::new(TypeScriptAdapter),
        Language::JavaScript => Box::new(JavaScriptAdapter),
        Language::Python => Box::new(PythonAdapter),
        other => Box::new(GenericAdapter::new(other)),
    }
}

/// Resolves the protocol [`Language`] for a path by its file extension.
pub fn language_for_path(path: &Path) -> Language {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("rs") => Language::Rust,
        Some("go") => Language::Go,
        Some("ts" | "tsx") => Language::TypeScript,
        Some("js" | "jsx" | "mjs" | "cjs") => Language::JavaScript,
        Some("py") => Language::Python,
        Some("md" | "mdx") => Language::Markdown,
        Some("yaml" | "yml") => Language::Yaml,
        Some("json") => Language::Json,
        Some("toml") => Language::Toml,
        _ => Language::Generic,
    }
}

/// Facade over the adapter pipeline: parses a file into protocol primitives.
#[derive(Debug, Default)]
pub struct Parser;

impl Parser {
    /// Creates a parser.
    pub fn new() -> Self {
        Self
    }

    /// Extracts [`SymbolIdentity`] records declared by `source` at `path`.
    ///
    /// The adapter is chosen by extension; every format yields at least the
    /// file-level module symbol.
    pub fn parse_symbols(
        &self,
        path: &Path,
        source: &str,
    ) -> Result<Vec<SymbolIdentity>, ParseError> {
        adapter_for_path(path).parse_symbols(source, path)
    }

    /// Extracts symbols and the intra-file relations among them in one pass.
    ///
    /// Grammar-backed languages report both; generic formats report symbols
    /// with an empty relation set.
    pub fn parse(
        &self,
        path: &Path,
        source: &str,
    ) -> Result<(Vec<SymbolIdentity>, Vec<Relation>), ParseError> {
        let adapter = adapter_for_path(path);
        let symbols = adapter.parse_symbols(source, path)?;
        let relations = match adapter.new_tree(source) {
            Ok(Some(tree)) => adapter.extract_relations(source, &tree, &symbols)?,
            _ => Vec::new(),
        };
        Ok((symbols, relations))
    }
}

#[cfg(test)]
mod tests {
    use super::{language_for_path, Parser};
    use lynx_protocol::{Language, RelationKind, SymbolKind};

    #[test]
    fn routes_extensions_to_languages() {
        assert_eq!(language_for_path("src/lib.rs".as_ref()), Language::Rust);
        assert_eq!(language_for_path("cmd/app.go".as_ref()), Language::Go);
        assert_eq!(language_for_path("ui.tsx".as_ref()), Language::TypeScript);
        assert_eq!(language_for_path("app.mjs".as_ref()), Language::JavaScript);
        assert_eq!(language_for_path("module.py".as_ref()), Language::Python);
        assert_eq!(language_for_path("README.md".as_ref()), Language::Markdown);
        assert_eq!(language_for_path("ci.yml".as_ref()), Language::Yaml);
        assert_eq!(language_for_path("data.json".as_ref()), Language::Json);
        assert_eq!(language_for_path("Cargo.toml".as_ref()), Language::Toml);
        assert_eq!(language_for_path("LICENSE".as_ref()), Language::Generic);
    }

    #[test]
    fn parses_go_symbols_and_relations() {
        let source = r#"
package auth

type User struct {
    Name string
}

func (u *User) Name() string { return u.Name }

func Validate(u *User) bool { return u.Name() != "" }
"#;
        let parser = Parser::new();
        let (symbols, relations) = parser.parse("auth/user.go".as_ref(), source).unwrap();

        assert!(symbols.iter().any(|s| s.fqdn == "auth::User"));
        assert!(symbols.iter().any(|s| s.fqdn == "auth::User::Name"));
        assert!(symbols.iter().any(|s| s.fqdn == "auth::Validate"));

        // The module pseudo-symbol leads every identity set.
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Module));
        // Validate calls Name, so a Calls edge connects their hashes.
        assert!(relations
            .iter()
            .any(|r| r.kind == RelationKind::Calls));
    }

    #[test]
    fn parses_rust_methods_as_methods() {
        let source = r#"
struct Greeter;

impl Greeter {
    fn greet(&self) -> String { String::from("hi") }
}

fn entry() { let g = Greeter; }
"#;
        let parser = Parser::new();
        let (symbols, _relations) = parser.parse("src/main.rs".as_ref(), source).unwrap();

        assert!(symbols
            .iter()
            .any(|s| s.fqdn == "main::Greeter::greet" && s.kind == SymbolKind::Method));
        assert!(symbols.iter().any(|s| s.fqdn == "main::entry"));
    }

    #[test]
    fn parses_python_classes_with_dotted_fqdn() {
        let source = r#"
class Service:
    def run(self):
        return 1
"#;
        let parser = Parser::new();
        let (symbols, _relations) = parser.parse("service.py".as_ref(), source).unwrap();

        assert!(symbols
            .iter()
            .any(|s| s.fqdn == "service::Service.run" && s.kind == SymbolKind::Method));
        assert!(symbols.iter().any(|s| s.fqdn == "service::Service"));
    }

    #[test]
    fn generic_adapter_returns_empty_relations() {
        let source = "name: demo\nversion: 1\n";
        let parser = Parser::new();
        let (symbols, relations) = parser.parse("demo.yaml".as_ref(), source).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Module);
        assert!(relations.is_empty());
    }
}
