use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use rayon::prelude::*;
use regex::Regex;

use crate::{Link, LocatedLink, Location, Note, NoteError, SearchError, common};

#[derive(Debug, Clone, Copy)]
enum CaseSensitivity {
    Sensitive,
    Ignore,
    Smart,
}

#[derive(Debug, Clone, Copy)]
pub enum SortOrder {
    PathAsc,
    PathDesc,
    ModifiedAsc,
    ModifiedDesc,
    CreatedAsc,
    CreatedDesc,
}

pub fn sort_notes<T>(items: &mut [Note], sort: &SortOrder) {
    sort_notes_by(items, |n| Some(n), sort);
}

pub fn sort_notes_by<T>(items: &mut [T], key: impl Fn(&T) -> Option<&Note>, sort: &SortOrder) {
    let fallback_path = PathBuf::new();
    match sort {
        SortOrder::PathAsc => items.sort_by(|a, b| {
            let a_path = key(a).as_ref().map(|n| &n.path).unwrap_or(&fallback_path);
            let b_path = key(b).as_ref().map(|n| &n.path).unwrap_or(&fallback_path);
            a_path.cmp(b_path)
        }),
        SortOrder::PathDesc => items.sort_by(|a, b| {
            let a_path = key(a).as_ref().map(|n| &n.path).unwrap_or(&fallback_path);
            let b_path = key(b).as_ref().map(|n| &n.path).unwrap_or(&fallback_path);
            b_path.cmp(a_path)
        }),
        SortOrder::ModifiedAsc => items.sort_by_key(|r| {
            key(r)
                .as_ref()
                .map(|n| n.last_modified_time())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        }),
        SortOrder::ModifiedDesc => items.sort_by_key(|r| {
            std::cmp::Reverse(
                key(r)
                    .as_ref()
                    .map(|n| n.last_modified_time())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
        }),
        SortOrder::CreatedAsc => items.sort_by_key(|r| {
            key(r)
                .as_ref()
                .map(|n| n.creation_time())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        }),
        SortOrder::CreatedDesc => items.sort_by_key(|r| {
            std::cmp::Reverse(
                key(r)
                    .as_ref()
                    .map(|n| n.creation_time())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
        }),
    }
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
pub struct SearchQuery<'a> {
    config: SearchQueryConfig,
    loaded_notes: Option<&'a HashMap<PathBuf, Note>>,
}

/// All owned, non-reference fields of [`SearchQuery`]. Extracted into its own struct so that
/// [`SearchQuery::with_loaded_notes`] can change the lifetime parameter without reconstructing
/// every field individually.
struct SearchQueryConfig {
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
    and_links_to: Vec<Note>,
    or_links_to: Vec<Note>,
    case_sensitivity: Option<CaseSensitivity>,
    include_inline_tags: bool,
    sort_order: Option<SortOrder>,
}

impl SearchQuery<'static> {
    pub fn new(root: impl AsRef<Path>) -> Self {
        SearchQuery {
            config: SearchQueryConfig {
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
                and_links_to: Vec::new(),
                or_links_to: Vec::new(),
                case_sensitivity: None,
                include_inline_tags: false,
                sort_order: None,
            },
            loaded_notes: None,
        }
    }

    /// Provide in-memory notes to use instead of their on-disk counterparts.
    ///
    /// Each note's `note.path` is matched against the vault's note paths on disk. Notes whose
    /// paths exist on disk shadow the disk version; notes with no on-disk counterpart are included
    /// as additional candidates. In-memory notes are assumed to have `content` populated whenever
    /// content filters (e.g. [`and_content_contains`](Self::and_content_contains)) are used.
    pub fn with_loaded_notes<'a>(self, notes: &'a HashMap<PathBuf, Note>) -> SearchQuery<'a> {
        SearchQuery {
            config: self.config,
            loaded_notes: Some(notes),
        }
    }
}

impl<'a> SearchQuery<'a> {
    /// Note path must match this glob pattern (matched against the note's path relative to the vault root).
    pub fn and_glob(mut self, pattern: impl Into<String>) -> Self {
        self.config.and_globs.push(pattern.into());
        self
    }

    /// Note path could match this glob pattern (matched against the note's path relative to the vault root).
    pub fn or_glob(mut self, pattern: impl Into<String>) -> Self {
        self.config.or_globs.push(pattern.into());
        self
    }

