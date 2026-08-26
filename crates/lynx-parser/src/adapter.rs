//! [`LanguageAdapter`] implementations producing frozen protocol primitives.
//!
//! Every adapter either owns a tree-sitter grammar and runs at
//! [`CapabilityLevel::L3Structural`] (symbols **and** intra-file relations), or
//! it is grammar-less like [`GenericAdapter`] and degrades gracefully to
//! [`CapabilityLevel::L1Parsed`].
//!
//! All relations are strictly intra-file: an edge is emitted only when both
//! endpoints were extracted from the same `source`, and endpoint ids are the
//! two symbols' `content_hash` values.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use lynx_protocol::{
    CapabilityLevel, Language, Relation, RelationKind, SymbolIdentity, SymbolKind,
};
use tree_sitter::{Node, Query, QueryCursor, StreamingIterator, Tree};

use crate::error::ParseError;

// ---------------------------------------------------------------------------
// Adapter contract
// ---------------------------------------------------------------------------

/// Per-language parsing strategy yielding frozen protocol primitives.
///
/// # Capability degradation contract
///
/// Grammar-backed adapters ([`RustAdapter`], [`GoAdapter`],
/// [`TypeScriptAdapter`], [`JavaScriptAdapter`], [`PythonAdapter`]) report
/// [`CapabilityLevel::L3Structural`] and expose `new_tree`, letting the facade
/// build a syntax tree without downcasting.
///
/// [`GenericAdapter`] reports [`CapabilityLevel::L1Parsed`] and owns NO
/// tree-sitter grammar. Its `new_tree` always yields `None`; its
/// `extract_relations` ALWAYS returns `Ok(vec![])` and never dereferences the
/// passed tree; correspondingly the facade never invokes `extract_relations`
/// when an adapter's `new_tree` reports `None`.
pub trait LanguageAdapter: Send + Sync {
    /// Language reported for everything this adapter produces.
    fn language(&self) -> Language;

    /// Maximum guarantee this adapter currently makes about its output.
    fn capability_level(&self) -> CapabilityLevel;

    /// Extracts symbol identities declared by `source` at `file_path`.
    fn parse_symbols(
        &self,
        source: &str,
        file_path: &Path,
    ) -> Result<Vec<SymbolIdentity>, ParseError>;

    /// Parses `source` into a syntax tree, or `Ok(None)` for adapters that
    /// own no tree-sitter grammar.
    ///
    /// The default yields `Ok(None)`; grammar-backed adapters override it. The
    /// facade uses this to decide whether `extract_relations` may run.
    fn new_tree(&self, _source: &str) -> Result<Option<Tree>, ParseError> {
        Ok(None)
    }

    /// Extracts intra-file relations among `symbols` from the parsed `tree`.
    fn extract_relations(
        &self,
        source: &str,
        tree: &Tree,
        symbols: &[SymbolIdentity],
    ) -> Result<Vec<Relation>, ParseError>;
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Derives the normalized module prefix for `path`: extension stripped,
/// `/` folded to `::`, and a leading `src/` component dropped
/// (`src/auth/service.rs` -> `auth::service`).
fn module_prefix(path: &Path) -> String {
    let flattened = path.with_extension("").to_string_lossy().replace('\\', "/");
    let mut parts: Vec<&str> = flattened
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.first() == Some(&"src") {
        parts.remove(0);
    }
    if parts.is_empty() {
        "module".to_string()
    } else {
        parts.join("::")
    }
}

/// Hex blake3 digest of `text`: the deterministic symbol `content_hash`.
fn hash_text(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_string()
}

/// Trailing identifier segment of an fqdn (`a::b.C.d` -> `d`).
fn trailing_segment(fqdn: &str) -> &str {
    match fqdn.rfind([':', '.']) {
        Some(index) => &fqdn[index + 1..],
        None => fqdn,
    }
}

/// Trailing identifier run of arbitrary expression text
/// (`path::to::<T>::func()` -> `func`, `obj.method()` -> `method`).
fn trailing_identifier(text: &str) -> Option<String> {
    let reversed: String = text
        .chars()
        .rev()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect();
    let ident: String = reversed.chars().rev().collect();
    if ident.is_empty() {
        None
    } else {
        Some(ident)
    }
}

/// Returns the lazily compiled query cached in `cell`.
///
/// `tree_sitter::QueryError` is not `Clone`, so a failed compile is cached as
/// its message string instead of the error itself.
fn cached_query(
    cell: &'static OnceLock<Result<Query, String>>,
    grammar: tree_sitter::Language,
    source: &'static str,
) -> Result<&'static Query, ParseError> {
    cell.get_or_init(|| Query::new(&grammar, source).map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|message| ParseError::TreeSitter(message.clone()))
}

fn capture_node<'query, 'tree>(
    query: &Query,
    mat: &tree_sitter::QueryMatch<'query, 'tree>,
    name: &str,
) -> Option<Node<'tree>> {
    let names = query.capture_names();
    mat.captures
        .iter()
        .find(|capture| names.get(capture.index as usize) == Some(&name))
        .map(|capture| capture.node)
}

fn capture_text<'text>(
    query: &Query,
    mat: &tree_sitter::QueryMatch<'_, '_>,
    name: &str,
    text: &'text [u8],
) -> Option<&'text str> {
    capture_node(query, mat, name).and_then(|node| node.utf8_text(text).ok())
}

fn first_descendant_of<'node>(node: Node<'node>, kind: &str) -> Option<Node<'node>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if let Some(found) = first_descendant_of(child, kind) {
                return Some(found);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

/// A symbol extracted from a syntax tree before protocol freezing.
struct RawSymbol {
    kind: SymbolKind,
    fqdn: String,
    start_byte: usize,
    end_byte: usize,
    hash: String,
}

/// An observed call expression awaiting resolution against extracted symbols.
struct CallSite {
    start_byte: usize,
    callee: String,
}

/// Names the container a symbol should be linked to via [`RelationKind::Contains`].
enum ContainerKey {
    /// The file-level module pseudo-symbol every adapter emits.
    Module,
    /// A named type declared in this file (struct/class/enum/interface).
    Type(String),
    /// A trait declared in this file.
    Trait(String),
    /// A declared module (Rust `mod` item).
    Mod(String),
}

