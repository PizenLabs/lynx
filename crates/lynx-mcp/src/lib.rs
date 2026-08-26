//! Minimal, stable stdio JSON-RPC transport exposing Lynx primitives over
//! the Model Context Protocol.
//!
//! The server speaks JSON-RPC 2.0 newline-delimited frames on stdin/stdout
//! and exposes exactly seven tools mapping one-to-one onto
//! [`pizen_lynx_core::Engine`] primitives:
//!
//! | Tool           | Primitive                        |
//! |----------------|----------------------------------|
//! | `search`       | [`Engine::search`]               |
//! | `resolve`      | [`Engine::resolve`]              |
//! | `inspect`      | [`Engine::inspect`]              |
//! | `relations`    | [`Engine::relations`]            |
//! | `similar`      | [`Engine::similar`]              |
//! | `context`      | [`Engine::context`]              |
//! | `index_status` | [`Engine::index_status`]         |
//!
//! Internal storage surfaces (SQLite, Tantivy) are never exposed.
//!
//! Lifecycle: [`run`] opens a [`Session`] (ephemeral substrate indexed once
//! from the configured workspace), then serves until stdin EOF. All
//! diagnostics go to stderr; stdout carries only protocol frames.

#![forbid(unsafe_code)]

mod rpc;
pub mod session;
mod tools;

use std::path::PathBuf;

use anyhow::Result;

pub use session::{workspace_root, Session};

/// Configuration for [`run`].
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Workspace root indexed at startup.
    pub workspace_root: PathBuf,
    /// Whether test, mock, and generated files join the index.
    pub include_tests: bool,
}

