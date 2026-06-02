use anyhow::Result;
use lynx_protocol::{CodeChunk, SymbolRecord};
use std::path::Path;
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};
use tree_sitter_javascript::LANGUAGE;

pub fn extract(path: &Path, content: &str) -> Result<(Vec<CodeChunk>, Vec<SymbolRecord>)> {
    let mut parser = Parser::new();
    parser.set_language(&LANGUAGE.into())?;

    let tree = parser
        .parse(content, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse JavaScript file"))?;
    let root_node = tree.root_node();

    let mut chunks = Vec::new();
    let mut symbols = Vec::new();

    let query_str = r#"
        (function_declaration name: (identifier) @func_name) @func
        (class_declaration name: (identifier) @class_name) @class
        (method_definition name: (property_identifier) @method_name) @method
    "#;

    let query = Query::new(&LANGUAGE.into(), query_str)?;
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&query, root_node, content.as_bytes());

    while let Some(&(ref mat, capture_index)) = captures.next() {
        let capture = mat.captures[capture_index];
        let capture_name = query.capture_names()[capture.index as usize];

        if !["func", "class", "method"].contains(&capture_name) {
            continue;
        }

        let node = capture.node;
        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        let raw_content = node.utf8_text(content.as_bytes())?.to_string();

        let symbol_name = match resolve_symbol_name(mat, node, &query, content.as_bytes()) {
            Some(name) => name,
            None => continue,
        };

        let file_path = path.to_string_lossy().replace('\\', "/");
        let symbol_id = format!("{}:{}:{}", capture_name, file_path, symbol_name);

        symbols.push(SymbolRecord {
            symbol_id: symbol_id.clone(),
            symbol_name: symbol_name.clone(),
            file_path: file_path.clone(),
            start_line,
            end_line,
        });

        chunks.push(CodeChunk {
            id: blake3::hash(raw_content.as_bytes()).to_string(),
            file_path: file_path.clone(),
            start_line,
            end_line,
            raw_content,
            symbols_defined: vec![symbol_id],
        });
    }

    Ok((chunks, symbols))
}

fn resolve_symbol_name(
    mat: &tree_sitter::QueryMatch,
    node: tree_sitter::Node,
    query: &Query,
    content: &[u8],
) -> Option<String> {
    if let Some(capture) = mat.captures.iter().find(|c| {
        let name = query.capture_names()[c.index as usize];
        name.ends_with("_name")
    }) {
        if let Ok(text) = capture.node.utf8_text(content) {
            return Some(text.to_string());
        }
    }

    if let Some(name_node) = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("type"))
    {
        if let Ok(text) = name_node.utf8_text(content) {
            return Some(text.to_string());
        }
    }

    find_identifier_in_node(node, content)
}

fn find_identifier_in_node(node: tree_sitter::Node, content: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "type_identifier" | "field_identifier" | "property_identifier"
        ) {
            if let Ok(text) = child.utf8_text(content) {
                return Some(text.to_string());
            }
        }
        if let Some(name) = find_identifier_in_node(child, content) {
            return Some(name);
        }
    }
    None
}