fn push_raw(
    raws: &mut Vec<RawSymbol>,
    requests: &mut Vec<(ContainerKey, usize)>,
    kind: SymbolKind,
    fqdn: String,
    node: Node,
    text: &[u8],
    container: Option<ContainerKey>,
) {
    let Ok(slice) = node.utf8_text(text) else {
        return;
    };
    let index = raws.len();
    raws.push(RawSymbol {
        kind,
        fqdn,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        hash: hash_text(slice),
    });
    if let Some(key) = container {
        requests.push((key, index));
    }
}

fn find_raw(prefix: &str, raws: &[RawSymbol], name: &str, kinds: &[SymbolKind]) -> Option<usize> {
    let fqdn = format!("{prefix}::{name}");
    raws.iter()
        .position(|raw| raw.fqdn == fqdn && kinds.contains(&raw.kind))
}

/// Freezes raw symbols into protocol identities, led by the module pseudo-symbol.
fn symbol_identities(
    language: Language,
    file_path: &Path,
    source: &str,
    raws: &[RawSymbol],
) -> Vec<SymbolIdentity> {
    let mut symbols = Vec::with_capacity(raws.len() + 1);
    symbols.push(SymbolIdentity {
        language,
        fqdn: module_prefix(file_path),
        kind: SymbolKind::Module,
        file_path: file_path.to_path_buf(),
        content_hash: hash_text(source),
    });
    symbols.extend(raws.iter().map(|raw| SymbolIdentity {
        language,
        fqdn: raw.fqdn.clone(),
        kind: raw.kind,
        file_path: file_path.to_path_buf(),
        content_hash: raw.hash.clone(),
    }));
    symbols
}

fn contains_edge(container_hash: &str, member_hash: &str) -> Relation {
    Relation {
        source_id: container_hash.to_string(),
        target_id: member_hash.to_string(),
        kind: RelationKind::Contains,
    }
}

fn implements_edge(type_hash: &str, contract_hash: &str) -> Relation {
    Relation {
        source_id: type_hash.to_string(),
        target_id: contract_hash.to_string(),
        kind: RelationKind::Implements,
    }
}

fn sort_and_dedup(mut relations: Vec<Relation>) -> Vec<Relation> {
    relations.sort_by(|a, b| {
        (kind_rank(a.kind), &a.source_id, &a.target_id).cmp(&(
            kind_rank(b.kind),
            &b.source_id,
            &b.target_id,
        ))
    });
    relations.dedup_by(|a, b| {
        a.kind == b.kind && a.source_id == b.source_id && a.target_id == b.target_id
    });
    relations
}

/// Deterministic ordinal for a relation kind, so edges sort consistently.
fn kind_rank(kind: RelationKind) -> u8 {
    match kind {
        RelationKind::Calls => 0,
        RelationKind::CalledBy => 1,
        RelationKind::Implements => 2,
        RelationKind::ImplementedBy => 3,
        RelationKind::Imports => 4,
        RelationKind::Contains => 5,
    }
}

/// Resolves container requests into `Contains` edges.
///
/// Unresolvable containers (e.g. an impl block whose type lives in another
/// file) are silently dropped: only intra-file structure is reported.
fn resolve_contains(
    prefix: &str,
    raws: &[RawSymbol],
    module_hash: &str,
    requests: Vec<(ContainerKey, usize)>,
) -> Vec<Relation> {
    let mut relations = Vec::new();
    for (key, member_index) in requests {
        let container_hash = match &key {
            ContainerKey::Module => Some(module_hash.to_string()),
            ContainerKey::Type(name) => find_raw(prefix, raws, name, &[SymbolKind::Class])
                .or_else(|| find_raw(prefix, raws, name, &[SymbolKind::Struct]))
                .or_else(|| find_raw(prefix, raws, name, &[SymbolKind::Interface]))
                .map(|index| raws[index].hash.clone()),
            ContainerKey::Trait(name) => find_raw(prefix, raws, name, &[SymbolKind::Trait])
                .map(|index| raws[index].hash.clone()),
            ContainerKey::Mod(name) => find_raw(prefix, raws, name, &[SymbolKind::Module])
                .map(|index| raws[index].hash.clone()),
        };
        if let Some(hash) = container_hash {
            relations.push(contains_edge(&hash, &raws[member_index].hash));
        }
    }
    sort_and_dedup(relations)
}

/// Resolves call sites into `Calls` edges.
///
/// The callee's trailing identifier segment must match the trailing segment of
/// some Function/Method fqdn in the same file; the edge source is the
/// innermost enclosing function or method. Trivial self-recursion is skipped.
fn resolve_calls(raws: &[RawSymbol], sites: &[CallSite]) -> Vec<Relation> {
    let mut callable: HashMap<&str, usize> = HashMap::new();
    for (index, raw) in raws.iter().enumerate() {
        if matches!(raw.kind, SymbolKind::Function | SymbolKind::Method) {
            callable.entry(trailing_segment(&raw.fqdn)).or_insert(index);
        }
    }
    let mut relations = Vec::new();
    for site in sites {
        let Some(target_index) = callable.get(site.callee.as_str()).copied() else {
            continue;
        };
        let Some(source_index) = smallest_enclosing_function(raws, site.start_byte) else {
            continue;
        };
        if source_index == target_index {
            continue;
        }
        relations.push(Relation {
            source_id: raws[source_index].hash.clone(),
            target_id: raws[target_index].hash.clone(),
            kind: RelationKind::Calls,
        });
    }
    sort_and_dedup(relations)
}

fn smallest_enclosing_function(raws: &[RawSymbol], offset: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, raw) in raws.iter().enumerate() {
        if !matches!(raw.kind, SymbolKind::Function | SymbolKind::Method) {
            continue;
        }
        if raw.start_byte <= offset && offset < raw.end_byte {
            let tighter = match best {
                None => true,
                Some(current) => {
                    raw.end_byte - raw.start_byte
                        < raws[current].end_byte - raws[current].start_byte
                }
            };
            if tighter {
                best = Some(index);
            }
        }
    }
    best
}

