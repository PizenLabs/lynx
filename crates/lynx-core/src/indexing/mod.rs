use anyhow::Result;
use lynx_parser::Parser;
use lynx_storage::Storage;
use std::path::Path;
use walkdir::WalkDir;

pub struct Indexer<'a> {
    parser: &'a Parser,
    storage: &'a Storage,
}

impl<'a> Indexer<'a> {
    pub fn new(parser: &'a Parser, storage: &'a Storage) -> Self {
        Self { parser, storage }
    }

    pub async fn index_repository(&mut self, repo_path: &Path) -> Result<()> {
        for entry in WalkDir::new(repo_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if self.should_skip(path) {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue, // Skip binary or unreadable files
            };

            let (chunks, symbols) = self.parser.parse_file(path, &content)?;
            
            if !chunks.is_empty() {
                self.storage.index_chunks(&chunks)?;
            }
            if !symbols.is_empty() {
                self.storage.index_symbols(&symbols)?;
            }
        }
        Ok(())
    }

    fn should_skip(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        path_str.contains("/.git/") || 
        path_str.contains("/node_modules/") || 
        path_str.contains("/vendor/") ||
        path_str.contains("/target/") ||
        path_str.contains("/build/") ||
        path_str.contains("/dist/")
    }
}
