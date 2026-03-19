use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "obsidian", about = "Query and navigate Obsidian vaults")]
pub struct Cli {
    /// Path to the vault directory. Defaults to current directory.
    #[arg(long, short = 'v', global = true, env = "OBSIDIAN_VAULT", default_value = ".")]
    pub vault: PathBuf,
    /// Force color output even when not writing to a TTY
    #[arg(long, global = true)]
    pub color: bool,
    /// Disable color output
    #[arg(long, global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Search for notes in the vault
    Search(SearchArgs),
    /// Work with individual notes
    Note(NoteArgs),
    /// Work with tags across the vault
    Tags(TagsArgs),
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
    /// Filter by exact note ID match
    #[arg(long)]
    pub id: Option<String>,
    /// Filter by content regex
    #[arg(long, short = 'r')]
    pub regex: Option<String>,
    /// Sort order for results
    #[arg(long, short = 's', default_value = "path-asc")]
    pub sort: SortOrder,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(clap::Args)]
pub struct BacklinksArgs {
    /// Path to the note (resolved relative to current directory)
    pub note: PathBuf,
    /// Sort order for results
    #[arg(long, short = 's', default_value = "path-asc")]
    pub sort: SortOrder,
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

#[derive(clap::Args)]
pub struct NoteArgs {
    #[command(subcommand)]
    pub subcommand: NoteCommand,
}

#[derive(Subcommand)]
pub enum NoteCommand {
    /// Find notes that link to a given note
    Backlinks(BacklinksArgs),
    /// Rename a note and update all backlinks
    Rename(RenameArgs),
    /// Update fields of a note
    Update(UpdateArgs),
}

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// Path to the note (resolved relative to current directory).
    /// If omitted, note paths are read from stdin (one per line).
    pub note: Option<PathBuf>,
    /// Add tag(s) to frontmatter (repeatable)
    #[arg(long, short = 't')]
    pub tag: Vec<String>,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(Clone, ValueEnum)]
pub enum OutputFormat {
    Plain,
    Json,
}

#[derive(Clone, ValueEnum, Default)]
pub enum SortOrder {
    #[default]
    PathAsc,
    PathDesc,
    ModifiedAsc,
    ModifiedDesc,
}

#[derive(clap::Args)]
pub struct TagsArgs {
    #[command(subcommand)]
    pub subcommand: TagsCommand,
}

#[derive(Subcommand)]
pub enum TagsCommand {
    /// Find all occurrences of the given tags across the vault
    Search(TagsSearchArgs),
    /// List all tags used across the vault
    List(TagsListArgs),
}

#[derive(clap::Args)]
pub struct TagsSearchArgs {
    /// Tags to search for (OR semantics — occurrences of any given tag are shown)
    #[arg(required = true)]
    pub tags: Vec<String>,
    /// Sort order for results
    #[arg(long, short = 's', default_value = "path-asc")]
    pub sort: SortOrder,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(clap::Args)]
pub struct TagsListArgs {
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}
