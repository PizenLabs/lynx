use anyhow::{Context, Result};
use lynx_core::Lynx;
use serde::Deserialize;
use serde_json::json;
use std::future;
use std::path::PathBuf;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Deserialize)]
struct Request {
    method: String,
    params: Option<serde_json::Value>,
}

fn find_lynx_dir() -> Option<PathBuf> {
    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join(".lynx").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let storage_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(find_lynx_dir)
        .unwrap_or_else(|| PathBuf::from(".lynx"));

    let storage_dir = if storage_path.is_relative() {
        if storage_path.as_os_str() == ".lynx" {
            // Default .lynx — try to resolve relative to project root
            find_lynx_dir()
                .map(|root| root.join(".lynx"))
                .unwrap_or(storage_path)
        } else {
            storage_path
        }
    } else {
        storage_path
    };

    let lynx = Lynx::new(&storage_dir)
        .await
        .with_context(|| format!("Failed to initialize Lynx at {:?}", storage_dir))?;

    let stdin = BufReader::new(io::stdin());
    let mut stdout = io::stdout();
    let mut lines = stdin.lines();

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        let line = line.trim().to_string();
                        if line.is_empty() {
                            continue;
                        }

                        let request: Request = match serde_json::from_str(&line) {
                            Ok(request) => request,
                            Err(err) => {
                                let response = json!({"error": err.to_string()});
                                let mut buf = serde_json::to_string(&response)?;
                                buf.push('\n');
                                let _ = stdout.write_all(buf.as_bytes()).await;
                                let _ = stdout.flush().await;
                                continue;
                            }
                        };

                        let response = handle_request(&lynx, request).await;
                        let mut buf = serde_json::to_string(&response)?;
                        buf.push('\n');
                        if let Err(e) = stdout.write_all(buf.as_bytes()).await {
                            eprintln!("write error: {}", e);
                            break;
                        }
                        if let Err(e) = stdout.flush().await {
                            eprintln!("flush error: {}", e);
                            break;
                        }
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(e) => {
                        eprintln!("stdin error: {}", e);
                        break;
                    }
                }
            }
            // Keep the async runtime alive even during idle periods
            // Prevents premature process exit on some MCP clients
            _ = future::pending::<()>() => {}
        }
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
                (Some(file_path), Some(line)) => {
                    match lynx.find_related(file_path, line as usize).await {
                        Ok(results) => json!({"result": results}),
                        Err(err) => json!({"error": err.to_string()}),
                    }
                }
                _ => json!({"error": "Missing file or line parameter"}),
            }
        }
        _ => json!({"error": "Unknown method"}),
    }
}
