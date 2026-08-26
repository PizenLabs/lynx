#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryIntent {
    Symbol,       // Exact matches, e.g., "AuthService"
    Definition,   // Target signatures, e.g., "ParseJWT"
    Flow,         // Control path mapping, e.g., "authentication flow"
    Architecture, // Inter-crate relationships, e.g., "request lifecycle"
    Semantic,     // Broad natural language, e.g., "how do we cache vectors"
}

impl QueryIntent {
    pub fn weights(self) -> (f32, f32) {
        match self {
            QueryIntent::Symbol => (0.95, 0.05),
            QueryIntent::Definition => (0.80, 0.20),
            QueryIntent::Flow => (0.40, 0.60),
            QueryIntent::Architecture => (0.30, 0.70),
            QueryIntent::Semantic => (0.10, 0.90),
        }
    }
}

pub fn classify_query(query: &str) -> QueryIntent {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return QueryIntent::Semantic;
    }

    let has_whitespace = trimmed.split_whitespace().count() > 1;
    let has_symbol_marker = trimmed.contains("func:")
        || trimmed.contains("type:")
        || trimmed.contains("method:")
        || trimmed.contains("::")
        || trimmed.contains('.');

    if !has_whitespace {
        if has_symbol_marker {
            return QueryIntent::Symbol;
        }
        return QueryIntent::Symbol;
    }

    let query_lower = trimmed.to_lowercase();

    // Heuristics for Flow
    if query_lower.contains("flow")
        || query_lower.contains("lifecycle")
        || query_lower.contains("path")
        || query_lower.contains("sequence")
    {
        return QueryIntent::Flow;
    }

    // Heuristics for Architecture
    if query_lower.contains("architecture")
        || query_lower.contains("module")
        || query_lower.contains("crate")
        || query_lower.contains("relationship")
        || query_lower.contains("dependency")
    {
        return QueryIntent::Architecture;
    }

    // Heuristics for Definition
    if query_lower.contains("define")
        || query_lower.contains("definition")
        || query_lower.contains("implement")
        || query_lower.contains("signature")
    {
        return QueryIntent::Definition;
    }

    let has_identifier = trimmed.split_whitespace().any(is_identifier_like);

    if has_identifier && !has_whitespace {
        QueryIntent::Symbol
    } else if has_identifier {
        QueryIntent::Definition
    } else {
        QueryIntent::Semantic
    }
}

fn is_identifier_like(token: &str) -> bool {
    token.contains("::")
        || token.contains('.')
        || token.contains('_')
        || token.chars().any(|c| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_defaults_to_semantic() {
        assert_eq!(classify_query(""), QueryIntent::Semantic);
        assert_eq!(classify_query("   "), QueryIntent::Semantic);
    }

    #[test]
    fn test_single_token_is_symbol() {
        assert_eq!(classify_query("AuthService"), QueryIntent::Symbol);
        assert_eq!(classify_query("parse_jwt"), QueryIntent::Symbol);
        assert_eq!(classify_query("core::ranking"), QueryIntent::Symbol);
        assert_eq!(classify_query("config.load"), QueryIntent::Symbol);
    }

    #[test]
    fn test_flow_keywords() {
        assert_eq!(classify_query("authentication flow"), QueryIntent::Flow);
        assert_eq!(classify_query("request lifecycle"), QueryIntent::Flow);
        assert_eq!(classify_query("error handling path"), QueryIntent::Flow);
        assert_eq!(classify_query("execution sequence"), QueryIntent::Flow);
    }

    #[test]
    fn test_architecture_keywords() {
        assert_eq!(
            classify_query("module relationships"),
            QueryIntent::Architecture
        );
        assert_eq!(
            classify_query("crate dependency graph"),
            QueryIntent::Architecture
        );
    }

    #[test]
    fn test_definition_keywords() {
        assert_eq!(
            classify_query("define retry policy"),
            QueryIntent::Definition
        );
        assert_eq!(
            classify_query("implement backoff strategy"),
            QueryIntent::Definition
        );
        assert_eq!(
            classify_query("public function signature"),
            QueryIntent::Definition
        );
    }

    #[test]
    fn test_multiword_identifier_is_definition() {
        // Identifier-like tokens in multi-word queries mean "find this thing"
        assert_eq!(
            classify_query("parse_jwt validation"),
            QueryIntent::Definition
        );
        assert_eq!(
            classify_query("AuthService entry point"),
            QueryIntent::Definition
        );
    }

    #[test]
    fn test_plain_language_is_semantic() {
        assert_eq!(
            classify_query("how do we cache vectors"),
            QueryIntent::Semantic
        );
    }

    #[test]
    fn test_flow_takes_precedence_over_architecture() {
        // Flow keywords are checked before Architecture keywords
        assert_eq!(
            classify_query("architecture flow overview"),
            QueryIntent::Flow
        );
        // "path" is a Flow keyword even alongside an Architecture keyword
        assert_eq!(classify_query("dependency path tracing"), QueryIntent::Flow);
    }

    #[test]
    fn test_weights_per_intent() {
        assert_eq!(QueryIntent::Symbol.weights(), (0.95, 0.05));
        assert_eq!(QueryIntent::Definition.weights(), (0.80, 0.20));
        assert_eq!(QueryIntent::Flow.weights(), (0.40, 0.60));
        assert_eq!(QueryIntent::Architecture.weights(), (0.30, 0.70));
        assert_eq!(QueryIntent::Semantic.weights(), (0.10, 0.90));
    }
}
