use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "obsidian", about = "Query and navigate Obsidian vaults")]
pub struct Cli {
    /// Path to the vault directory. Defaults to current directory.
    #[arg(
        long,
        short = 'v',
        global = true,
        env = "OBSIDIAN_VAULT",
        default_value = "."
    )]
    pub vault: PathBuf,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Search for notes in the vault
    Search(SearchArgs),
    /// Find notes that link to a given note
    Backlinks(BacklinksArgs),
}

#[derive(clap::Args)]
pub struct SearchArgs {
    /// Filter by tag (AND semantics, repeatable)
    #[arg(long, short = 't')]
    pub tag: Vec<String>,
    /// Filter by title substring (case-insensitive)
    #[arg(long)]
    pub title: Option<String>,
    /// Filter by glob pattern (OR semantics, repeatable)
    #[arg(long, short = 'g')]
    pub glob: Vec<String>,
    /// Filter by content substring (AND semantics, repeatable)
    #[arg(long, short = 'c')]
    pub content: Vec<String>,
    /// Filter by content regex
    #[arg(long, short = 'r')]
    pub regex: Option<String>,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(clap::Args)]
pub struct BacklinksArgs {
    /// Path to the note (resolved relative to current directory)
    pub note: PathBuf,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(Clone, ValueEnum)]
pub enum OutputFormat {
    Plain,
    Json,
}