    /// Note must have this ID (case-sensitive by default).
    pub fn and_has_id(mut self, id: impl Into<String>) -> Self {
        self.config.and_id = Some(id.into());
        self
    }

    /// Note could have this ID (case-sensitive by default).
    pub fn or_has_id(mut self, id: impl Into<String>) -> Self {
        self.config.or_ids.push(id.into());
        self
    }

    /// Note must have this tag (case-insensitive by default).
    pub fn and_has_tag(mut self, tag: impl Into<String>) -> Self {
        self.config.and_tags.push(crate::tag::clean_tag(&tag.into()));
        self
    }

    /// Note could have this tag (case-insensitive by default).
    pub fn or_has_tag(mut self, tag: impl Into<String>) -> Self {
        self.config.or_tags.push(crate::tag::clean_tag(&tag.into()));
        self
    }

    /// Must title must contain this substring (smart case-sensitive by default).
    pub fn and_title_contains(mut self, s: impl Into<String>) -> Self {
        self.config.and_title_contains.push(s.into());
        self
    }

    /// Must title could contain this substring (smart case-sensitive by default).
    pub fn or_title_contains(mut self, s: impl Into<String>) -> Self {
        self.config.or_title_contains.push(s.into());
        self
    }

    /// Note must have this alias (smart case-sensitive by default).
    pub fn and_has_alias(mut self, alias: impl Into<String>) -> Self {
        self.config.and_aliases.push(alias.into());
        self
    }

    /// Note could have this alias (smart case-sensitive by default).
    pub fn or_has_alias(mut self, alias: impl Into<String>) -> Self {
        self.config.or_aliases.push(alias.into());
        self
    }

    /// Substring must match against any of the note's aliases (smart case-sensitive by default).
    pub fn and_alias_contains(mut self, s: impl Into<String>) -> Self {
        self.config.and_alias_contains.push(s.into());
        self
    }

    /// Substring could match against any of the note's aliases (smart case-sensitive by default).
    pub fn or_alias_contains(mut self, s: impl Into<String>) -> Self {
        self.config.or_alias_contains.push(s.into());
        self
    }

    /// Note body must contain this string (smart case-sensitive by default).
    pub fn and_content_contains(mut self, s: impl Into<String>) -> Self {
        self.config.and_content_contains.push(s.into());
        self
    }

    /// Note body could contain this string (smart case-sensitive by default).
    pub fn or_content_contains(mut self, s: impl Into<String>) -> Self {
        self.config.or_content_contains.push(s.into());
        self
    }

    /// Regex body must match this pattern (smart case-sensitive by default).
    pub fn and_content_matches(mut self, pattern: impl Into<String>) -> Self {
        self.config.and_content_matches.push(pattern.into());
        self
    }

    /// Regex body could match this pattern (smart case-sensitive by default).
    pub fn or_content_matches(mut self, pattern: impl Into<String>) -> Self {
        self.config.or_content_matches.push(pattern.into());
        self
    }

    /// Has a link to this note
    pub fn and_links_to(mut self, note: Note) -> Self {
        self.config.and_links_to.push(note);
        self
    }

    /// May link to this note
    pub fn or_links_to(mut self, note: Note) -> Self {
        self.config.or_links_to.push(note);
        self
    }

    /// Execute the search case-sensitively.
    pub fn case_sensitive(mut self) -> Self {
        self.config.case_sensitivity = Some(CaseSensitivity::Sensitive);
        self
    }

    /// Execute the search case-insensitively.
    pub fn ignore_case(mut self) -> Self {
        self.config.case_sensitivity = Some(CaseSensitivity::Ignore);
        self
    }

    /// Execute the search with smart case sensitivity: case-sensitive if the query contains any uppercase letters,
    /// otherwise case-insensitive.
    pub fn smart_case(mut self) -> Self {
        self.config.case_sensitivity = Some(CaseSensitivity::Smart);
        self
    }

    pub fn include_inline_tags(mut self) -> Self {
        self.config.include_inline_tags = true;
        self
    }

    pub fn sort_by(mut self, sort_order: SortOrder) -> Self {
        self.config.sort_order = Some(sort_order);
        self
    }