/// Runs the stdio server until stdin EOF.
pub fn run(config: ServerConfig) -> Result<()> {
    let session = Session::open(&config.workspace_root, config.include_tests)?;
    eprintln!(
        "lynx-mcp serving {} ({} symbols)",
        config.workspace_root.display(),
        session.engine().index_status().symbol_count,
    );
    rpc::serve(&session)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Hermetic fixtures: an 8-dimensional fake provider plus a session
    //! over a tiny Go workspace, mirroring the core engine's own tests.

    use super::*;

    /// Deterministic fake provider seeded by character sums, so tests never
    /// load the ONNX model.
    pub struct FakeProvider;

    impl lynx_embed::VectorProvider for FakeProvider {
        fn embed_query(&self, text: &str) -> Result<Vec<f32>, lynx_embed::EmbedError> {
            Ok(fake_vec(text))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, lynx_embed::EmbedError> {
            Ok(texts.iter().map(|t| fake_vec(t)).collect())
        }

        fn dimension(&self) -> usize {
            8
        }
    }

    fn fake_vec(text: &str) -> Vec<f32> {
        let mut out = [0.0_f32; 8];
        for (i, ch) in text.chars().enumerate() {
            out[i % 8] += ch as u32 as f32;
        }
        let norm = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in out.iter_mut() {
                *x /= norm;
            }
        }
        out.to_vec()
    }

    /// Guard keeping the fixture workspace directory alive.
    pub struct Fixture {
        pub _workspace: tempfile::TempDir,
        pub session: Session,
    }

    /// A session over a two-symbol Go workspace (`auth::User`,
    /// `auth::Validate`) with a Calls edge between them.
    pub fn fixture_session() -> Fixture {
        let workspace = tempfile::tempdir().expect("temp workspace");
        std::fs::write(
            workspace.path().join("service.go"),
            "package auth\n\ntype User struct {\n    Name string\n}\n\nfunc (u *User) Name() string { return u.Name }\n\nfunc Validate(u *User) bool { return u.Name() != \"\" }\n",
        )
        .expect("fixture source");
        let session = Session::open_with(workspace.path(), false, Box::new(FakeProvider))
            .expect("fixture session");
        Fixture {
            _workspace: workspace,
            session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::fixture_session;
    use crate::rpc;
    use serde_json::Value;

    fn request(id: u64, method: &str, params: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#)
    }

    fn result_of(response: &str) -> Value {
        let value: Value = serde_json::from_str(response).expect("valid json");
        assert!(value.get("error").is_none(), "unexpected error: {response}");
        value["result"].clone()
    }

    #[test]
    fn tools_list_exposes_exactly_the_seven_primitives() {
        let fixture = fixture_session();
        let response =
            rpc::handle_for_test(&fixture.session, &request(1, "tools/list", "{}")).unwrap();
        let tools = result_of(&response)["tools"].as_array().unwrap().clone();
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "search",
                "resolve",
                "inspect",
                "relations",
                "similar",
                "context",
                "index_status"
            ]
        );
    }

    #[test]
    fn direct_search_returns_raw_evidence_array() {
        let fixture = fixture_session();
        let response = rpc::handle_for_test(
            &fixture.session,
            &request(2, "search", r#"{"query":"Validate","limit":5}"#),
        )
        .unwrap();
        let evidence = result_of(&response).as_array().expect("array").clone();
        assert!(!evidence.is_empty());
        assert!(evidence[0]["identity"]["fqdn"].as_str().is_some());
        assert!(evidence[0]["range"]["start_line"].as_u64().is_some());
    }

    #[test]
    fn tools_call_wraps_results_in_content_envelope() {
        let fixture = fixture_session();
        let response = rpc::handle_for_test(
            &fixture.session,
            &request(
                3,
                "tools/call",
                r#"{"name":"resolve","arguments":{"name":"auth::Validate"}}"#,
            ),
        )
        .unwrap();
        let result = result_of(&response);
        assert_eq!(result["isError"], Value::Bool(false));
        let inner: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap())
            .expect("embedded json");
        assert_eq!(inner["identity"]["fqdn"], "auth::Validate");
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let fixture = fixture_session();
        let response = rpc::handle_for_test(
            &fixture.session,
            &request(4, "tools/call", r#"{"name":"sqlite_query","arguments":{}}"#),
        )
        .unwrap();
        assert!(response.contains("-32602"));
    }

    #[test]
    fn similar_unknown_hash_reports_tool_failure_not_protocol_error() {
        let fixture = fixture_session();
        let resolved = rpc::handle_for_test(
            &fixture.session,
            &request(5, "resolve", r#"{"name":"auth::Validate"}"#),
        )
        .unwrap();
        let hash = result_of(&resolved)["identity"]["content_hash"]
            .as_str()
            .unwrap()
            .to_string();
        // Self lookup would succeed; an absent hash cannot be embedded, so
        // the primitive fails internally and must surface as isError.
        let bogus = format!("{hash}00");
        let response = rpc::handle_for_test(
            &fixture.session,
            &request(
                6,
                "tools/call",
                &format!(r#"{{"name":"similar","arguments":{{"symbol_hash":"{bogus}"}}}}"#),
            ),
        )
        .unwrap();
        let result = result_of(&response);
        assert_eq!(result["isError"], Value::Bool(true));
    }

    #[test]
    fn index_status_validates_requested_path() {
        let fixture = fixture_session();
        let ok = rpc::handle_for_test(
            &fixture.session,
            &request(
                7,
                "tools/call",
                r#"{"name":"index_status","arguments":{"path":""}}"#,
            ),
        );
        // Empty path mismatches the served root -> invalid params.
        assert!(ok.unwrap().contains("-32602"));

        let served = fixture
            .session
            .workspace_root()
            .to_string_lossy()
            .to_string();
        let body = format!(
            r#"{{"name":"index_status","arguments":{{"path":"{}"}}}}"#,
            served.replace('\\', "\\\\")
        );
        let response =
            rpc::handle_for_test(&fixture.session, &request(8, "tools/call", &body)).unwrap();
        let status: Value =
            serde_json::from_str(result_of(&response)["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert!(status["file_count"].as_u64().unwrap() >= 1);
        assert_eq!(status["capability_level"], "L3Structural");
    }

    #[test]
    fn context_respects_token_budget() {
        let fixture = fixture_session();
        let response = rpc::handle_for_test(
            &fixture.session,
            &request(
                9,
                "tools/call",
                r#"{"name":"context","arguments":{"query":"Validate","token_budget":40}}"#,
            ),
        )
        .unwrap();
        let package: Value =
            serde_json::from_str(result_of(&response)["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert!(package["estimated_tokens"].as_u64().unwrap() <= 40);
        assert!(!package["primary_evidence"].as_array().unwrap().is_empty());
    }

    #[test]
    fn relations_filter_by_kind() {
        let fixture = fixture_session();
        let resolved = rpc::handle_for_test(
            &fixture.session,
            &request(10, "resolve", r#"{"name":"auth::Validate"}"#),
        )
        .unwrap();
        let hash = result_of(&resolved)["identity"]["content_hash"]
            .as_str()
            .unwrap()
            .to_string();
        let body = format!(
            r#"{{"name":"relations","arguments":{{"symbol_hash":"{hash}","kind":"Calls"}}}}"#
        );
        let response =
            rpc::handle_for_test(&fixture.session, &request(11, "tools/call", &body)).unwrap();
        let edges: Value =
            serde_json::from_str(result_of(&response)["content"][0]["text"].as_str().unwrap())
                .unwrap();
        for edge in edges.as_array().unwrap() {
            assert_eq!(edge["kind"], "Calls");
        }
        assert!(!edges.as_array().unwrap().is_empty());
    }
}
