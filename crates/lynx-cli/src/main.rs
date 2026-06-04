use anyhow::Result;
use clap::{Parser, Subcommand};
use lynx_core::Lynx;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "lx")]
#[command(about = "Lynx: Discovery Engine for AI-Native Software Engineering", long_about = None)]
struct Cli {
    #[arg(short, long, default_value = ".lynx")]
    storage_path: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index a repository
    Index {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Include test, mock, generated files in indexing
        #[arg(long, action = clap::ArgAction::SetTrue, default_value_t = false)]
        include_tests: bool,
    },
    /// Search the index
    Search {
        query: String,
        /// Include test, mock, generated files in search results
        #[arg(long, action = clap::ArgAction::SetTrue, default_value_t = false)]
        include_tests: bool,
    },
    /// Resolve a symbol by name
    Resolve { name: String },
    /// Find related implementations
    Related { location: String },
    #[command(hide = true)]
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let mut lynx = Lynx::new(&cli.storage_path).await?;

    match cli.command {
        Commands::Index {
            path,
            include_tests,
        } => {
            println!("Indexing repository at {:?}", path);
            lynx.set_include_tests(include_tests);
            lynx.index_repository(&path).await?;
            println!("Indexing complete.");
        }
        Commands::Search {
            query,
            include_tests,
        } => {
            lynx.set_include_tests(include_tests);
            let results = lynx.search(&query).await?;
            if results.is_empty() {
                println!("No results found.");
            } else {
                for result in results {
                    println!("{}", format_discovery(&result));
                }
            }
        }
        Commands::Resolve { name } => {
            let results = lynx.resolve_symbol(&name).await?;
            if results.is_empty() {
                println!("No symbols found.");
            } else {
                for result in results {
                    println!("{}", format_discovery(&result));
                }
            }
        }
        Commands::Related { location } => {
            let (file_path, line) = parse_location(&location)?;
            let results = lynx.find_related(&file_path, line).await?;
            if results.is_empty() {
                println!("No related results found.");
            } else {
                for result in results {
                    println!("{}", format_discovery(&result));
                }
            }
        }
        Commands::Init { path } => {
            println!("Initializing indexes at {:?}", path);
            let mut lea_child = Command::new("lea")
                .arg("index")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            lynx.index_repository(&path).await?;
            let status = lea_child.wait()?;
            if !status.success() {
                return Err(anyhow::anyhow!("lea index failed with status {}", status));
            }
            println!("Initialization complete.");
        }
    }

    Ok(())
}

fn parse_location(location: &str) -> Result<(String, usize)> {
    let mut parts = location.rsplitn(2, ':');
    let line_part = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing line number"))?;
    let file_part = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing file path"))?;
    let line: usize = line_part
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid line number"))?;
    Ok((file_part.to_string(), line))
}

fn format_discovery(result: &lynx_protocol::DiscoveryResult) -> String {
    let (kind, symbol_name) = split_symbol_id(&result.symbol_id, &result.file_path);
    let lines = if result.start_line == result.end_line {
        format!("{}", result.start_line)
    } else {
        format!("{}-{}", result.start_line, result.end_line)
    };

    // Normalize score to 0-100% for display
    // BM25 + Vector scores can be small, so we use a scaling factor
    let percentage = (result.score * 100.0).min(100.0);

    let confidence = if percentage > 85.0 {
        "High"
    } else if percentage > 50.0 {
        "Medium"
    } else {
        "Low"
    };

    let why_str = if result.reasons.is_empty() {
        "".to_string()
    } else {
        let reasons_list: Vec<String> = result
            .reasons
            .iter()
            .map(|r| format!("  - {}", r))
            .collect();
        format!("\n  Why:\n{}\n", reasons_list.join("\n"))
    };

    format!(
        "{}\n  {}\n\n  Confidence: {} ({:.0}%)\n{}\n  Symbol:\n  {}\n\n  File:\n  {}:{}\n",
        kind.to_uppercase(),
        symbol_name,
        confidence,
        percentage,
        why_str,
        result.symbol_id,
        result.file_path,
        lines
    )
}

fn split_symbol_id(symbol_id: &str, file_path: &str) -> (String, String) {
    if let Some(rest) = symbol_id.strip_prefix("file:") {
        let display_name = Path::new(rest)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(rest);
        return ("file".to_string(), display_name.to_string());
    }

    // New format: kind:package:SymbolName or kind:package:Receiver.MethodName
    let parts: Vec<&str> = symbol_id.split(':').collect();
    if parts.len() >= 3 {
        let kind = parts[0];
        let symbol_name = parts.last().unwrap_or(&"");
        return (kind.to_string(), symbol_name.to_string());
    }

    // Fallback for old format or unexpected formats
    let mut tail = symbol_id.rsplitn(2, ':');
    let symbol_name = tail.next().unwrap_or(symbol_id);
    if let Some(head) = tail.next() {
        let mut head_parts = head.splitn(2, ':');
        let kind = head_parts.next().unwrap_or("symbol");
        return (kind.to_string(), symbol_name.to_string());
    }

    let fallback = Path::new(file_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(symbol_id);
    ("symbol".to_string(), fallback.to_string())
}
