use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use rayon::prelude::*;
use regex::Regex;

use crate::{Location, Note, NoteError, SearchError};

#[derive(Debug, Clone, Copy)]
enum CaseSensitivity {
    Sensitive,
    Ignore,
    Smart,
}

/// A composable query for filtering notes in a vault.
///
/// # Filter semantics
/// - [`and_glob`](SearchQuery::and_glob): AND — note's relative path matches one of these patterns
/// - [`or_glob`](SearchQuery::or_glob): OR — note's relative path matches one of these patterns
/// - [`and_has_id`](SearchQuery::and_has_id): OR — has this ID
/// - [`or_has_id`](SearchQuery::or_has_id): OR — has one of these IDs
/// - [`and_has_tag`](SearchQuery::and_has_tag): AND — note has all of these tags
/// - [`or_has_tag`](SearchQuery::or_has_tag): OR — note has one of these tag
/// - [`and_has_alias`](SearchQuery::and_has_alias): AND — note has all of these aliases (case insensitive)
/// - [`or_has_alias`](SearchQuery::or_has_alias): OR — note has any of these aliases (case insensitive)
/// - [`and_title_contains`](SearchQuery::and_title_contains): AND — title contains all of these substrings
/// - [`or_title_contains`](SearchQuery::or_title_contains): OR — title contains any of these substrings
/// - [`and_alias_contains`](SearchQuery::and_alias_contains): AND — all of these substrings must match some alias
/// - [`or_alias_contains`](SearchQuery::or_alias_contains): OR — one of these substrings matches some alias
/// - [`and_content_contains`](SearchQuery::and_content_contains): AND — content contains all of these substrings (case-sensitive)
/// - [`or_content_contains`](SearchQuery::or_content_contains): OR — content contains any of these substrings (case-sensitive)
/// - [`and_content_matches`](SearchQuery::and_content_matches): AND — content matches all of these patterns
/// - [`or_content_matches`](SearchQuery::or_content_matches): OR — content matches any of these patterns
pub struct SearchQuery {
    root: PathBuf,
    and_globs: Vec<String>,
    or_globs: Vec<String>,
    and_id: Option<String>,
    or_ids: Vec<String>,
    and_tags: Vec<String>,
    or_tags: Vec<String>,
    and_title_contains: Vec<String>,
    or_title_contains: Vec<String>,
    and_aliases: Vec<String>,
    or_aliases: Vec<String>,
    and_alias_contains: Vec<String>,
    or_alias_contains: Vec<String>,
    and_content_contains: Vec<String>,
    or_content_contains: Vec<String>,
    and_content_matches: Vec<String>,
    or_content_matches: Vec<String>,
    case_sensitivity: Option<CaseSensitivity>,
    include_inline_tags: bool,
}

impl SearchQuery {
    pub fn new(root: impl AsRef<Path>) -> Self {
        SearchQuery {
            root: root.as_ref().to_path_buf(),
            and_globs: Vec::new(),
            or_globs: Vec::new(),
            and_id: None,
            or_ids: Vec::new(),
            and_tags: Vec::new(),
            or_tags: Vec::new(),
            and_title_contains: Vec::new(),
            or_title_contains: Vec::new(),
            and_aliases: Vec::new(),
            or_aliases: Vec::new(),
            and_alias_contains: Vec::new(),
            or_alias_contains: Vec::new(),
            and_content_contains: Vec::new(),
            or_content_contains: Vec::new(),
            and_content_matches: Vec::new(),
            or_content_matches: Vec::new(),
            case_sensitivity: None,
            include_inline_tags: false,
        }
    }

    /// Note path must match this glob pattern (matched against the note's path relative to the vault root).
    pub fn and_glob(mut self, pattern: impl Into<String>) -> Self {
        self.and_globs.push(pattern.into());
        self
    }

    /// Note path could match this glob pattern (matched against the note's path relative to the vault root).
    pub fn or_glob(mut self, pattern: impl Into<String>) -> Self {
        self.or_globs.push(pattern.into());
        self
    }

