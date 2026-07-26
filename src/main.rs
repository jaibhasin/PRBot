use anyhow::Result;
use clap::{Parser, Subcommand};

mod agents;
mod config;
mod github;
mod llm;
mod reporting;
mod repository;
mod review;
mod types;

/// PRBot - multi-agent PR reviewer for GitHub Actions.
#[derive(Debug, Parser)]
#[command(name = "prbot", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run PR review agents and post feedback.
    Review(Box<review::ReviewArgs>),
    /// Print build/runtime info (useful for Action smoke tests).
    Version,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Review(args) => review::run(*args).await,
        Commands::Version => {
            println!("prbot {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
