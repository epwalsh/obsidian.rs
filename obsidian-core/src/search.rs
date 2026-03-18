use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use regex::Regex;
use walkdir::WalkDir;

use crate::{Note, NoteError, SearchError};

/// A composable query for filtering notes in a vault.
///
/// # Filter semantics
/// - [`glob`](SearchQuery::glob): OR — note's relative path matches any pattern
/// - [`has_tag`](SearchQuery::has_tag): AND — note must have all specified tags
/// - [`has_alias`](SearchQuery::has_alias): OR — note must have at least one specified alias
/// - [`content_contains`](SearchQuery::content_contains): AND — body must contain all strings
/// - [`title_contains`](SearchQuery::title_contains) / [`id`](SearchQuery::id) /
///   [`content_matches`](SearchQuery::content_matches): last call wins
pub struct SearchQuery {
    root: PathBuf,
    globs: Vec<String>,
    id: Option<String>,
    tags: Vec<String>,
    title_contains: Option<String>,
    aliases: Vec<String>,
    content_strings: Vec<String>,
    content_regex: Option<String>,
}

impl SearchQuery {
    pub fn new(root: impl AsRef<Path>) -> Self {
        SearchQuery {
            root: root.as_ref().to_path_buf(),
            globs: Vec::new(),
            id: None,
            tags: Vec::new(),
            title_contains: None,
            aliases: Vec::new(),
            content_strings: Vec::new(),
            content_regex: None,
        }
    }

    /// Filter by glob pattern matched against the note's path relative to the vault root.
    /// Multiple calls use OR semantics: a note passes if it matches any pattern.
    pub fn glob(mut self, pattern: impl Into<String>) -> Self {
        self.globs.push(pattern.into());
        self
    }

    /// Filter by exact id match. Last call wins.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Note must have this tag. Multiple calls use AND semantics.
    pub fn has_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Case-insensitive substring match on note title. Last call wins.
    pub fn title_contains(mut self, s: impl Into<String>) -> Self {
        self.title_contains = Some(s.into());
        self
    }

    /// Note must have at least one of the specified aliases. Multiple calls use OR semantics.
    pub fn has_alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// Note body must contain this string. Multiple calls use AND semantics.
    pub fn content_contains(mut self, s: impl Into<String>) -> Self {
        self.content_strings.push(s.into());
        self
    }

    /// Regex match against note body. Last call wins.
    pub fn content_matches(mut self, pattern: impl Into<String>) -> Self {
        self.content_regex = Some(pattern.into());
        self
    }

    /// Execute the query, returning matching notes.
    ///
    /// Returns `Err` if any glob or regex pattern is invalid.
    /// Each inner `Err` represents an I/O failure loading a specific note.
    pub fn execute(self) -> Result<Vec<Result<Note, NoteError>>, SearchError> {
        let SearchQuery {
            root,
            globs,
            id,
            tags,
            title_contains,
            aliases,
            content_strings,
            content_regex,
        } = self;

        let glob_set = build_glob_set(&globs)?;
        let regex = content_regex.as_deref().map(Regex::new).transpose()?;

        let has_globs = !globs.is_empty();
        let paths: Vec<PathBuf> = find_note_paths(&root)
            .filter(|path| {
                if !has_globs {
                    return true;
                }
                let rel = path.strip_prefix(&root).unwrap_or(path);
                glob_set.is_match(rel)
            })
            .collect();

        let results = paths
            .into_par_iter()
            .filter_map(|path| -> Option<Result<Note, NoteError>> {
                let note = match Note::from_path(&path) {
                    Ok(n) => n,
                    Err(e) => return Some(Err(e)),
                };

                if let Some(ref expected_id) = id
                    && note.id != *expected_id
                {
                    return None;
                }

                if !tags.iter().all(|t| note.tags.contains(t)) {
                    return None;
                }

                if let Some(ref substr) = title_contains {
                    match &note.title {
                        Some(title) if title.to_lowercase().contains(&substr.to_lowercase()) => {}
                        _ => return None,
                    }
                }

                if !aliases.is_empty() && !aliases.iter().any(|a| note.aliases.contains(a)) {
                    return None;
                }

                if !content_strings
                    .iter()
                    .all(|s| note.content.contains(s.as_str()))
                {
                    return None;
                }

                if let Some(ref re) = regex
                    && !re.is_match(&note.content)
                {
                    return None;
                }

                Some(Ok(note))
            })
            .collect();

        Ok(results)
    }
}

fn build_glob_set(patterns: &[String]) -> Result<GlobSet, SearchError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern).literal_separator(true).build()?;
        builder.add(glob);
    }
    Ok(builder.build()?)
}

/// Returns an iterator over all `.md` file paths found recursively under `root`.
pub fn find_note_paths(root: impl AsRef<Path>) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().and_then(|e| e.to_str()) == Some("md")
        })
        .map(|entry| entry.into_path())
}

