use std::path::PathBuf;

/// Health report produced by [`Vault::check`](crate::Vault::check).
pub struct VaultHealthReport {
    /// Total number of notes scanned.
    pub note_count: usize,
    /// Groups of notes that share the same ID, sorted by ID.
    pub duplicate_ids: Vec<DuplicateId>,
    /// Groups of notes that share the same alias (case-insensitive), sorted by alias.
    pub duplicate_aliases: Vec<DuplicateAlias>,
    /// Broken links found across the vault, sorted by source path then line.
    pub broken_links: Vec<BrokenLink>,
}

impl VaultHealthReport {
    /// Returns `true` if any health issues were found.
    pub fn has_issues(&self) -> bool {
        !self.duplicate_ids.is_empty() || !self.duplicate_aliases.is_empty() || !self.broken_links.is_empty()
    }
}

/// A group of notes that share the same ID.
pub struct DuplicateId {
    pub id: String,
    /// Notes with this ID, sorted by path.
    pub notes: Vec<NoteRef>,
}

/// A group of notes that share the same alias (compared case-insensitively; stored lowercase).
pub struct DuplicateAlias {
    pub alias: String,
    /// Notes with this alias, sorted by path.
    pub notes: Vec<NoteRef>,
}

/// A note path with its backlink count, used inside duplicate-detection results.
pub struct NoteRef {
    pub path: PathBuf,
    pub backlink_count: usize,
}

/// A broken link found in a note.
pub struct BrokenLink {
    pub source_path: PathBuf,
    /// 1-indexed line number of the link within the note.
    pub line: usize,
    /// Formatted link text, e.g. `[[target]]` or `[...](url.md)`.
    pub text: String,
}