/// Collects call expressions of `call_kind` anywhere under `node`.
fn collect_call_sites(node: Node, text: &[u8], call_kind: &str, sites: &mut Vec<CallSite>) {
    if node.kind() == call_kind {
        if let Some(function_node) = node.child_by_field_name("function") {
            if let Ok(expression_text) = function_node.utf8_text(text) {
                if let Some(callee) = trailing_identifier(expression_text) {
                    sites.push(CallSite {
                        start_byte: node.start_byte(),
                        callee,
                    });
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_call_sites(child, text, call_kind, sites);
    }
}

// ---------------------------------------------------------------------------
// RustAdapter
// ---------------------------------------------------------------------------

const RUST_SYMBOL_QUERY_SRC: &str = r#"
    (function_item name: (identifier) @func_name) @func_node
    (struct_item name: (type_identifier) @struct_name) @struct_node
    (enum_item name: (type_identifier) @enum_name) @enum_node
    (trait_item name: (type_identifier) @trait_name) @trait_node
    (mod_item name: (identifier) @mod_name) @mod_node
"#;

const RUST_IMPL_QUERY_SRC: &str = r#"
    (impl_item trait: (_) @impl_trait type: (_) @impl_type)
"#;

fn rust_symbol_query() -> Result<&'static Query, ParseError> {
    static CELL: OnceLock<Result<Query, String>> = OnceLock::new();
    cached_query(
        &CELL,
        tree_sitter_rust::LANGUAGE.into(),
        RUST_SYMBOL_QUERY_SRC,
    )
}

fn rust_impl_query() -> Result<&'static Query, ParseError> {
    static CELL: OnceLock<Result<Query, String>> = OnceLock::new();
    cached_query(
        &CELL,
        tree_sitter_rust::LANGUAGE.into(),
        RUST_IMPL_QUERY_SRC,
    )
}

fn rust_tree(source: &str) -> Result<Tree, ParseError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|error| ParseError::Grammar(error.to_string()))?;
    parser
        .parse(source, None)
        .ok_or(ParseError::InvalidSource("Rust"))
}

/// Names the lexical container governing a Rust item's fqdn and ownership.
enum RustContainer {
    /// Method inside an `impl Type` block.
    ImplType { name: String },
    /// Default method inside a `trait` definition.
    Trait { name: String },
    /// Item inside a named `mod` block.
    Mod { name: String },
    /// Function nested inside another function.
    NestedFunction,
    /// Top-level item owned by the file module.
    None,
}

fn rust_container(node: Node, text: &[u8]) -> RustContainer {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "impl_item" => {
                if let Some(name) = parent
                    .child_by_field_name("type")
                    .and_then(|type_node| rust_type_name(type_node, text))
                {
                    return RustContainer::ImplType { name };
                }
            }
            "trait_item" | "mod_item" => {
                if let Some(name_node) = parent.child_by_field_name("name") {
                    if let Ok(name) = name_node.utf8_text(text) {
                        let name = name.to_string();
                        return if parent.kind() == "trait_item" {
                            RustContainer::Trait { name }
                        } else {
                            RustContainer::Mod { name }
                        };
                    }
                }
            }
            "function_item" => return RustContainer::NestedFunction,
            _ => {}
        }
        current = parent.parent();
    }
    RustContainer::None
}

fn rust_type_name(node: Node, text: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" | "identifier" => node.utf8_text(text).ok().map(str::to_string),
        "generic_type" | "pointer_type" | "reference_type" => node
            .child_by_field_name("type")
            .and_then(|inner| rust_type_name(inner, text)),
        "scoped_type_identifier" => node
            .child_by_field_name("name")
            .and_then(|name| name.utf8_text(text).ok().map(str::to_string)),
        _ => {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if let Some(name) = rust_type_name(child, text) {
                        return Some(name);
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
            None
        }
    }
}

struct RustAnalysis {
    raws: Vec<RawSymbol>,
    container_requests: Vec<(ContainerKey, usize)>,
    calls: Vec<CallSite>,
}

fn analyze_rust(tree: &Tree, source: &str, path: &Path) -> Result<RustAnalysis, ParseError> {
    let prefix = module_prefix(path);
    let text = source.as_bytes();
    let root = tree.root_node();
    let query = rust_symbol_query()?;

    let mut raws: Vec<RawSymbol> = Vec::new();
    let mut container_requests: Vec<(ContainerKey, usize)> = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, text);
    while let Some(mat) = matches.next() {
        if let Some(node) = capture_node(query, mat, "func_node") {
            let Some(name) = capture_text(query, mat, "func_name", text) else {
                continue;
            };
            let (kind, fqdn, container) = match rust_container(node, text) {
                RustContainer::ImplType { name: type_name } => (
                    SymbolKind::Method,
                    format!("{prefix}::{type_name}::{name}"),
                    Some(ContainerKey::Type(type_name)),
                ),
                RustContainer::Trait { name: trait_name } => (
                    SymbolKind::Method,
                    format!("{prefix}::{trait_name}::{name}"),
                    Some(ContainerKey::Trait(trait_name)),
                ),
                RustContainer::Mod { name: mod_name } => (
                    SymbolKind::Function,
                    format!("{prefix}::{name}"),
                    Some(ContainerKey::Mod(mod_name)),
                ),
                RustContainer::NestedFunction => {
                    (SymbolKind::Function, format!("{prefix}::{name}"), None)
                }
                RustContainer::None => (
                    SymbolKind::Function,
                    format!("{prefix}::{name}"),
                    Some(ContainerKey::Module),
                ),
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                kind,
                fqdn,
                node,
                text,
                container,
            );
        } else if let Some(node) = capture_node(query, mat, "struct_node") {
            let Some(name) = capture_text(query, mat, "struct_name", text) else {
                continue;
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                SymbolKind::Struct,
                format!("{prefix}::{name}"),
                node,
                text,
                Some(ContainerKey::Module),
            );
        } else if let Some(node) = capture_node(query, mat, "enum_node") {
            let Some(name) = capture_text(query, mat, "enum_name", text) else {
                continue;
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                SymbolKind::Enum,
                format!("{prefix}::{name}"),
                node,
                text,
                Some(ContainerKey::Module),
            );
        } else if let Some(node) = capture_node(query, mat, "trait_node") {
            let Some(name) = capture_text(query, mat, "trait_name", text) else {
                continue;
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                SymbolKind::Trait,
                format!("{prefix}::{name}"),
                node,
                text,
                Some(ContainerKey::Module),
            );
        } else if let Some(node) = capture_node(query, mat, "mod_node") {
            let Some(name) = capture_text(query, mat, "mod_name", text) else {
                continue;
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                SymbolKind::Module,
                format!("{prefix}::{name}"),
                node,
                text,
                Some(ContainerKey::Module),
            );
        }
    }

    let mut calls = Vec::new();
    collect_call_sites(root, text, "call_expression", &mut calls);

    Ok(RustAnalysis {
        raws,
        container_requests,
        calls,
    })
}