    /// Note must have this ID (case-sensitive by default).
    pub fn and_has_id(mut self, id: impl Into<String>) -> Self {
        self.and_id = Some(id.into());
        self
    }

    /// Note could have this ID (case-sensitive by default).
    pub fn or_has_id(mut self, id: impl Into<String>) -> Self {
        self.or_ids.push(id.into());
        self
    }

    /// Note must have this tag (case-insensitive by default).
    pub fn and_has_tag(mut self, tag: impl Into<String>) -> Self {
        self.and_tags.push(crate::tag::clean_tag(&tag.into()));
        self
    }

    /// Note could have this tag (case-insensitive by default).
    pub fn or_has_tag(mut self, tag: impl Into<String>) -> Self {
        self.or_tags.push(crate::tag::clean_tag(&tag.into()));
        self
    }

    /// Must title must contain this substring (smart case-sensitive by default).
    pub fn and_title_contains(mut self, s: impl Into<String>) -> Self {
        self.and_title_contains.push(s.into());
        self
    }

    /// Must title could contain this substring (smart case-sensitive by default).
    pub fn or_title_contains(mut self, s: impl Into<String>) -> Self {
        self.or_title_contains.push(s.into());
        self
    }

    /// Note must have this alias (smart case-sensitive by default).
    pub fn and_has_alias(mut self, alias: impl Into<String>) -> Self {
        self.and_aliases.push(alias.into());
        self
    }

    /// Note could have this alias (smart case-sensitive by default).
    pub fn or_has_alias(mut self, alias: impl Into<String>) -> Self {
        self.or_aliases.push(alias.into());
        self
    }

    /// Substring must match against any of the note's aliases (smart case-sensitive by default).
    pub fn and_alias_contains(mut self, s: impl Into<String>) -> Self {
        self.and_alias_contains.push(s.into());
        self
    }

    /// Substring could match against any of the note's aliases (smart case-sensitive by default).
    pub fn or_alias_contains(mut self, s: impl Into<String>) -> Self {
        self.or_alias_contains.push(s.into());
        self
    }

    /// Note body must contain this string (smart case-sensitive by default).
    pub fn and_content_contains(mut self, s: impl Into<String>) -> Self {
        self.and_content_contains.push(s.into());
        self
    }

    /// Note body could contain this string (smart case-sensitive by default).
    pub fn or_content_contains(mut self, s: impl Into<String>) -> Self {
        self.or_content_contains.push(s.into());
        self
    }

    /// Regex body must match this pattern (smart case-sensitive by default).
    pub fn and_content_matches(mut self, pattern: impl Into<String>) -> Self {
        self.and_content_matches.push(pattern.into());
        self
    }

    /// Regex body could match this pattern (smart case-sensitive by default).
    pub fn or_content_matches(mut self, pattern: impl Into<String>) -> Self {
        self.or_content_matches.push(pattern.into());
        self
    }

    /// Execute the search case-sensitively.
    pub fn case_sensitive(mut self) -> Self {
        self.case_sensitivity = Some(CaseSensitivity::Sensitive);
        self
    }

    /// Execute the search case-insensitively.
    pub fn ignore_case(mut self) -> Self {
        self.case_sensitivity = Some(CaseSensitivity::Ignore);
        self
    }

    /// Execute the search with smart case sensitivity: case-sensitive if the query contains any uppercase letters,
    /// otherwise case-insensitive.
    pub fn smart_case(mut self) -> Self {
        self.case_sensitivity = Some(CaseSensitivity::Smart);
        self
    }

    pub fn include_inline_tags(mut self) -> Self {
        self.include_inline_tags = true;
        self
    }