    /// Execute the query, returning matching notes.
    ///
    /// Returns `Err` if any glob or regex pattern is invalid.
    /// Each inner `Err` represents an I/O failure loading a specific note.
    pub fn execute(self) -> Result<Vec<Result<Note, NoteError>>, SearchError> {
        let SearchQuery { config, loaded_notes } = self;
        let SearchQueryConfig {
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
            and_links_to,
            or_links_to,
            case_sensitivity,
            include_inline_tags,
            sort_order,
        } = config;

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

        // Build a set of paths covered by in-memory overrides so they can be excluded from the disk walk.
        let override_paths: HashSet<&Path> = loaded_notes
            .map(|m| m.keys().map(|p| p.as_path()).collect())
            .unwrap_or_default();

        let paths: Vec<PathBuf> = find_note_paths(&root)
            .filter(|path| !override_paths.contains(path.as_path()))
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
            || !or_regexes.is_empty()
            || !or_links_to.is_empty();
        let has_filters = has_or_filters
            || and_id.is_some()
            || !and_tags.is_empty()
            || !and_title_contains.is_empty()
            || !and_aliases.is_empty()
            || !and_alias_contains.is_empty()
            || !and_content_contains.is_empty()
            || !and_regexes.is_empty()
            || !and_links_to.is_empty();

        // Shared filter closure: apply all AND/OR filters to an already-loaded note.
        // `rel` is the note's path relative to the vault root (used for or_glob matching).
        let filter_note = |note: Note, rel: &Path| -> Option<Result<Note, NoteError>> {
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
                    note.title
                        .as_deref()
                        .is_some_and(|t| string_contains(t, substr, case_sensitivity.unwrap_or(CaseSensitivity::Smart)))
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
                        note.body.as_deref().unwrap(),
                        s,
                        case_sensitivity.unwrap_or(CaseSensitivity::Smart),
                    )
                })
            {
                return None;
            }

            if !and_regexes.is_empty() && !and_regexes.iter().all(|re| re.is_match(note.body.as_deref().unwrap())) {
                return None;
            }

            if !and_links_to.is_empty()
                && !and_links_to
                    .iter()
                    .all(|n| !find_matching_links(&note, n, &root).is_empty())
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
                    note.body.as_deref().unwrap(),
                    s,
                    case_sensitivity.unwrap_or(CaseSensitivity::Smart),
                )
            }) {
                return Some(Ok(note));
            }

            if or_regexes.iter().any(|re| re.is_match(note.body.as_deref().unwrap())) {
                return Some(Ok(note));
            }

            if or_links_to
                .iter()
                .any(|n| !find_matching_links(&note, n, &root).is_empty())
            {
                return Some(Ok(note));
            }

            None
        };

        // Process disk notes in parallel.
        let mut results: Vec<Result<Note, NoteError>> = paths
            .into_par_iter()
            .filter_map(|path| -> Option<Result<Note, NoteError>> {
                let rel = path.strip_prefix(&root).unwrap_or(&path);
                let load = if needs_content {
                    Note::from_path_with_body(&path)
                } else {
                    Note::from_path(&path)
                };
                let note = match load {
                    Ok(n) => n,
                    Err(e) => return Some(Err(e)),
                };
                filter_note(note, rel)
            })
            .collect();

        // Process in-memory override notes sequentially (typically a small set).
        if let Some(notes) = loaded_notes {
            for note in notes.values() {
                // Apply and_glob pre-filter.
                if !and_globs.is_empty() {
                    let rel = note.path.strip_prefix(&root).unwrap_or(&note.path);
                    if !and_glob_set.is_match(rel) {
                        continue;
                    }
                }
                // Guard against missing content when content filters are active.
                if needs_content && note.body.is_none() {
                    results.push(Err(NoteError::BodyNotLoaded));
                    continue;
                }
                // Compute rel as owned PathBuf so we can move the cloned note into filter_note.
                let rel_buf = note.path.strip_prefix(&root).unwrap_or(&note.path).to_path_buf();
                if let Some(result) = filter_note(note.clone(), &rel_buf) {
                    results.push(result);
                }
            }
        }

        if let Some(sort_order) = sort_order {
            sort_notes_by(&mut results, |r| r.as_ref().ok(), &sort_order);
        };

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