/// Loads all notes found recursively under `root` in parallel.
pub fn find_notes(root: impl AsRef<Path>) -> Vec<Result<Note, NoteError>> {
    find_note_paths(root)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_note(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn unwrap_notes(results: Vec<Result<Note, crate::NoteError>>) -> Vec<Note> {
        results.into_iter().map(|r| r.unwrap()).collect()
    }

    fn sorted_ids(notes: Vec<Note>) -> Vec<String> {
        let mut ids: Vec<String> = notes.into_iter().map(|n| n.id).collect();
        ids.sort();
        ids
    }

    #[test]
    fn glob_filters_by_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        write_note(&dir.path().join("root.md"), "root note");
        write_note(&subdir.join("sub.md"), "sub note");

        let results = SearchQuery::new(dir.path())
            .glob("subdir/**")
            .execute()
            .unwrap();
        let notes = unwrap_notes(results);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].path.ends_with("subdir/sub.md"));
    }

    #[test]
    fn multiple_globs_or_semantics() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["a", "b", "c"] {
            write_note(&dir.path().join(d).join("note.md"), d);
        }

        let notes = unwrap_notes(
            SearchQuery::new(dir.path())
                .glob("a/**")
                .glob("b/**")
                .execute()
                .unwrap(),
        );
        let mut paths: Vec<String> = notes
            .iter()
            .map(|n| {
                n.path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        paths.sort();
        assert_eq!(paths, vec!["a", "b"]);
    }

    #[test]
    fn glob_no_match_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("note.md"), "content");

        let notes = unwrap_notes(
            SearchQuery::new(dir.path())
                .glob("nonexistent/**")
                .execute()
                .unwrap(),
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn has_tag_single() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("tagged.md"),
            "---\ntags: [rust]\n---\nContent.",
        );
        write_note(&dir.path().join("untagged.md"), "No tags here.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .has_tag("rust")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["tagged"]);
    }

    #[test]
    fn has_tag_and_semantics() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("both.md"),
            "---\ntags: [rust, obsidian]\n---\nContent.",
        );
        write_note(
            &dir.path().join("one.md"),
            "---\ntags: [rust]\n---\nContent.",
        );
        write_note(&dir.path().join("none.md"), "No tags.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .has_tag("rust")
                .has_tag("obsidian")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["both"]);
    }

    #[test]
    fn has_tag_no_match() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("note.md"),
            "---\ntags: [rust]\n---\nContent.",
        );

        let notes = unwrap_notes(
            SearchQuery::new(dir.path())
                .has_tag("python")
                .execute()
                .unwrap(),
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn id_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("note-a.md"),
            "---\nid: my-special-id\n---\nContent.",
        );
        write_note(&dir.path().join("note-b.md"), "Other note.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .id("my-special-id")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["my-special-id"]);
    }

    #[test]
    fn title_contains_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("match.md"),
            "# Rust Programming\n\nContent.",
        );
        write_note(
            &dir.path().join("no-match.md"),
            "# Python Notes\n\nContent.",
        );

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .title_contains("rust")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["match"]);
    }

    #[test]
    fn title_contains_no_title_excluded() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("no-title.md"),
            "Just plain content, no heading.",
        );
        write_note(&dir.path().join("has-title.md"), "# My Title\n\nContent.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .title_contains("my")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["has-title"]);
    }

    #[test]
    fn has_alias_or_semantics() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("alpha.md"),
            "---\ntitle: Note Alpha\naliases: [alpha-alias]\n---\nContent.",
        );
        write_note(
            &dir.path().join("beta.md"),
            "---\ntitle: Note Beta\naliases: [beta-alias]\n---\nContent.",
        );
        write_note(&dir.path().join("gamma.md"), "# Gamma\n\nContent.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .has_alias("alpha-alias")
                .has_alias("beta-alias")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn content_contains_single() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("match.md"), "This note mentions ferris.");
        write_note(
            &dir.path().join("no-match.md"),
            "This note mentions nothing special.",
        );

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .content_contains("ferris")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["match"]);
    }

    #[test]
    fn content_contains_and_semantics() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("both.md"), "Contains alpha and beta.");
        write_note(&dir.path().join("one.md"), "Contains alpha only.");
        write_note(&dir.path().join("none.md"), "Contains neither.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .content_contains("alpha")
                .content_contains("beta")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["both"]);
    }

    #[test]
    fn content_matches_regex() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("match.md"), "Score: 42 points");
        write_note(&dir.path().join("no-match.md"), "No numbers here.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .content_matches(r"\d+")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["match"]);
    }

    #[test]
    fn content_matches_invalid_regex_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = SearchQuery::new(dir.path())
            .content_matches(r"[invalid")
            .execute();
        assert!(matches!(result, Err(SearchError::InvalidRegex(_))));
    }

    #[test]
    fn invalid_glob_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = SearchQuery::new(dir.path()).glob("[invalid").execute();
        assert!(matches!(result, Err(SearchError::InvalidGlob(_))));
    }

    #[test]
    fn combined_glob_tag_content() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("notes");

        write_note(
            &subdir.join("target.md"),
            "---\ntags: [rust]\n---\nThis note mentions ferris.",
        );
        write_note(
            &subdir.join("wrong-content.md"),
            "---\ntags: [rust]\n---\nNo special word.",
        );
        write_note(
            &dir.path().join("wrong-glob.md"),
            "---\ntags: [rust]\n---\nThis note mentions ferris.",
        );

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .glob("notes/**")
                .has_tag("rust")
                .content_contains("ferris")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["target"]);
    }

    #[test]
    fn empty_query_returns_all_notes() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("a.md"), "Note A.");
        write_note(&dir.path().join("b.md"), "Note B.");
        write_note(&dir.path().join("c.md"), "Note C.");

        let via_query = unwrap_notes(SearchQuery::new(dir.path()).execute().unwrap());
        let via_find = find_notes(dir.path())
            .into_iter()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>();

        assert_eq!(via_query.len(), via_find.len());
        assert_eq!(via_query.len(), 3);
    }

    #[test]
    fn vault_search_convenience() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("tagged.md"),
            "---\ntags: [my-tag]\n---\nContent.",
        );
        write_note(&dir.path().join("untagged.md"), "No tags.");

        let vault = crate::Vault::open(dir.path()).unwrap();
        let ids = sorted_ids(unwrap_notes(
            vault.search().has_tag("my-tag").execute().unwrap(),
        ));
        assert_eq!(ids, vec!["tagged"]);
    }
}