    /// Execute the query, returning matching notes.
    ///
    /// Returns `Err` if any glob or regex pattern is invalid.
    /// Each inner `Err` represents an I/O failure loading a specific note.
    pub fn execute(self) -> Result<Vec<Result<Note, NoteError>>, SearchError> {
        let SearchQuery {
            root,
            and_globs,
            or_globs,
            and_id,
            or_ids,
            and_tags,
            or_tags,
            and_title_contains,
            or_title_contains,
            and_aliases,
            or_aliases,
            and_alias_contains,
            or_alias_contains,
            and_content_contains,
            or_content_contains,
            and_content_matches,
            or_content_matches,
            case_sensitivity,
            include_inline_tags,
        } = self;

        let strings_equal = |s: &str, query: &str, cs: CaseSensitivity| match cs {
            CaseSensitivity::Sensitive => s == query,
            CaseSensitivity::Ignore => s.eq_ignore_ascii_case(query),
            CaseSensitivity::Smart => {
                if query.chars().any(|c| c.is_ascii_uppercase()) {
                    s == query
                } else {
                    s.eq_ignore_ascii_case(query)
                }
            }
        };
        let string_contains = |s: &str, query: &str, cs: CaseSensitivity| match cs {
            CaseSensitivity::Sensitive => s.contains(query),
            CaseSensitivity::Ignore => s.to_lowercase().contains(&query.to_lowercase()),
            CaseSensitivity::Smart => {
                if query.chars().any(|c| c.is_ascii_uppercase()) {
                    s.contains(query)
                } else {
                    s.to_lowercase().contains(&query.to_lowercase())
                }
            }
        };
        let compare_tag = |note_tag: &str, query_tag: &str, cs: CaseSensitivity| match cs {
            CaseSensitivity::Sensitive => note_tag == query_tag || note_tag.starts_with(&format!("{query_tag}/")),
            CaseSensitivity::Ignore => {
                note_tag.eq_ignore_ascii_case(query_tag)
                    || note_tag
                        .to_lowercase()
                        .starts_with(&format!("{}/", query_tag.to_lowercase()))
            }
            CaseSensitivity::Smart => {
                if query_tag.chars().any(|c| c.is_ascii_uppercase()) {
                    note_tag == query_tag || note_tag.starts_with(&format!("{query_tag}/"))
                } else {
                    note_tag.eq_ignore_ascii_case(query_tag)
                        || note_tag
                            .to_lowercase()
                            .starts_with(&format!("{}/", query_tag.to_lowercase()))
                }
            }
        };

        let and_glob_set = build_glob_set(&and_globs)?;
        let or_glob_set = build_glob_set(&or_globs)?;

        let paths: Vec<PathBuf> = find_note_paths(&root)
            .filter(|path| {
                if and_globs.is_empty() {
                    return true;
                }
                let rel = path.strip_prefix(&root).unwrap_or(path);
                and_glob_set.is_match(rel)
            })
            .collect();

        let mut and_regexes: Vec<Regex> = Vec::new();
        for pattern in and_content_matches {
            let pattern = match case_sensitivity.unwrap_or(CaseSensitivity::Smart) {
                CaseSensitivity::Sensitive => pattern,
                CaseSensitivity::Ignore => format!("(?i:{pattern})"),
                CaseSensitivity::Smart => {
                    if pattern.chars().any(|c| c.is_ascii_uppercase()) {
                        pattern
                    } else {
                        format!("(?i:{pattern})")
                    }
                }
            };
            let re = Regex::new(&pattern).map_err(SearchError::InvalidRegex)?;
            and_regexes.push(re);
        }

        let mut or_regexes: Vec<Regex> = Vec::new();
        for pattern in or_content_matches {
            let pattern = match case_sensitivity.unwrap_or(CaseSensitivity::Smart) {
                CaseSensitivity::Sensitive => pattern,
                CaseSensitivity::Ignore => format!("(?i){pattern}"),
                CaseSensitivity::Smart => {
                    if pattern.chars().any(|c| c.is_ascii_uppercase()) {
                        pattern
                    } else {
                        format!("(?i){pattern}")
                    }
                }
            };
            let re = Regex::new(&pattern).map_err(SearchError::InvalidRegex)?;
            or_regexes.push(re);
        }

        let needs_content = !and_content_contains.is_empty()
            || !or_content_contains.is_empty()
            || !and_regexes.is_empty()
            || !or_regexes.is_empty();
        let has_or_filters = !or_globs.is_empty()
            || !or_ids.is_empty()
            || !or_tags.is_empty()
            || !or_title_contains.is_empty()
            || !or_aliases.is_empty()
            || !or_alias_contains.is_empty()
            || !or_content_contains.is_empty()
            || !or_regexes.is_empty();
        let has_filters = has_or_filters
            || and_id.is_some()
            || !and_tags.is_empty()
            || !and_title_contains.is_empty()
            || !and_aliases.is_empty()
            || !and_alias_contains.is_empty()
            || !and_content_contains.is_empty()
            || !and_regexes.is_empty();

        let results = paths
            .into_par_iter()
            .filter_map(|path| -> Option<Result<Note, NoteError>> {
                let rel = path.strip_prefix(&root).unwrap_or(&path);
                let load = if needs_content {
                    Note::from_path_with_content(&path)
                } else {
                    Note::from_path(&path)
                };
                let note = match load {
                    Ok(n) => n,
                    Err(e) => return Some(Err(e)),
                };

                if !has_filters {
                    return Some(Ok(note));
                }

                // ---------------------------------------------------------------------
                // Begin AND filters. Exclude note immediately if it fails any of these.
                // ---------------------------------------------------------------------
                if let Some(ref expected_id) = and_id
                    && !strings_equal(
                        &note.id,
                        expected_id,
                        case_sensitivity.unwrap_or(CaseSensitivity::Sensitive),
                    )
                {
                    return None;
                }

                if !and_tags.is_empty()
                    && !and_tags.iter().all(|t| {
                        note.tags.iter().any(|lt| {
                            (include_inline_tags || matches!(lt.location, Location::Frontmatter))
                                && compare_tag(&lt.tag, t, case_sensitivity.unwrap_or(CaseSensitivity::Ignore))
                        })
                    })
                {
                    return None;
                }

                if !and_aliases.is_empty()
                    && !and_aliases.iter().all(|a| {
                        note.aliases
                            .iter()
                            .any(|na| strings_equal(na, a, case_sensitivity.unwrap_or(CaseSensitivity::Smart)))
                    })
                {
                    return None;
                }

                if !and_title_contains.is_empty()
                    && !and_title_contains.iter().all(|substr| {
                        note.title.as_deref().is_some_and(|t| {
                            string_contains(t, substr, case_sensitivity.unwrap_or(CaseSensitivity::Smart))
                        })
                    })
                {
                    return None;
                }

                if !and_alias_contains.is_empty()
                    && !and_alias_contains.iter().all(|substr| {
                        note.aliases
                            .iter()
                            .any(|a| string_contains(a, substr, case_sensitivity.unwrap_or(CaseSensitivity::Smart)))
                    })
                {
                    return None;
                }

                if !and_content_contains.is_empty()
                    && !and_content_contains.iter().all(|s| {
                        string_contains(
                            note.content.as_deref().unwrap(),
                            s,
                            case_sensitivity.unwrap_or(CaseSensitivity::Smart),
                        )
                    })
                {
                    return None;
                }

                if !and_regexes.is_empty()
                    && !and_regexes
                        .iter()
                        .all(|re| re.is_match(note.content.as_deref().unwrap()))
                {
                    return None;
                }

                // --------------------------------------------------------------------------------------------
                // Begin OR filters. Include note if it satisfies any of these (or if there are no OR filters).
                // --------------------------------------------------------------------------------------------
                if !has_or_filters {
                    return Some(Ok(note));
                }

                if !or_globs.is_empty() && or_glob_set.is_match(rel) {
                    return Some(Ok(note));
                }

                if or_ids
                    .iter()
                    .any(|id| strings_equal(&note.id, id, case_sensitivity.unwrap_or(CaseSensitivity::Sensitive)))
                {
                    return Some(Ok(note));
                }

                if or_tags.iter().any(|t| {
                    note.tags.iter().any(|lt| {
                        (include_inline_tags || matches!(lt.location, Location::Frontmatter))
                            && compare_tag(&lt.tag, t, case_sensitivity.unwrap_or(CaseSensitivity::Ignore))
                    })
                }) {
                    return Some(Ok(note));
                }

                if or_title_contains.iter().any(|substr| {
                    note.title
                        .as_deref()
                        .is_some_and(|t| string_contains(t, substr, case_sensitivity.unwrap_or(CaseSensitivity::Smart)))
                }) {
                    return Some(Ok(note));
                }

                if or_aliases.iter().any(|a| {
                    note.aliases
                        .iter()
                        .any(|na| strings_equal(na, a, case_sensitivity.unwrap_or(CaseSensitivity::Smart)))
                }) {
                    return Some(Ok(note));
                }

                if or_alias_contains.iter().any(|substr| {
                    note.aliases
                        .iter()
                        .any(|a| string_contains(a, substr, case_sensitivity.unwrap_or(CaseSensitivity::Smart)))
                }) {
                    return Some(Ok(note));
                }

                if or_content_contains.iter().any(|s| {
                    string_contains(
                        note.content.as_deref().unwrap(),
                        s,
                        case_sensitivity.unwrap_or(CaseSensitivity::Smart),
                    )
                }) {
                    return Some(Ok(note));
                }

                if or_regexes
                    .iter()
                    .any(|re| re.is_match(note.content.as_deref().unwrap()))
                {
                    return Some(Ok(note));
                }

                None
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
/// Respects `.gitignore`, `.git/info/exclude`, and `.ignore` files.
pub fn find_note_paths(root: impl AsRef<Path>) -> impl Iterator<Item = PathBuf> {
    WalkBuilder::new(root)
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
                && entry.path().extension().and_then(|e| e.to_str()) == Some("md")
        })
        .map(|entry| entry.into_path())
}

/// Loads all notes found recursively under `root` in parallel, without retaining body content.
pub fn find_notes(root: impl AsRef<Path>) -> Vec<Result<Note, NoteError>> {
    find_note_paths(root)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path)
        .collect()
}

/// Find all tags
pub fn find_all_tags(root: impl AsRef<Path>) -> Result<Vec<String>, NoteError> {
    let tags = find_note_paths(root)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path)
        .filter_map(|res| match res {
            Ok(note) => {
                let tags: BTreeSet<String> = note.tags.into_iter().map(|lt| lt.tag.to_lowercase()).collect();
                Some(Ok(tags))
            }
            Err(e) => Some(Err(e)),
        })
        .flatten()
        .flatten()
        .collect::<BTreeSet<String>>();

