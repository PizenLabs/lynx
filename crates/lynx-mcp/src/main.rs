use anyhow::Result;
use lynx_core::Lynx;
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct Request {
    method: String,
    params: Option<serde_json::Value>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let storage_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".lynx"));

    let lynx = Lynx::new(&storage_path).await?;

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                writeln!(stdout, "{}", json!({"error": err.to_string()}))?;
                continue;
            }
        };

        let response = handle_request(&lynx, request).await;
        writeln!(stdout, "{}", response)?;
    }

    Ok(())
}

async fn handle_request(lynx: &Lynx, request: Request) -> serde_json::Value {
    match request.method.as_str() {
        "search" => {
            let query = request
                .params
                .as_ref()
                .and_then(|value| value.get("query"))
                .and_then(|value| value.as_str());

            match query {
                Some(query) => match lynx.search(query).await {
                    Ok(results) => json!({"result": results}),
                    Err(err) => json!({"error": err.to_string()}),
                },
                None => json!({"error": "Missing query parameter"}),
            }
        }
        "resolve_symbol" => {
            let name = request
                .params
                .as_ref()
                .and_then(|value| value.get("name"))
                .and_then(|value| value.as_str());

            match name {
                Some(name) => match lynx.resolve_symbol(name).await {
                    Ok(results) => json!({"result": results}),
                    Err(err) => json!({"error": err.to_string()}),
                },
                None => json!({"error": "Missing name parameter"}),
            }
        }
        "find_related" => {
            let file_path = request
                .params
                .as_ref()
                .and_then(|value| value.get("file"))
                .and_then(|value| value.as_str());
            let line = request
                .params
                .as_ref()
                .and_then(|value| value.get("line"))
                .and_then(|value| value.as_u64());

            match (file_path, line) {
                (Some(file_path), Some(line)) => match lynx.find_related(file_path, line as usize).await {
                    Ok(results) => json!({"result": results}),
                    Err(err) => json!({"error": err.to_string()}),
                },
                _ => json!({"error": "Missing file or line parameter"}),
            }
        }
        _ => json!({"error": "Unknown method"}),
    }
}
