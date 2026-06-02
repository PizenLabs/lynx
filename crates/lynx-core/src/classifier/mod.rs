#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryType {
    Symbol,
    NaturalLanguage,
    Hybrid,
}

impl QueryType {
    pub fn weights(self) -> (f32, f32) {
        match self {
            QueryType::Symbol => (0.90, 0.10),
            QueryType::Hybrid => (0.50, 0.50),
            QueryType::NaturalLanguage => (0.10, 0.90),
        }
    }
}

pub fn classify_query(query: &str) -> QueryType {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return QueryType::NaturalLanguage;
    }

    let has_whitespace = trimmed.split_whitespace().count() > 1;
    let has_symbol_marker = trimmed.contains("func:")
        || trimmed.contains("type:")
        || trimmed.contains("method:")
        || trimmed.contains("::")
        || trimmed.contains('.');

    if has_symbol_marker && !has_whitespace {
        return QueryType::Symbol;
    }

    if !has_whitespace {
        return QueryType::Symbol;
    }

    let has_identifier = trimmed
        .split_whitespace()
        .any(|token| is_identifier_like(token));

    if has_identifier {
        QueryType::Hybrid
    } else {
        QueryType::NaturalLanguage
    }
}

fn is_identifier_like(token: &str) -> bool {
    token.contains("::")
        || token.contains('.')
        || token.contains('_')
        || token.chars().any(|c| c.is_ascii_uppercase())
}