fn rust_implements(
    tree: &Tree,
    source: &str,
    path: &Path,
    raws: &[RawSymbol],
) -> Result<Vec<Relation>, ParseError> {
    let prefix = module_prefix(path);
    let text = source.as_bytes();
    let query = rust_impl_query()?;
    let mut relations = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text);
    while let Some(mat) = matches.next() {
        let Some(trait_text) = capture_text(query, mat, "impl_trait", text) else {
            continue;
        };
        let Some(trait_name) = trailing_identifier(trait_text) else {
            continue;
        };
        let Some(type_node) = capture_node(query, mat, "impl_type") else {
            continue;
        };
        let Some(type_name) = rust_type_name(type_node, text) else {
            continue;
        };
        let type_index = find_raw(
            &prefix,
            raws,
            &type_name,
            &[SymbolKind::Struct, SymbolKind::Enum],
        );
        let trait_index = find_raw(&prefix, raws, &trait_name, &[SymbolKind::Trait]);
        if let (Some(type_index), Some(trait_index)) = (type_index, trait_index) {
            relations.push(implements_edge(
                &raws[type_index].hash,
                &raws[trait_index].hash,
            ));
        }
    }
    Ok(sort_and_dedup(relations))
}

/// Rust adapter: fqdns follow `relative_path::module::TypeName::method_name`
/// for methods and `relative_path::module::fn_name` for free functions, where
/// the module path is the file path with extension stripped, `/` folded to
/// `::`, and a leading `src/` component dropped.
pub struct RustAdapter;

impl LanguageAdapter for RustAdapter {
    /// Parses `source` with the Rust grammar; `None` never occurs for Rust.
    fn new_tree(&self, source: &str) -> Result<Option<Tree>, ParseError> {
        rust_tree(source).map(Some)
    }

    fn language(&self) -> Language {
        Language::Rust
    }

    fn capability_level(&self) -> CapabilityLevel {
        CapabilityLevel::L3Structural
    }

    fn parse_symbols(
        &self,
        source: &str,
        file_path: &Path,
    ) -> Result<Vec<SymbolIdentity>, ParseError> {
        let tree = rust_tree(source)?;
        let analysis = analyze_rust(&tree, source, file_path)?;
        Ok(symbol_identities(
            Language::Rust,
            file_path,
            source,
            &analysis.raws,
        ))
    }

    fn extract_relations(
        &self,
        source: &str,
        tree: &Tree,
        symbols: &[SymbolIdentity],
    ) -> Result<Vec<Relation>, ParseError> {
        let Some(first) = symbols.first() else {
            return Ok(Vec::new());
        };
        let path = first.file_path.as_path();
        let analysis = analyze_rust(tree, source, path)?;
        let module_hash = hash_text(source);
        let contains = resolve_contains(
            &module_prefix(path),
            &analysis.raws,
            &module_hash,
            analysis.container_requests,
        );
        let implements = rust_implements(tree, source, path, &analysis.raws)?;
        let calls = resolve_calls(&analysis.raws, &analysis.calls);
        Ok(sort_and_dedup({
            let mut all = contains;
            all.extend(implements);
            all.extend(calls);
            all
        }))
    }
}

// ---------------------------------------------------------------------------
// GoAdapter
// ---------------------------------------------------------------------------

const GO_SYMBOL_QUERY_SRC: &str = r#"
    (function_declaration name: (identifier) @func_name) @func_node
    (method_declaration name: (field_identifier) @method_name) @method_node
    (type_spec name: (type_identifier) @type_name) @type_node
"#;

fn go_symbol_query() -> Result<&'static Query, ParseError> {
    static CELL: OnceLock<Result<Query, String>> = OnceLock::new();
    cached_query(&CELL, tree_sitter_go::LANGUAGE.into(), GO_SYMBOL_QUERY_SRC)
}

fn go_tree(source: &str) -> Result<Tree, ParseError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|error| ParseError::Grammar(error.to_string()))?;
    parser
        .parse(source, None)
        .ok_or(ParseError::InvalidSource("Go"))
}

fn go_package_name(root: Node, text: &[u8]) -> String {
    first_descendant_of(root, "package_clause")
        .and_then(|clause| first_descendant_of(clause, "package_identifier"))
        .and_then(|identifier| identifier.utf8_text(text).ok())
        .unwrap_or("main")
        .to_string()
}

fn go_receiver_type(node: Node, text: &[u8]) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    let type_node = first_descendant_of(receiver, "type_identifier")?;
    type_node.utf8_text(text).ok().map(str::to_string)
}

fn go_interface_methods(interface_node: Node, text: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut stack = vec![interface_node];
    while let Some(current) = stack.pop() {
        if current.kind() == "method_elem" {
            if let Some(name_node) = current.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(text) {
                    names.push(name.to_string());
                }
            }
        }
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            stack.push(child);
        }
    }
    names
}

struct GoAnalysis {
    package: String,
    raws: Vec<RawSymbol>,
    container_requests: Vec<(ContainerKey, usize)>,
    /// `(interface type name, its method element names)` for in-file interfaces.
    interfaces: Vec<(String, Vec<String>)>,
    /// `(receiver type name, its method names)` for in-file receiver types.
    receiver_methods: Vec<(String, Vec<String>)>,
    calls: Vec<CallSite>,
}

