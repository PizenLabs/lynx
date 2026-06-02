#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryType {
    Symbol,
    NaturalLanguage,
    Hybrid,
}

impl QueryType {
    pub fn weights(self) -> (f32, f32) {
        match self {
            QueryType::Symbol => (0.95, 0.05),
            QueryType::Hybrid => (0.70, 0.30),
            QueryType::NaturalLanguage => (0.20, 0.80),
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

    let has_identifier = trimmed.split_whitespace().any(is_identifier_like);

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