/// Find all tags used across the vault. When `loaded_notes` is provided, notes whose paths appear
/// in the map are excluded from the disk walk and the in-memory versions are used instead.
pub fn find_all_tags(
    root: impl AsRef<Path>,
    loaded_notes: Option<&HashMap<PathBuf, Note>>,
) -> Result<Vec<String>, NoteError> {
    let root = root.as_ref();
    let override_paths: HashSet<&Path> = loaded_notes
        .map(|m| m.keys().map(|p| p.as_path()).collect())
        .unwrap_or_default();

    let mut tags: BTreeSet<String> = find_note_paths(root)
        .filter(|p| !override_paths.contains(p.as_path()))
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

    // Include tags from in-memory notes.
    if let Some(notes) = loaded_notes {
        for note in notes.values() {
            for lt in &note.tags {
                tags.insert(lt.tag.to_lowercase());
            }
        }
    }

    Ok(tags.into_iter().collect())
}

/// Find occurrences of specific tags. Returns a list of located tags grouped by the note in which
/// they were found. When `loaded_notes` is provided, notes whose paths appear in the map are
/// excluded from the disk walk and the in-memory versions are used instead.
pub fn find_tags(
    root: impl AsRef<Path>,
    tags: &[String],
    loaded_notes: Option<&HashMap<PathBuf, Note>>,
) -> Result<Vec<(Note, Vec<crate::LocatedTag>)>, SearchError> {
    let tags = tags.iter().map(|t| crate::tag::clean_tag(t)).collect::<Vec<String>>();
    let root_ref = root.as_ref();
    let notes: Vec<Note> = if let Some(loaded) = loaded_notes {
        let mut q = SearchQuery::new(root_ref)
            .include_inline_tags()
            .with_loaded_notes(loaded);
        for tag in &tags {
            q = q.or_has_tag(tag);
        }
        q.execute()?.into_iter().filter_map(|r| r.ok()).collect()
    } else {
        let mut q = SearchQuery::new(root_ref).include_inline_tags();
        for tag in &tags {
            q = q.or_has_tag(tag);
        }
        q.execute()?.into_iter().filter_map(|r| r.ok()).collect()
    };

    // A note tag matches a search term if it equals the term exactly or is a sub-tag of it
    // (e.g. "workout/upper-body" matches search term "workout").
    let tag_matches_search = |tag: &str| {
        tags.iter()
            .any(|s| tag.eq_ignore_ascii_case(s) || tag.to_lowercase().starts_with(&format!("{}/", s.to_lowercase())))
    };

    let results: Vec<(Note, Vec<crate::LocatedTag>)> = notes
        .into_iter()
        .filter_map(|note| {
            let matched: Vec<crate::LocatedTag> = note
                .tags
                .iter()
                .filter_map(|lt| {
                    if tag_matches_search(&lt.tag) {
                        Some(lt.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if matched.is_empty() {
                None
            } else {
                Some((note, matched))
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
        .map(Note::from_path_with_body)
        .collect()
}

/// Like [`find_notes`], but only loads notes whose path satisfies `filter`.
/// Filtering happens before any file I/O, so non-matching files are never read.
/// When `loaded_notes` is provided, notes whose paths appear in the map are excluded from the
/// disk walk and the in-memory versions are used instead (if they also pass `filter`).
pub fn find_notes_filtered(
    root: impl AsRef<Path>,
    filter: impl Fn(&Path) -> bool,
    loaded_notes: Option<&HashMap<PathBuf, Note>>,
) -> Vec<Result<Note, NoteError>> {
    let root = root.as_ref();
    let override_paths: HashSet<&Path> = loaded_notes
        .map(|m| m.keys().map(|p| p.as_path()).collect())
        .unwrap_or_default();

    let mut results: Vec<Result<Note, NoteError>> = find_note_paths(root)
        .filter(|path| !override_paths.contains(path.as_path()))
        .filter(|path| filter(path))
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path)
        .collect();

    if let Some(notes) = loaded_notes {
        for note in notes.values() {
            if filter(&note.path) {
                results.push(Ok(note.clone()));
            }
        }
    }

    results
}

/// Like [`find_notes_filtered`], but retains body content in each [`Note::content`].
pub fn find_notes_filtered_with_content(
    root: impl AsRef<Path>,
    filter: impl Fn(&Path) -> bool,
    loaded_notes: Option<&HashMap<PathBuf, Note>>,
) -> Vec<Result<Note, NoteError>> {
    let root = root.as_ref();
    let override_paths: HashSet<&Path> = loaded_notes
        .map(|m| m.keys().map(|p| p.as_path()).collect())
        .unwrap_or_default();

    let mut results: Vec<Result<Note, NoteError>> = find_note_paths(root)
        .filter(|path| !override_paths.contains(path.as_path()))
        .filter(|path| filter(path))
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path_with_body)
        .collect();

    if let Some(notes) = loaded_notes {
        for note in notes.values() {
            if filter(&note.path) {
                results.push(Ok(note.clone()));
            }
        }
    }

    results
}

/// Returns all links in `source` that point to `target`, using the vault root `vault_path`
/// for resolving relative markdown URLs. Returns an empty vec if `source` is `target`.
pub fn find_matching_links(source: &Note, target: &Note, vault_path: &std::path::Path) -> Vec<LocatedLink> {
    if source.path == target.path {
        return Vec::new();
    }
    let target_stem = target.path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string());
    source
        .links
        .clone()
        .into_iter()
        .filter(|ll| match &ll.link {
            Link::Wiki {
                target: wiki_target, ..
            } => {
                wiki_target == &target.id
                    || target_stem.as_deref().is_some_and(|s| wiki_target == s)
                    || target.aliases.iter().any(|a| wiki_target == a)
            }
            Link::Markdown { url, .. } => {
                if url.contains("://") || url.starts_with('/') {
                    return false;
                }
                let url_path = match url.find('#') {
                    Some(i) => &url[..i],
                    None => url.as_str(),
                };
                if !url_path.ends_with(".md") {
                    return false;
                }
                let source_dir = source.path.parent().unwrap_or(&source.path);
                (common::normalize_path(source_dir.join(url_path), Some(vault_path)) == target.path)
                    || (url_path == common::relative_path(vault_path, &target.path).to_string_lossy())
            }
            _ => false,
        })
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

    // --- with_loaded_notes tests ---

    #[test]
    fn with_loaded_notes_replaces_disk_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        write_note(&path, "disk content");

        // Override with in-memory version that has different content.
        let mut in_memory = Note::from_path_with_body(&path).unwrap();
        in_memory.body = Some("in-memory content".to_string());
        let overrides: HashMap<PathBuf, Note> = [(path.clone(), in_memory)].into_iter().collect();

        let notes = unwrap_notes(
            SearchQuery::new(dir.path())
                .and_content_contains("in-memory")
                .with_loaded_notes(&overrides)
                .execute()
                .unwrap(),
        );
        assert_eq!(notes.len(), 1);

        // The disk version (with "disk content") should not appear.
        let disk_match = unwrap_notes(
            SearchQuery::new(dir.path())
                .and_content_contains("disk")
                .execute()
                .unwrap(),
        );
        assert_eq!(disk_match.len(), 1); // baseline: disk has "disk content"

        let overrides2: HashMap<PathBuf, Note> = {
            let mut m2 = Note::from_path_with_body(&path).unwrap();
            m2.body = Some("in-memory content".to_string());
            [(path, m2)].into_iter().collect()
        };
        let no_disk_match = unwrap_notes(
            SearchQuery::new(dir.path())
                .and_content_contains("disk")
                .with_loaded_notes(&overrides2)
                .execute()
                .unwrap(),
        );
        assert!(no_disk_match.is_empty());
    }

    #[test]
    fn with_loaded_notes_no_double_counting() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("a.md"), "Note A.");
        write_note(&dir.path().join("b.md"), "Note B.");
        write_note(&dir.path().join("c.md"), "Note C.");

        let path_a = dir.path().join("a.md");
        let override_a = Note::from_path(&path_a).unwrap();
        let overrides: HashMap<PathBuf, Note> = [(path_a, override_a)].into_iter().collect();

        let notes = unwrap_notes(
            SearchQuery::new(dir.path())
                .with_loaded_notes(&overrides)
                .execute()
                .unwrap(),
        );
        // Should still be exactly 3, not 4.
        assert_eq!(notes.len(), 3);
    }

    #[test]
    fn with_loaded_notes_new_note_not_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("existing.md"), "Existing note.");

        // Create an in-memory note whose path does not exist on disk.
        let new_path = dir.path().join("new-unsaved.md");
        let new_note = Note {
            path: new_path.clone(),
            id: "new-unsaved".to_string(),
            title: None,
            aliases: Vec::new(),
            tags: Vec::new(),
            body: Some("Brand new content.".to_string()),
            links: Vec::new(),
            frontmatter: None,
            frontmatter_line_count: 0,
        };
        let overrides: HashMap<PathBuf, Note> = [(new_path, new_note)].into_iter().collect();

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .with_loaded_notes(&overrides)
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["existing", "new-unsaved"]);
    }

    #[test]
    fn with_loaded_notes_respects_tag_filter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        write_note(&path, "---\ntags: [old-tag]\n---\nContent.");

        // Override with a note that has a different tag.
        let mut override_note = Note::from_path(&path).unwrap();
        override_note.tags = vec![crate::LocatedTag {
            tag: "new-tag".to_string(),
            location: crate::Location::Frontmatter,
        }];
        let overrides: HashMap<PathBuf, Note> = [(path, override_note)].into_iter().collect();

        // Should find the override note via the new tag.
        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_has_tag("new-tag")
                .with_loaded_notes(&overrides)
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["note"]);

        // Should NOT find via the old tag (disk version is excluded).
        let ids_old = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_has_tag("old-tag")
                .with_loaded_notes(&overrides)
                .execute()
                .unwrap(),
        ));
        assert!(ids_old.is_empty());
    }

    #[test]
    fn with_loaded_notes_glob_filter_applied_to_override() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("notes");
        write_note(&subdir.join("included.md"), "In notes/.");

        // In-memory note at root level — should be excluded by the and_glob("notes/**") filter.
        let root_path = dir.path().join("outside.md");
        let outside_note = Note {
            path: root_path.clone(),
            id: "outside".to_string(),
            title: None,
            aliases: Vec::new(),
            tags: Vec::new(),
            body: Some("Outside notes dir.".to_string()),
            links: Vec::new(),
            frontmatter: None,
            frontmatter_line_count: 0,
        };
        let overrides: HashMap<PathBuf, Note> = [(root_path, outside_note)].into_iter().collect();

        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_glob("notes/**")
                .with_loaded_notes(&overrides)
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["included"]);
    }

    #[test]
    fn with_loaded_notes_content_not_loaded_returns_error() {
        let dir = tempfile::tempdir().unwrap();

        // In-memory note with no content loaded.
        let path = dir.path().join("no-content.md");
        let note = Note {
            path: path.clone(),
            id: "no-content".to_string(),
            title: None,
            aliases: Vec::new(),
            tags: Vec::new(),
            body: None, // content not loaded
            links: Vec::new(),
            frontmatter: None,
            frontmatter_line_count: 0,
        };
        let overrides: HashMap<PathBuf, Note> = [(path, note)].into_iter().collect();

        let results = SearchQuery::new(dir.path())
            .and_content_contains("anything")
            .with_loaded_notes(&overrides)
            .execute()
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(NoteError::BodyNotLoaded)));
    }

    #[test]
    fn with_loaded_notes_multiple_overrides() {
        let dir = tempfile::tempdir().unwrap();
        write_note(&dir.path().join("a.md"), "---\ntags: [old]\n---\nContent A.");
        write_note(&dir.path().join("b.md"), "---\ntags: [old]\n---\nContent B.");
        write_note(&dir.path().join("c.md"), "---\ntags: [old]\n---\nContent C.");

        let path_a = dir.path().join("a.md");
        let path_b = dir.path().join("b.md");

        let mut override_a = Note::from_path(&path_a).unwrap();
        override_a.tags = vec![crate::LocatedTag {
            tag: "new".to_string(),
            location: crate::Location::Frontmatter,
        }];
        let mut override_b = Note::from_path(&path_b).unwrap();
        override_b.tags = vec![crate::LocatedTag {
            tag: "new".to_string(),
            location: crate::Location::Frontmatter,
        }];

        let overrides: HashMap<PathBuf, Note> = [(path_a, override_a), (path_b, override_b)].into_iter().collect();

        // "new" tag should match both overrides but not "c" (which has "old" on disk).
        let ids = sorted_ids(unwrap_notes(
            SearchQuery::new(dir.path())
                .and_has_tag("new")
                .with_loaded_notes(&overrides)
                .execute()
                .unwrap(),
        ));
        assert_eq!(ids, vec!["a", "b"]);
    }
}
