pub mod languages;
pub mod symbol_extraction;
pub mod tree_sitter;

use lynx_protocol::{CodeChunk, SymbolRecord};
use anyhow::Result;
use std::path::Path;

pub struct Parser {
    // We can add state here if needed, like a pool of tree-sitter parsers
}

impl Parser {
    pub fn new() -> Self {
        Self {}
    }

    pub fn parse_file(&self, path: &Path, content: &str) -> Result<(Vec<CodeChunk>, Vec<SymbolRecord>)> {
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        
        match extension {
            "rs" => self.parse_rust(path, content),
            "go" => self.parse_go(path, content),
            "ts" | "tsx" => self.parse_typescript(path, content),
            "js" | "jsx" => self.parse_javascript(path, content),
            "py" => self.parse_python(path, content),
            _ => {
                // For unsupported languages, we might want to do basic line-based chunking
                // or just return empty for now
                Ok((vec![], vec![]))
            }
        }
    }

    fn parse_rust(&self, path: &Path, content: &str) -> Result<(Vec<CodeChunk>, Vec<SymbolRecord>)> {
        symbol_extraction::rust::extract(path, content)
    }

    fn parse_go(&self, path: &Path, content: &str) -> Result<(Vec<CodeChunk>, Vec<SymbolRecord>)> {
        symbol_extraction::go::extract(path, content)
    }

    fn parse_typescript(&self, path: &Path, content: &str) -> Result<(Vec<CodeChunk>, Vec<SymbolRecord>)> {
        symbol_extraction::typescript::extract(path, content)
    }

    fn parse_javascript(&self, path: &Path, content: &str) -> Result<(Vec<CodeChunk>, Vec<SymbolRecord>)> {
        symbol_extraction::javascript::extract(path, content)
    }

    fn parse_python(&self, path: &Path, content: &str) -> Result<(Vec<CodeChunk>, Vec<SymbolRecord>)> {
        symbol_extraction::python::extract(path, content)
    }
}
