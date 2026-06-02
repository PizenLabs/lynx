use anyhow::Result;
use clap::{Parser, Subcommand};
use lynx_core::Lynx;
use std::path::PathBuf;

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
    },
    /// Search the index
    Search { query: String },
    /// Resolve a symbol by name
    Resolve { name: String },
    /// Find related implementations
    Related { location: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let lynx = Lynx::new(&cli.storage_path).await?;

    match cli.command {
        Commands::Index { path } => {
            println!("Indexing repository at {:?}", path);
            lynx.index_repository(&path).await?;
            println!("Indexing complete.");
        }
        Commands::Search { query } => {
            let results = lynx.search(&query).await?;
            if results.is_empty() {
                println!("No results found.");
            } else {
                for result in results {
                    println!(
                        "[{:.4}] {}:{}-{} - {}",
                        result.score,
                        result.file_path,
                        result.start_line,
                        result.end_line,
                        result.symbol_id
                    );
                }
            }
        }
        Commands::Resolve { name } => {
            let results = lynx.resolve_symbol(&name).await?;
            if results.is_empty() {
                println!("No symbols found.");
            } else {
                for result in results {
                    println!(
                        "[symbol] {}:{}-{} - {}",
                        result.file_path, result.start_line, result.end_line, result.symbol_id
                    );
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
                    println!(
                        "[{:.4}] {}:{}-{} - {}",
                        result.score,
                        result.file_path,
                        result.start_line,
                        result.end_line,
                        result.symbol_id
                    );
                }
            }
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