    let tags: Vec<String> = tags.into_iter().collect();
    Ok(tags)
}

/// Find occurrences of specific tags. Returns a list of located tags grouped by the note in which
/// they were found.
pub fn find_tags(root: impl AsRef<Path>, tags: &[String]) -> Result<Vec<crate::NoteTags>, SearchError> {
    let tags = tags.iter().map(|t| crate::tag::clean_tag(t)).collect::<Vec<String>>();
    let mut search = SearchQuery::new(root).include_inline_tags();
    for tag in &tags {
        search = search.or_has_tag(tag);
    }
    let notes: Vec<Note> = search.execute()?.into_iter().filter_map(|r| r.ok()).collect();

    // A note tag matches a search term if it equals the term exactly or is a sub-tag of it
    // (e.g. "workout/upper-body" matches search term "workout").
    let tag_matches_search = |tag: &str| {
        tags.iter()
            .any(|s| tag.eq_ignore_ascii_case(s) || tag.to_lowercase().starts_with(&format!("{}/", s.to_lowercase())))
    };

    let results: Vec<crate::NoteTags> = notes
        .into_iter()
        .filter_map(|note| {
            let matched: Vec<crate::LocatedTag> =
                note.tags.into_iter().filter(|lt| tag_matches_search(&lt.tag)).collect();

            if matched.is_empty() {
                None
            } else {
                Some(crate::NoteTags {
                    path: note.path.clone(),
                    tags: matched,
                })
            }
        })
        .collect();

    Ok(results)
}