fn analyze_go(tree: &Tree, source: &str, _path: &Path) -> Result<GoAnalysis, ParseError> {
    let text = source.as_bytes();
    let root = tree.root_node();
    let query = go_symbol_query()?;
    let package = go_package_name(root, text);

    let mut raws: Vec<RawSymbol> = Vec::new();
    let mut container_requests: Vec<(ContainerKey, usize)> = Vec::new();
    let mut interfaces: Vec<(String, Vec<String>)> = Vec::new();
    let mut receiver_methods: Vec<(String, Vec<String>)> = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, text);
    while let Some(mat) = matches.next() {
        if let Some(node) = capture_node(query, mat, "func_node") {
            let Some(name) = capture_text(query, mat, "func_name", text) else {
                continue;
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                SymbolKind::Function,
                format!("{package}::{name}"),
                node,
                text,
                Some(ContainerKey::Module),
            );
        } else if let Some(node) = capture_node(query, mat, "method_node") {
            let Some(name) = capture_text(query, mat, "method_name", text) else {
                continue;
            };
            match go_receiver_type(node, text) {
                Some(receiver) => {
                    let entry = receiver_methods
                        .iter_mut()
                        .find(|(owner, _)| *owner == receiver);
                    match entry {
                        Some((_, methods)) => methods.push(name.to_string()),
                        None => receiver_methods.push((receiver.clone(), vec![name.to_string()])),
                    }
                    push_raw(
                        &mut raws,
                        &mut container_requests,
                        SymbolKind::Method,
                        format!("{package}::{receiver}::{name}"),
                        node,
                        text,
                        Some(ContainerKey::Type(receiver)),
                    );
                }
                None => push_raw(
                    &mut raws,
                    &mut container_requests,
                    SymbolKind::Function,
                    format!("{package}::{name}"),
                    node,
                    text,
                    Some(ContainerKey::Module),
                ),
            }
        } else if let Some(node) = capture_node(query, mat, "type_node") {
            let Some(name) = capture_text(query, mat, "type_name", text) else {
                continue;
            };
            let Some(type_child) = node.child_by_field_name("type") else {
                continue;
            };
            let (kind, container) = match type_child.kind() {
                "struct_type" => (SymbolKind::Struct, Some(ContainerKey::Module)),
                "interface_type" => {
                    interfaces.push((name.to_string(), go_interface_methods(type_child, text)));
                    (SymbolKind::Interface, Some(ContainerKey::Module))
                }
                _ => continue,
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                kind,
                format!("{package}::{name}"),
                node,
                text,
                container,
            );
        }
    }

    let mut calls = Vec::new();
    collect_call_sites(root, text, "call_expression", &mut calls);

    Ok(GoAnalysis {
        package,
        raws,
        container_requests,
        interfaces,
        receiver_methods,
        calls,
    })
}

fn go_implements(analysis: &GoAnalysis, raws: &[RawSymbol]) -> Vec<Relation> {
    let prefix = &analysis.package;
    let mut relations = Vec::new();
    for (interface_name, required) in &analysis.interfaces {
        if required.is_empty() {
            continue;
        }
        for (receiver, provided) in &analysis.receiver_methods {
            if receiver == interface_name || !required.iter().all(|m| provided.contains(m)) {
                continue;
            }
            let type_index = find_raw(
                prefix,
                raws,
                receiver,
                &[SymbolKind::Struct, SymbolKind::Interface],
            );
            let contract_index = find_raw(prefix, raws, interface_name, &[SymbolKind::Interface]);
            if let (Some(type_index), Some(contract_index)) = (type_index, contract_index) {
                relations.push(implements_edge(
                    &raws[type_index].hash,
                    &raws[contract_index].hash,
                ));
            }
        }
    }
    sort_and_dedup(relations)
}

/// Go adapter: fqdns follow `package_name::ReceiverType::MethodName` for
/// methods and `package_name::func_name` for free functions, where the package
/// clause is parsed from the source text.
pub struct GoAdapter;

impl LanguageAdapter for GoAdapter {
    /// Parses `source` with the Go grammar; `None` never occurs for Go.
    fn new_tree(&self, source: &str) -> Result<Option<Tree>, ParseError> {
        go_tree(source).map(Some)
    }

    fn language(&self) -> Language {
        Language::Go
    }

    fn capability_level(&self) -> CapabilityLevel {
        CapabilityLevel::L3Structural
    }

    fn parse_symbols(
        &self,
        source: &str,
        file_path: &Path,
    ) -> Result<Vec<SymbolIdentity>, ParseError> {
        let tree = go_tree(source)?;
        let analysis = analyze_go(&tree, source, file_path)?;
        Ok(symbol_identities(
            Language::Go,
            file_path,
            source,
            &analysis.raws,
        ))
    }

    fn extract_relations(
        &self,
        source: &str,
        tree: &Tree,
        symbols: &[SymbolIdentity],
    ) -> Result<Vec<Relation>, ParseError> {
        let Some(first) = symbols.first() else {
            return Ok(Vec::new());
        };
        let path = first.file_path.as_path();
        let analysis = analyze_go(tree, source, path)?;
        let module_hash = hash_text(source);
        let implements = go_implements(&analysis, &analysis.raws);
        let calls = resolve_calls(&analysis.raws, &analysis.calls);
        let contains = resolve_contains(
            &analysis.package,
            &analysis.raws,
            &module_hash,
            analysis.container_requests,
        );
        Ok(sort_and_dedup({
            let mut all = contains;
            all.extend(implements);
            all.extend(calls);
            all
        }))
    }
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript family
// ---------------------------------------------------------------------------

const TS_SYMBOL_QUERY_SRC: &str = r#"
    (function_declaration name: (identifier) @func_name) @func_node
    (class_declaration name: [
        (type_identifier) @class_name
        (identifier) @class_name
    ]) @class_node
    (interface_declaration name: (type_identifier) @interface_name) @interface_node
    (method_definition name: (property_identifier) @method_name) @method_node
"#;

const JS_SYMBOL_QUERY_SRC: &str = r#"
    (function_declaration name: (identifier) @func_name) @func_node
    (class_declaration name: (identifier) @class_name) @class_node
    (method_definition name: (property_identifier) @method_name) @method_node
"#;

const TS_IMPLEMENTS_QUERY_SRC: &str = r#"
    (class_declaration name: [
        (type_identifier) @class_name
        (identifier) @class_name
    ] (class_heritage (implements_clause (type_identifier) @interface_name)))
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TsFamily {
    TypeScript,
    JavaScript,
}

fn ts_family_symbol_query(family: TsFamily) -> Result<&'static Query, ParseError> {
    static TS_CELL: OnceLock<Result<Query, String>> = OnceLock::new();
    static JS_CELL: OnceLock<Result<Query, String>> = OnceLock::new();
    match family {
        TsFamily::TypeScript => cached_query(
            &TS_CELL,
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TS_SYMBOL_QUERY_SRC,
        ),
        TsFamily::JavaScript => cached_query(
            &JS_CELL,
            tree_sitter_javascript::LANGUAGE.into(),
            JS_SYMBOL_QUERY_SRC,
        ),
    }
}

