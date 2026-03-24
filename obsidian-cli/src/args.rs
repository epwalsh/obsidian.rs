use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "obsidian", about = "Query and navigate Obsidian vaults")]
pub struct Cli {
    /// Path to the vault directory. Defaults to the nearest parent directory containing
    /// '.obsidian/', or the current directory if none is found.
    #[arg(long, short = 'v', global = true, env = "OBSIDIAN_VAULT")]
    pub vault: Option<PathBuf>,
    /// Force color output even when not writing to a TTY
    #[arg(long, global = true)]
    pub color: bool,
    /// Disable color output
    #[arg(long, global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Command {
    /// Search for notes in the vault
    Search(SearchArgs),
    /// Work with individual notes
    Note(NoteArgs),
    /// Work with tags across the vault
    Tags(TagsArgs),
    /// Check vault health: report duplicate IDs/aliases and broken links
    Check(CheckArgs),
}

#[derive(clap::Args)]
pub struct CheckArgs {
    /// Ignore notes matching this glob pattern (matched against vault-relative path, repeatable)
    #[arg(long, short = 'i')]
    pub ignore: Vec<String>,
}

#[derive(clap::Args)]
pub struct SearchArgs {
    /// Only include notes whose path matches one of these glob patterns (matched against vault-relative path, repeatable)
    #[arg(long)]
    pub glob: Vec<String>,
    /// Same as --glob but with global OR semantics
    #[arg(long)]
    pub or_glob: Vec<String>,
    /// Filter by exact note ID match (AND semantics)
    #[arg(long)]
    pub id: Option<String>,
    /// Filter by exact note ID match, case-sensitive by default (OR semantics, repeatable)
    #[arg(long)]
    pub or_id: Vec<String>,
    /// Filter by tag, case-sensitive by default (AND semantics, repeatable)
    #[arg(long)]
    pub tag: Vec<String>,
    /// Filter by tag, case-sensitive by default (OR semantics, repeatable)
    #[arg(long)]
    pub or_tag: Vec<String>,
    /// Filter by title substring, smart case-sensitive by default (AND semantics, repeatable)
    #[arg(long)]
    pub title_contains: Vec<String>,
    /// Filter by title substring, smart case-sensitive by default (OR semantics, repeatable)
    #[arg(long)]
    pub or_title_contains: Vec<String>,
    /// Filter by exact alias, smart case-sensitive by default (AND semantics, repeatable)
    #[arg(long)]
    pub alias: Vec<String>,
    /// Filter by exact alias, smart case-sensitive by default (OR semantics, repeatable)
    #[arg(long)]
    pub or_alias: Vec<String>,
    /// Filter by alias substring, smart case-sensitive by default (AND semantics, repeatable)
    #[arg(long)]
    pub alias_contains: Vec<String>,
    /// Filter by alias substring, smart case-sensitive by default (OR semantics, repeatable)
    #[arg(long)]
    pub or_alias_contains: Vec<String>,
    /// Filter by content substring, smart case-sensitive by default (AND semantics, repeatable)
    #[arg(long)]
    pub content_contains: Vec<String>,
    /// Filter by content substring, smart case-sensitive by default (OR semantics, repeatable)
    #[arg(long)]
    pub or_content_contains: Vec<String>,
    /// Filter by content pattern, smart case-sensitive by default (AND semantics, repeatable).
    /// See https://docs.rs/regex/latest/regex/#syntax.
    #[arg(long)]
    pub content_matches: Vec<String>,
    /// Filter by content pattern, smart case-sensitive by default (OR semantics, repeatable).
    /// See https://docs.rs/regex/latest/regex/#syntax.
    #[arg(long)]
    pub or_content_matches: Vec<String>,
    /// Execute the search case sensitive. By default, title, alias, and content filters are
    /// smart case-insensitive while ID and tag filters are case-sensitive.
    /// This flag overrides -i/--ignore-case and -S/--smart-case.
    #[arg(long, short = 's')]
    pub case_sensitive: bool,
    /// Execute the search case insensitive. This flag overrides -S/--smart-case.
    #[arg(long, short = 'i')]
    pub ignore_case: bool,
    /// Search case insensitively for patterns that are all lowercase, otherwise search case
    /// sensitively.
    #[arg(long, short = 'S')]
    pub smart_case: bool,
    /// Sort order for results
    #[arg(long, default_value = "path-asc")]
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
pub struct MergeArgs {
    /// One or more source notes followed by the destination note.
    /// All paths are resolved relative to the current directory.
    /// The last path is the destination; all preceding paths are sources.
    /// Sources are merged into the destination (which is created if it doesn't exist) and deleted.
    #[arg(name = "PATH", required = true, num_args = 2..)]
    pub paths: Vec<PathBuf>,
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
    /// Resolve a note from a path, ID, or alias.
    Resolve(ResolveArgs),
    /// Read contents/frontmatter of a note
    Read(ReadArgs),
    /// Write a new note
    Write(WriteArgs),
    /// Find notes that link to a given note
    Backlinks(BacklinksArgs),
    /// Merge two or more notes into a single destination note
    Merge(MergeArgs),
    /// Patch the content of a note by replacing one exact string with another
    Patch(PatchArgs),
    /// Rename a note and update all backlinks
    Rename(RenameArgs),
    /// Update frontmatter metadata fields of a note
    Update(UpdateArgs),
}

#[derive(clap::Args)]
pub struct ResolveArgs {
    /// Path, ID, or alias of the note to resolve
    pub note: String,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(clap::Args)]
pub struct ReadArgs {
    /// Path to the note to read (resolved relative to current directory)
    pub note: PathBuf,
    /// Include frontmatter in the output
    #[arg(long)]
    pub frontmatter: bool,
    /// Exclude content from the output (--frontmatter is assumed if this is set)
    #[arg(long)]
    pub no_content: bool,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(clap::Args)]
pub struct WriteArgs {
    /// Path to the note to write (resolved relative to the vault root or current directory, .md added if omitted)
    pub note: PathBuf,
    /// Content to write to the note. If omitted, content is read from stdin.
    pub content: Option<String>,
    /// A title for the note if one can't be inferred from the content
    pub title: Option<String>,
    /// Add tag(s) to frontmatter (repeatable)
    #[arg(long, short = 't')]
    pub tag: Vec<String>,
    /// Add alias(es) to frontmatter (repeatable)
    #[arg(long, short = 'a')]
    pub alias: Vec<String>,
    /// Force overwrite any existing note
    #[arg(long)]
    pub force: bool,
    /// Output format
    #[arg(long, short = 'f', default_value = "plain")]
    pub format: OutputFormat,
}

#[derive(clap::Args)]
pub struct PatchArgs {
    /// Path to the note (resolved relative to current directory)
    pub note: PathBuf,
    /// The exact string to find (must appear exactly once in the note)
    #[arg(long)]
    pub old_string: String,
    /// The string to replace it with
    #[arg(long)]
    pub new_string: String,
}

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// Path to the note (resolved relative to vault root or current directory).
    /// If omitted, note paths are read from stdin (one per line).
    pub note: Option<PathBuf>,
    /// Add tag(s) to frontmatter (repeatable)
    #[arg(long, short = 't')]
    pub add_tag: Vec<String>,
    /// Remove tag(s) from frontmatter (repeatable)
    #[arg(long)]
    pub rm_tag: Vec<String>,
    /// Add alias(es) to frontmatter (repeatable)
    #[arg(long, short = 'a')]
    pub add_alias: Vec<String>,
    /// Set a field in the frontmatter to a value (repeatable, --set key=value). The value is
    /// parsed as YAML, so it can be a string (with or without quotes), number, boolean, list, map, or null.
    /// If the field already exits, it will be overwritten. To remove a field, set it to null (e.g. --set myfield=null).
    #[arg(long)]
    pub set: Vec<String>,
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