/// Like [`find_notes`], but retains body content in each [`Note::content`].
pub fn find_notes_with_content(root: impl AsRef<Path>) -> Vec<Result<Note, NoteError>> {
    find_note_paths(root)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path_with_content)
        .collect()
}

/// Like [`find_notes`], but only loads notes whose path satisfies `filter`.
/// Filtering happens before any file I/O, so non-matching files are never read.
pub fn find_notes_filtered(root: impl AsRef<Path>, filter: impl Fn(&Path) -> bool) -> Vec<Result<Note, NoteError>> {
    find_note_paths(root)
        .filter(|path| filter(path))
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path)
        .collect()
}

/// Like [`find_notes_filtered`], but retains body content in each [`Note::content`].
pub fn find_notes_filtered_with_content(
    root: impl AsRef<Path>,
    filter: impl Fn(&Path) -> bool,
) -> Vec<Result<Note, NoteError>> {
    find_note_paths(root)
        .filter(|path| filter(path))
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path_with_content)
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

        let results = SearchQuery::new(dir.path()).and_glob("subdir/**").execute().unwrap();
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
                .and_glob("a/**")
                .and_glob("b/**")
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
                .and_glob("nonexistent/**")
                .execute()
                .unwrap(),
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn and_has_tag_single() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("tagged.md"), "---\ntags: [rust]\n---\nContent.");
        write_note(&dir.path().join("untagged.md"), "No tags here.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path()).and_has_tag("rust").execute().unwrap(),
        ));
        assert_eq!(ids, vec!["tagged"]);
    }

    #[test]
    fn and_tag_semantics() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("both.md"),
            "---\ntags: [rust, obsidian]\n---\nContent.",
        );
        write_note(&dir.path().join("one.md"), "---\ntags: [rust]\n---\nContent.");
        write_note(&dir.path().join("none.md"), "No tags.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_has_tag("rust")
                .and_has_tag("obsidian")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["both"]);

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .or_has_tag("rust")
                .or_has_tag("obsidian")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["both", "one"]);
    }

    #[test]
    fn and_has_tag_no_match() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("note.md"), "---\ntags: [rust]\n---\nContent.");

        let notes = unwrap_notes(SearchQuery::new(dir.path()).and_has_tag("python").execute().unwrap());
        assert!(notes.is_empty());
    }

    #[test]
    fn id_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("note-a.md"), "---\nid: my-special-id\n---\nContent.");
        write_note(&dir.path().join("note-b.md"), "Other note.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_has_id("my-special-id")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["my-special-id"]);
    }

    #[test]
    fn title_contains_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("match.md"), "# Rust Programming\n\nContent.");
        write_note(&dir.path().join("no-match.md"), "# Python Notes\n\nContent.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_title_contains("rust")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["match"]);
    }

    #[test]
    fn title_contains_no_title_excluded() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("no-title.md"), "Just plain content, no heading.");
        write_note(&dir.path().join("has-title.md"), "# My Title\n\nContent.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path()).and_title_contains("my").execute().unwrap(),
        ));
        assert_eq!(ids, vec!["has-title"]);
    }

    #[test]
    fn or_has_alias_or_semantics() {
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
                .or_has_alias("alpha-alias")
                .or_has_alias("beta-alias")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn title_contains_or_semantics() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("rust.md"), "# Rust Language\n\nContent.");
        write_note(&dir.path().join("notes.md"), "# Programming Notes\n\nContent.");
        write_note(&dir.path().join("other.md"), "# Something Else\n\nContent.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .or_title_contains("rust")
                .or_title_contains("notes")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["notes", "rust"]);
    }

    #[test]
    fn alias_contains_or_semantics() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("rust.md"),
            "---\naliases: [Rust Language]\n---\nContent.",
        );
        write_note(
            &dir.path().join("notes.md"),
            "---\naliases: [Programming Notes]\n---\nContent.",
        );
        write_note(&dir.path().join("other.md"), "No aliases.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .or_alias_contains("rust")
                .or_alias_contains("notes")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["notes", "rust"]);
    }

    #[test]
    fn and_has_alias_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("note.md"),
            "---\naliases: [Rust Programming]\n---\nContent.",
        );

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_has_alias("rust programming")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["note"]);
    }

    #[test]
    fn alias_contains_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("match.md"),
            "---\naliases: [Rust Programming]\n---\nContent.",
        );
        write_note(
            &dir.path().join("no-match.md"),
            "---\naliases: [Python Notes]\n---\nContent.",
        );

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_alias_contains("rust")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["match"]);
    }

    #[test]
    fn alias_contains_matches_any_alias() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            &dir.path().join("note.md"),
            "---\naliases: [alpha, beta-suffix]\n---\nContent.",
        );

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_alias_contains("suffix")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["note"]);
    }

    #[test]
    fn alias_contains_no_match_excluded() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("note.md"), "---\naliases: [alpha]\n---\nContent.");

        let notes = unwrap_notes(
            SearchQuery::new(dir.path())
                .and_alias_contains("beta")
                .execute()
                .unwrap(),
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn alias_contains_no_aliases_excluded() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("note.md"), "No aliases here.");

        let notes = unwrap_notes(
            SearchQuery::new(dir.path())
                .and_alias_contains("anything")
                .execute()
                .unwrap(),
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn content_contains_single() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("match.md"), "This note mentions ferris.");
        write_note(&dir.path().join("no-match.md"), "This note mentions nothing special.");

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_content_contains("ferris")
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
                .and_content_contains("alpha")
                .and_content_contains("beta")
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
                .and_content_matches(r"\d+")
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["match"]);
    }

    #[test]
    fn content_matches_invalid_regex_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = SearchQuery::new(dir.path()).and_content_matches(r"[invalid").execute();
        assert!(matches!(result, Err(SearchError::InvalidRegex(_))));
    }

    #[test]
    fn invalid_glob_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = SearchQuery::new(dir.path()).and_glob("[invalid").execute();
        assert!(matches!(result, Err(SearchError::InvalidGlob(_))));
    }

    #[test]
    fn combined_glob_and_tag_content() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("notes");

        write_note(
            &subdir.join("target.md"),
            "---\ntags: [rust]\n---\nThis note mentions ferris.",
        );
        write_note(
            &dir.path().join("wrong-glob.md"),
            "---\ntags: [rust]\n---\nThis note mentions ferris.",
        );

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_glob("notes/**")
                .and_has_tag("rust")
                .and_content_contains("ferris")
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
    fn gitignore_excludes_ignored_notes() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("included.md"), "Normal note.");
        write_note(&dir.path().join("excluded").join("secret.md"), "Ignored note.");
        fs::write(dir.path().join(".ignore"), "excluded/\n").unwrap();

        let paths: Vec<PathBuf> = find_note_paths(dir.path()).collect();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("included.md"));
    }

    #[test]
    fn vault_search_convenience() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("tagged.md"), "---\ntags: [my-tag]\n---\nContent.");
        write_note(&dir.path().join("untagged.md"), "No tags.");

        let vault = crate::Vault::open(dir.path()).unwrap();
        let ids = sorted_ids(unwrap_notes(vault.search().and_has_tag("my-tag").execute().unwrap()));
        assert_eq!(ids, vec!["tagged"]);
    }
}