fn ts_implements_query() -> Result<&'static Query, ParseError> {
    static CELL: OnceLock<Result<Query, String>> = OnceLock::new();
    cached_query(
        &CELL,
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        TS_IMPLEMENTS_QUERY_SRC,
    )
}

fn ts_family_tree(family: TsFamily, source: &str) -> Result<Tree, ParseError> {
    let grammar = match family {
        TsFamily::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        TsFamily::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
    };
    let label = match family {
        TsFamily::TypeScript => "TypeScript",
        TsFamily::JavaScript => "JavaScript",
    };
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&grammar)
        .map_err(|error| ParseError::Grammar(error.to_string()))?;
    parser
        .parse(source, None)
        .ok_or(ParseError::InvalidSource(label))
}

/// Names the lexical container governing a TS/JS member's fqdn and ownership.
enum TsContainer {
    Class { name: String },
    NestedFunction,
    None,
}

fn ts_container(node: Node, text: &[u8]) -> TsContainer {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "class_declaration" => {
                if let Some(name_node) = parent.child_by_field_name("name") {
                    if let Ok(name) = name_node.utf8_text(text) {
                        return TsContainer::Class {
                            name: name.to_string(),
                        };
                    }
                }
            }
            "function_declaration" | "arrow_function" | "function_expression" => {
                return TsContainer::NestedFunction;
            }
            _ => {}
        }
        current = parent.parent();
    }
    TsContainer::None
}

struct TsFamilyAnalysis {
    raws: Vec<RawSymbol>,
    container_requests: Vec<(ContainerKey, usize)>,
    calls: Vec<CallSite>,
}

fn analyze_ts_family(
    family: TsFamily,
    tree: &Tree,
    source: &str,
    path: &Path,
) -> Result<TsFamilyAnalysis, ParseError> {
    let prefix = module_prefix(path);
    let text = source.as_bytes();
    let root = tree.root_node();
    let query = ts_family_symbol_query(family)?;

    let mut raws: Vec<RawSymbol> = Vec::new();
    let mut container_requests: Vec<(ContainerKey, usize)> = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, text);
    while let Some(mat) = matches.next() {
        if let Some(node) = capture_node(query, mat, "func_node") {
            let Some(name) = capture_text(query, mat, "func_name", text) else {
                continue;
            };
            let (kind, fqdn, container) = match ts_container(node, text) {
                TsContainer::Class { name: class_name } => (
                    SymbolKind::Method,
                    format!("{prefix}::{class_name}::{name}"),
                    Some(ContainerKey::Type(class_name)),
                ),
                TsContainer::NestedFunction => {
                    (SymbolKind::Function, format!("{prefix}::{name}"), None)
                }
                TsContainer::None => (
                    SymbolKind::Function,
                    format!("{prefix}::{name}"),
                    Some(ContainerKey::Module),
                ),
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                kind,
                fqdn,
                node,
                text,
                container,
            );
        } else if let Some(node) = capture_node(query, mat, "class_node") {
            let Some(name) = capture_text(query, mat, "class_name", text) else {
                continue;
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                SymbolKind::Class,
                format!("{prefix}::{name}"),
                node,
                text,
                Some(ContainerKey::Module),
            );
        } else if let Some(node) = capture_node(query, mat, "interface_node") {
            let Some(name) = capture_text(query, mat, "interface_name", text) else {
                continue;
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                SymbolKind::Interface,
                format!("{prefix}::{name}"),
                node,
                text,
                Some(ContainerKey::Module),
            );
        } else if let Some(node) = capture_node(query, mat, "method_node") {
            let Some(name) = capture_text(query, mat, "method_name", text) else {
                continue;
            };
            let (kind, fqdn, container) = match ts_container(node, text) {
                TsContainer::Class { name: class_name } => (
                    SymbolKind::Method,
                    format!("{prefix}::{class_name}::{name}"),
                    Some(ContainerKey::Type(class_name)),
                ),
                // Object-literal methods and bare method syntax degrade to
                // file-level functions.
                TsContainer::NestedFunction | TsContainer::None => (
                    SymbolKind::Function,
                    format!("{prefix}::{name}"),
                    (if matches!(ts_container(node, text), TsContainer::None) {
                        Some(ContainerKey::Module)
                    } else {
                        None
                    }),
                ),
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                kind,
                fqdn,
                node,
                text,
                container,
            );
        }
    }

    let mut calls = Vec::new();
    collect_call_sites(root, text, "call_expression", &mut calls);

    Ok(TsFamilyAnalysis {
        raws,
        container_requests,
        calls,
    })
}

fn ts_implements(
    tree: &Tree,
    source: &str,
    path: &Path,
    raws: &[RawSymbol],
) -> Result<Vec<Relation>, ParseError> {
    let prefix = module_prefix(path);
    let text = source.as_bytes();
    let query = ts_implements_query()?;
    let mut relations = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text);
    while let Some(mat) = matches.next() {
        let Some(class_name) = capture_text(query, mat, "class_name", text) else {
            continue;
        };
        let Some(interface_name) = capture_text(query, mat, "interface_name", text) else {
            continue;
        };
        let class_index = find_raw(&prefix, raws, class_name, &[SymbolKind::Class]);
        let interface_index = find_raw(&prefix, raws, interface_name, &[SymbolKind::Interface]);
        if let (Some(class_index), Some(interface_index)) = (class_index, interface_index) {
            relations.push(implements_edge(
                &raws[class_index].hash,
                &raws[interface_index].hash,
            ));
        }
    }
    Ok(sort_and_dedup(relations))
}

/// TypeScript adapter: fqdns follow `relative_path::ClassName::methodName` for
/// methods and `relative_path::fn_name` for top-level functions.
pub struct TypeScriptAdapter;

impl LanguageAdapter for TypeScriptAdapter {
    /// Parses `source` with the TypeScript grammar.
    fn new_tree(&self, source: &str) -> Result<Option<Tree>, ParseError> {
        ts_family_tree(TsFamily::TypeScript, source).map(Some)
    }

    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn capability_level(&self) -> CapabilityLevel {
        CapabilityLevel::L3Structural
    }

