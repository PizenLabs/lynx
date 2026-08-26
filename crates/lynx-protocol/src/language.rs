//! Language identification primitive.

use serde::{Deserialize, Serialize};

/// Languages recognized by the Lynx substrate.
///
/// Wire form: each variant serializes as its identifier verbatim
/// (e.g. `"Rust"`, `"TypeScript"`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Language {
    /// Rust sources (`.rs`).
    Rust,
    /// Go sources (`.go`).
    Go,
    /// TypeScript sources (`.ts`, `.tsx`).
    TypeScript,
    /// JavaScript sources (`.js`, `.jsx`, `.mjs`, `.cjs`).
    JavaScript,
    /// Python sources (`.py`).
    Python,
    /// Markdown documentation (`.md`, `.mdx`).
    Markdown,
    /// YAML manifests and configuration (`.yaml`, `.yml`).
    Yaml,
    /// JSON documents (`.json`).
    Json,
    /// TOML manifests and configuration (`.toml`).
    Toml,
    /// Any unrecognized format; handled as raw text.
    Generic,
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn roundtrips_every_variant() -> Result<(), serde_json::Error> {
        let all = [
            Language::Rust,
            Language::Go,
            Language::TypeScript,
            Language::JavaScript,
            Language::Python,
            Language::Markdown,
            Language::Yaml,
            Language::Json,
            Language::Toml,
            Language::Generic,
        ];
        for language in all {
            let json = serde_json::to_string(&language)?;
            let expected = match language {
                Language::Rust => "\"Rust\"",
                Language::Go => "\"Go\"",
                Language::TypeScript => "\"TypeScript\"",
                Language::JavaScript => "\"JavaScript\"",
                Language::Python => "\"Python\"",
                Language::Markdown => "\"Markdown\"",
                Language::Yaml => "\"Yaml\"",
                Language::Json => "\"Json\"",
                Language::Toml => "\"Toml\"",
                Language::Generic => "\"Generic\"",
            };
            assert_eq!(json, expected);
            let decoded: Language = serde_json::from_str(&json)?;
            assert_eq!(decoded, language);
        }
        Ok(())
    }
}
