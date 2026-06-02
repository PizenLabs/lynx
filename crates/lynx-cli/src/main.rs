use anyhow::Result;
use clap::{Parser, Subcommand};
use lynx_core::Lynx;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "lynx")]
#[command(about = "Lynx: Discovery Engine for AI-Native Software Engineering", long_about = None)]
struct Cli {
    #[arg(short, long, default_value = ".lynx")]
    storage_path: PathBuf,

    #[subcommand]
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
    Search {
        query: String,
    },
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
    }

    Ok(())
}