    fn parse_symbols(
        &self,
        source: &str,
        file_path: &Path,
    ) -> Result<Vec<SymbolIdentity>, ParseError> {
        let tree = ts_family_tree(TsFamily::TypeScript, source)?;
        let analysis = analyze_ts_family(TsFamily::TypeScript, &tree, source, file_path)?;
        Ok(symbol_identities(
            Language::TypeScript,
            file_path,
            source,
            &analysis.raws,
        ))
    }

    fn extract_relations(
        &self,
        source: &str,
        tree: &Tree,
        symbols: &[SymbolIdentity],
    ) -> Result<Vec<Relation>, ParseError> {
        let Some(first) = symbols.first() else {
            return Ok(Vec::new());
        };
        let path = first.file_path.as_path();
        let analysis = analyze_ts_family(TsFamily::TypeScript, tree, source, path)?;
        let module_hash = hash_text(source);
        let contains = resolve_contains(
            &module_prefix(path),
            &analysis.raws,
            &module_hash,
            analysis.container_requests,
        );
        let implements = ts_implements(tree, source, path, &analysis.raws)?;
        let calls = resolve_calls(&analysis.raws, &analysis.calls);
        Ok(sort_and_dedup({
            let mut all = contains;
            all.extend(implements);
            all.extend(calls);
            all
        }))
    }
}

/// JavaScript adapter: shares the TypeScript-family extractor but reports
/// [`Language::JavaScript`]; no interfaces exist, so `Implements` is never
/// emitted.
pub struct JavaScriptAdapter;

impl LanguageAdapter for JavaScriptAdapter {
    /// Parses `source` with the JavaScript grammar.
    fn new_tree(&self, source: &str) -> Result<Option<Tree>, ParseError> {
        ts_family_tree(TsFamily::JavaScript, source).map(Some)
    }

    fn language(&self) -> Language {
        Language::JavaScript
    }

    fn capability_level(&self) -> CapabilityLevel {
        CapabilityLevel::L3Structural
    }

    fn parse_symbols(
        &self,
        source: &str,
        file_path: &Path,
    ) -> Result<Vec<SymbolIdentity>, ParseError> {
        let tree = ts_family_tree(TsFamily::JavaScript, source)?;
        let analysis = analyze_ts_family(TsFamily::JavaScript, &tree, source, file_path)?;
        Ok(symbol_identities(
            Language::JavaScript,
            file_path,
            source,
            &analysis.raws,
        ))
    }

    fn extract_relations(
        &self,
        source: &str,
        tree: &Tree,
        symbols: &[SymbolIdentity],
    ) -> Result<Vec<Relation>, ParseError> {
        let Some(first) = symbols.first() else {
            return Ok(Vec::new());
        };
        let path = first.file_path.as_path();
        let analysis = analyze_ts_family(TsFamily::JavaScript, tree, source, path)?;
        let module_hash = hash_text(source);
        let contains = resolve_contains(
            &module_prefix(path),
            &analysis.raws,
            &module_hash,
            analysis.container_requests,
        );
        let calls = resolve_calls(&analysis.raws, &analysis.calls);
        Ok(sort_and_dedup({
            let mut all = contains;
            all.extend(calls);
            all
        }))
    }
}

// ---------------------------------------------------------------------------
// PythonAdapter
// ---------------------------------------------------------------------------

const PY_SYMBOL_QUERY_SRC: &str = r#"
    (function_definition name: (identifier) @func_name) @func_node
    (class_definition name: (identifier) @class_name) @class_node
"#;

fn py_symbol_query() -> Result<&'static Query, ParseError> {
    static CELL: OnceLock<Result<Query, String>> = OnceLock::new();
    cached_query(
        &CELL,
        tree_sitter_python::LANGUAGE.into(),
        PY_SYMBOL_QUERY_SRC,
    )
}

fn py_tree(source: &str) -> Result<Tree, ParseError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|error| ParseError::Grammar(error.to_string()))?;
    parser
        .parse(source, None)
        .ok_or(ParseError::InvalidSource("Python"))
}

/// Names the lexical container governing a Python def/class's fqdn.
enum PyContainer {
    /// Dotted chain of enclosing classes, outermost first (`A.B`).
    Classes(String),
    /// Function nested inside another function.
    NestedFunction,
    /// Top-level binding owned by the module.
    None,
}

fn python_container(node: Node, text: &[u8]) -> PyContainer {
    let mut chain: Vec<String> = Vec::new();
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "class_definition" => {
                if let Some(name_node) = parent.child_by_field_name("name") {
                    if let Ok(name) = name_node.utf8_text(text) {
                        chain.push(name.to_string());
                    }
                }
            }
            "function_definition" if chain.is_empty() => return PyContainer::NestedFunction,
            _ => {}
        }
        current = parent.parent();
    }
    if chain.is_empty() {
        PyContainer::None
    } else {
        chain.reverse();
        PyContainer::Classes(chain.join("."))
    }
}

struct PyAnalysis {
    raws: Vec<RawSymbol>,
    container_requests: Vec<(ContainerKey, usize)>,
    calls: Vec<CallSite>,
}

fn analyze_py(tree: &Tree, source: &str, path: &Path) -> Result<PyAnalysis, ParseError> {
    let prefix = module_prefix(path);
    let text = source.as_bytes();
    let root = tree.root_node();
    let query = py_symbol_query()?;

    let mut raws: Vec<RawSymbol> = Vec::new();
    let mut container_requests: Vec<(ContainerKey, usize)> = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, text);
    while let Some(mat) = matches.next() {
        if let Some(node) = capture_node(query, mat, "func_node") {
            let Some(name) = capture_text(query, mat, "func_name", text) else {
                continue;
            };
            let (kind, fqdn, container) = match python_container(node, text) {
                PyContainer::Classes(chain) => (
                    SymbolKind::Method,
                    format!("{prefix}::{chain}.{name}"),
                    Some(ContainerKey::Type(chain)),
                ),
                PyContainer::NestedFunction => {
                    (SymbolKind::Function, format!("{prefix}::{name}"), None)
                }
                PyContainer::None => (
                    SymbolKind::Function,
                    format!("{prefix}::{name}"),
                    Some(ContainerKey::Module),
                ),
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                kind,
                fqdn,
                node,
                text,
                container,
            );
        } else if let Some(node) = capture_node(query, mat, "class_node") {
            let Some(name) = capture_text(query, mat, "class_name", text) else {
                continue;
            };
            let (fqdn, container) = match python_container(node, text) {
                PyContainer::Classes(chain) => (
                    format!("{prefix}::{chain}.{name}"),
                    // Nested classes are owned by their outer class, resolved
                    // through the same Type key.
                    Some(ContainerKey::Type(chain)),
                ),
                PyContainer::NestedFunction | PyContainer::None => {
                    (format!("{prefix}::{name}"), Some(ContainerKey::Module))
                }
            };
            push_raw(
                &mut raws,
                &mut container_requests,
                SymbolKind::Class,
                fqdn,
                node,
                text,
                container,
            );
        }
    }

    let mut calls = Vec::new();
    collect_call_sites(root, text, "call", &mut calls);

    Ok(PyAnalysis {
        raws,
        container_requests,
        calls,
    })
}

