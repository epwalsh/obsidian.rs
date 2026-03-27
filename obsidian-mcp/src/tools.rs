use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SortOrder {
    PathAsc,
    PathDesc,
    ModifiedAsc,
    ModifiedDesc,
    CreatedAsc,
    CreatedDesc,
}

impl From<SortOrder> for obsidian_core::SortOrder {
    fn from(s: SortOrder) -> Self {
        match s {
            SortOrder::PathAsc => obsidian_core::SortOrder::PathAsc,
            SortOrder::PathDesc => obsidian_core::SortOrder::PathDesc,
            SortOrder::ModifiedAsc => obsidian_core::SortOrder::ModifiedAsc,
            SortOrder::ModifiedDesc => obsidian_core::SortOrder::ModifiedDesc,
            SortOrder::CreatedAsc => obsidian_core::SortOrder::CreatedAsc,
            SortOrder::CreatedDesc => obsidian_core::SortOrder::CreatedDesc,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadNoteParams {
    #[schemars(description = "Note identifier: a file path (absolute or vault-relative), note ID, or alias")]
    pub note: String,
    #[schemars(description = "Whether to include the body content in the response (default: true)")]
    pub include_content: Option<bool>,
    #[schemars(description = "Whether to include frontmatter fields in the response (default: true)")]
    pub include_frontmatter: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListNotesParams {
    #[schemars(
        description = "Sort order for results. One of: path-asc, path-desc, modified-asc, modified-desc, created-asc, created-desc"
    )]
    pub sort: Option<SortOrder>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteNoteParams {
    #[schemars(description = "Path for the note, relative to the vault root. Extension .md is added if omitted.")]
    pub path: String,
    #[schemars(description = "Full markdown content of the note, including any YAML frontmatter block")]
    pub content: String,
    #[schemars(description = "Title for the note. Overrides any title parsed from the content.")]
    pub title: Option<String>,
    #[schemars(description = "Tags to add to the note's frontmatter")]
    pub tags: Option<Vec<String>>,
    #[schemars(description = "Aliases to add to the note's frontmatter")]
    pub aliases: Option<Vec<String>>,
    #[schemars(description = "If true, overwrite an existing note at this path (default: false)")]
    pub force: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PatchNoteParams {
    #[schemars(description = "Note identifier: a file path (absolute or vault-relative), note ID, or alias")]
    pub note: String,
    #[schemars(description = "The exact string to find in the note. Must appear exactly once.")]
    pub old_string: String,
    #[schemars(description = "The string to replace old_string with")]
    pub new_string: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateNoteParams {
    #[schemars(description = "Note identifier: a file path (absolute or vault-relative), note ID, or alias")]
    pub note: String,
    #[schemars(description = "Tags to add to the note's frontmatter")]
    pub add_tags: Option<Vec<String>>,
    #[schemars(description = "Tags to remove from the note's frontmatter")]
    pub remove_tags: Option<Vec<String>>,
    #[schemars(description = "Aliases to add to the note's frontmatter")]
    pub add_aliases: Option<Vec<String>>,
    #[schemars(
        description = "Arbitrary frontmatter fields to set. Values are JSON: strings, numbers, booleans, arrays, or null. A null value removes the field. Reserved keys 'id', 'title', 'aliases', and 'tags' are rejected."
    )]
    pub set_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchNotesParams {
    #[schemars(
        description = "Filter by tags. All listed tags must be present on a note (AND semantics). Uses exact match."
    )]
    pub tags: Option<Vec<String>>,
    #[schemars(description = "Filter notes whose title contains this substring (case-insensitive)")]
    pub title_contains: Option<String>,
    #[schemars(description = "Filter notes whose body content contains this substring (case-insensitive)")]
    pub content_contains: Option<String>,
    #[schemars(description = "Filter by glob pattern matched against vault-relative path (e.g. 'notes/**')")]
    pub glob: Option<String>,
    #[schemars(description = "Filter by exact note ID")]
    pub id: Option<String>,
    #[schemars(description = "Filter by exact alias")]
    pub alias: Option<String>,
    #[schemars(
        description = "Sort order for results. One of: path-asc, path-desc, modified-asc, modified-desc, created-asc, created-desc"
    )]
    pub sort: Option<SortOrder>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenameNoteParams {
    #[schemars(description = "Note identifier: a file path (absolute or vault-relative), note ID, or alias")]
    pub note: String,
    #[schemars(description = "New path for the note, relative to the vault root. Extension .md is added if omitted.")]
    pub new_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTagsParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchByTagParams {
    #[schemars(description = "Tags to search for. Also matches sub-tags: 'workout' matches 'workout/upper-body'.")]
    pub tags: Vec<String>,
    #[schemars(
        description = "Sort order for results by source note. One of: path-asc, path-desc, modified-asc, modified-desc, created-asc, created-desc"
    )]
    pub sort: Option<SortOrder>,
}
