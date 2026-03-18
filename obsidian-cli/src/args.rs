use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "obsidian", about = "Query and navigate Obsidian vaults")]
pub struct Cli {
    /// Path to the vault directory. Defaults to current directory.
    #[arg(long, short = 'v', global = true, env = "OBSIDIAN_VAULT", default_value = ".")]
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
    /// Rename a note and update all backlinks
    Rename(RenameArgs),
}

#[derive(clap::Args)]
pub struct SearchArgs {
    /// Filter by tag (AND semantics, repeatable)
    #[arg(long, short = 't')]
    pub tag: Vec<String>,
    /// Filter by title substring, case-insensitive (OR semantics, repeatable)
    #[arg(long)]
    pub title_contains: Vec<String>,
    /// Filter by exact alias, case-insensitive (OR semantics, repeatable)
    #[arg(long, short = 'a')]
    pub alias: Vec<String>,
    /// Filter by alias substring, case-insensitive (OR semantics, repeatable)
    #[arg(long)]
    pub alias_contains: Vec<String>,
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

#[derive(clap::Args)]
pub struct RenameArgs {
    /// Path to the note to rename (resolved relative to current directory)
    pub note: PathBuf,
    /// New path for the note (resolved relative to current directory, .md added if omitted)
    pub new_path: PathBuf,
    /// Preview what would change without modifying any files
    #[arg(long)]
    pub dry_run: bool,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(Clone, ValueEnum)]
pub enum OutputFormat {
    Plain,
    Json,
}