/// Python adapter: fqdns use dots below the `::` path prefix —
/// `relative_path::ClassName.method_name` for methods and
/// `relative_path::fn_name` for top-level functions.
pub struct PythonAdapter;

impl LanguageAdapter for PythonAdapter {
    /// Parses `source` with the Python grammar.
    fn new_tree(&self, source: &str) -> Result<Option<Tree>, ParseError> {
        py_tree(source).map(Some)
    }

    fn language(&self) -> Language {
        Language::Python
    }

    fn capability_level(&self) -> CapabilityLevel {
        CapabilityLevel::L3Structural
    }

    fn parse_symbols(
        &self,
        source: &str,
        file_path: &Path,
    ) -> Result<Vec<SymbolIdentity>, ParseError> {
        let tree = py_tree(source)?;
        let analysis = analyze_py(&tree, source, file_path)?;
        Ok(symbol_identities(
            Language::Python,
            file_path,
            source,
            &analysis.raws,
        ))
    }

    fn extract_relations(
        &self,
        source: &str,
        tree: &Tree,
        symbols: &[SymbolIdentity],
    ) -> Result<Vec<Relation>, ParseError> {
        let Some(first) = symbols.first() else {
            return Ok(Vec::new());
        };
        let path = first.file_path.as_path();
        let analysis = analyze_py(tree, source, path)?;
        let module_hash = hash_text(source);
        let contains = resolve_contains(
            &module_prefix(path),
            &analysis.raws,
            &module_hash,
            analysis.container_requests,
        );
        let calls = resolve_calls(&analysis.raws, &analysis.calls);
        Ok(sort_and_dedup({
            let mut all = contains;
            all.extend(calls);
            all
        }))
    }
}

// ---------------------------------------------------------------------------
// GenericAdapter
// ---------------------------------------------------------------------------

/// Splits `source` into ATX-heading sections, returning `(slug, section text)`
/// pairs. Text preceding the first heading is ignored.
fn markdown_sections(source: &str) -> Vec<(String, String)> {
    let mut headings: Vec<(usize, String)> = Vec::new();
    let mut offset = 0usize;
    for line in source.split_inclusive('\n') {
        if let Some(slug) = atx_slug(line.trim_end_matches(['\n', '\r'])) {
            headings.push((offset, slug));
        }
        offset += line.len();
    }
    let mut sections = Vec::with_capacity(headings.len());
    for (index, (start, slug)) in headings.iter().enumerate() {
        let end = headings
            .get(index + 1)
            .map_or(source.len(), |(next_start, _)| *next_start);
        sections.push((slug.clone(), source[*start..end].trim_end().to_string()));
    }
    sections
}

/// Maps an ATX heading line to its slug (`## Deep Dive!` -> `deep-dive`).
fn atx_slug(line: &str) -> Option<String> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let title = rest.trim();
    if title.is_empty() {
        return None;
    }
    let mut slug = String::new();
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        None
    } else {
        Some(slug)
    }
}

/// Grammar-less adapter handling Markdown sections plus YAML, JSON, TOML, and
/// unrecognized formats at [`CapabilityLevel::L1Parsed`].
///
/// Markdown sources yield one [`SymbolKind::Module`] per ATX-heading section
/// with fqdn `relative_path::heading_slug`; every other input yields a single
/// whole-file Module with a path-derived fqdn. Per the trait contract,
/// `extract_relations` ALWAYS returns `Ok(vec![])` and never dereferences the
/// tree, and `new_tree` always yields `None`.
pub struct GenericAdapter {
    language: Language,
}

impl GenericAdapter {
    /// Creates a generic adapter reporting `language`.
    pub const fn new(language: Language) -> Self {
        Self { language }
    }

    /// Always `Ok(None)`: this adapter owns no tree-sitter grammar.
    pub fn new_tree(&self, _source: &str) -> Result<Option<Tree>, ParseError> {
        Ok(None)
    }
}

impl LanguageAdapter for GenericAdapter {
    fn language(&self) -> Language {
        self.language
    }

    fn capability_level(&self) -> CapabilityLevel {
        CapabilityLevel::L1Parsed
    }

    fn parse_symbols(
        &self,
        source: &str,
        file_path: &Path,
    ) -> Result<Vec<SymbolIdentity>, ParseError> {
        let prefix = module_prefix(file_path);
        let identities = match self.language() {
            Language::Markdown => markdown_sections(source)
                .into_iter()
                .map(|(slug, slice)| SymbolIdentity {
                    language: Language::Markdown,
                    fqdn: format!("{prefix}::{slug}"),
                    kind: SymbolKind::Module,
                    file_path: file_path.to_path_buf(),
                    content_hash: hash_text(&slice),
                })
                .collect(),
            _ => vec![SymbolIdentity {
                language: self.language(),
                fqdn: prefix,
                kind: SymbolKind::Module,
                file_path: file_path.to_path_buf(),
                content_hash: hash_text(source),
            }],
        };
        Ok(identities)
    }

    fn extract_relations(
        &self,
        _source: &str,
        _tree: &Tree,
        _symbols: &[SymbolIdentity],
    ) -> Result<Vec<Relation>, ParseError> {
        // Contract: grammar-less adapters never produce relations and never
        // dereference the tree. The facade never calls this method for them.
        Ok(Vec::new())
    }
}
