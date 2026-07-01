use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::{Link, Note, common, search};

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
    /// Notes with no incoming or outgoing note links, sorted by path.
    pub stranded_notes: Vec<StrandedNote>,
}

impl VaultHealthReport {
    /// Returns `true` if any health issues were found.
    pub fn has_issues(&self) -> bool {
        !self.duplicate_ids.is_empty()
            || !self.duplicate_aliases.is_empty()
            || !self.broken_links.is_empty()
            || !self.stranded_notes.is_empty()
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

/// A note with no incoming or outgoing note links.
pub struct StrandedNote {
    pub path: PathBuf,
}

#[derive(Default)]
struct NoteConnectivity {
    backlink_count: usize,
    has_outgoing_note_link: bool,
}

/// Scans an already-loaded note set for health issues.
///
/// This is the same duplicate ID, duplicate alias, broken-link, and stranded-note logic used by
/// [`Vault::check`](crate::Vault::check), but it lets callers reuse cached note
/// snapshots instead of walking the filesystem for every check.
pub fn check_notes(vault_path: impl AsRef<Path>, notes: &[Note]) -> VaultHealthReport {
    let vault_path = vault_path.as_ref();
    let note_paths: HashSet<PathBuf> = notes.iter().map(|note| note.path.clone()).collect();
    let connectivity = build_connectivity_by_path(notes, vault_path);

    // --- Duplicate IDs ---
    let mut id_map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for note in notes.iter() {
        id_map.entry(note.id.clone()).or_default().push(note.path.clone());
    }
    let mut duplicate_ids: Vec<DuplicateId> = id_map
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(id, mut paths)| {
            paths.sort();
            let note_refs = paths
                .into_iter()
                .map(|path| NoteRef {
                    backlink_count: connectivity.get(path.as_path()).map_or(0, |info| info.backlink_count),
                    path,
                })
                .collect();
            DuplicateId { id, notes: note_refs }
        })
        .collect();
    duplicate_ids.sort_by(|a, b| a.id.cmp(&b.id));

    // --- Duplicate aliases ---
    let mut alias_map: HashMap<String, HashSet<PathBuf>> = HashMap::new();
    for note in notes.iter() {
        for alias in &note.aliases {
            alias_map
                .entry(alias.to_lowercase())
                .or_default()
                .insert(note.path.clone());
        }
    }
    let mut duplicate_aliases: Vec<DuplicateAlias> = alias_map
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(alias, paths)| {
            let mut sorted_paths: Vec<PathBuf> = paths.into_iter().collect();
            sorted_paths.sort();
            let note_refs = sorted_paths
                .into_iter()
                .map(|path| NoteRef {
                    backlink_count: connectivity.get(path.as_path()).map_or(0, |info| info.backlink_count),
                    path,
                })
                .collect();
            DuplicateAlias {
                alias,
                notes: note_refs,
            }
        })
        .collect();
    duplicate_aliases.sort_by(|a, b| a.alias.cmp(&b.alias));

    // --- Broken links ---
    let mut valid_wiki_targets: HashSet<String> = HashSet::new();
    for note in notes.iter() {
        valid_wiki_targets.insert(note.id.clone());
        if let Some(stem) = note.path.file_stem().and_then(|s| s.to_str()) {
            valid_wiki_targets.insert(stem.to_string());
        }
        for alias in &note.aliases {
            valid_wiki_targets.insert(alias.clone());
            valid_wiki_targets.insert(alias.to_lowercase());
        }
    }

    let mut broken_links: Vec<BrokenLink> = Vec::new();
    for note in notes.iter() {
        for ll in &note.links {
            match &ll.link {
                Link::Wiki { target, .. } => {
                    if !target.is_empty() && !valid_wiki_targets.contains(target.as_str()) {
                        broken_links.push(BrokenLink {
                            source_path: note.path.clone(),
                            line: ll.location.line,
                            text: format!("[[{}]]", target),
                        });
                    }
                }
                Link::Markdown { url, .. } => {
                    // Skip external and absolute links; only check local .md links.
                    if url.contains("://") || url.starts_with('/') {
                        continue;
                    }
                    let url_path_raw = match url.find('#') {
                        Some(i) => &url[..i],
                        None => url.as_str(),
                    };
                    let url_path_decoded = common::percent_decode(url_path_raw);
                    let url_path = url_path_decoded.as_str();
                    if !url_path.ends_with(".md") {
                        continue;
                    }
                    let source_dirs = [vault_path, note.path.parent().unwrap_or(vault_path)];
                    if !source_dirs.iter().any(|dir| {
                        let candidate = common::normalize_path(url_path, Some(dir));
                        note_paths.contains(&candidate)
                    }) {
                        broken_links.push(BrokenLink {
                            source_path: note.path.clone(),
                            line: ll.location.line,
                            text: format!("[...]({})", url),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    broken_links.sort_by(|a, b| a.source_path.cmp(&b.source_path).then(a.line.cmp(&b.line)));

    // --- Stranded notes ---
    let mut stranded_notes: Vec<StrandedNote> = notes
        .iter()
        .filter(|note| !is_stranded_note_exempt(&note.path))
        .filter(|note| {
            let Some(connectivity) = connectivity.get(note.path.as_path()) else {
                return false;
            };
            connectivity.backlink_count == 0 && !connectivity.has_outgoing_note_link
        })
        .map(|note| StrandedNote {
            path: note.path.clone(),
        })
        .collect();
    stranded_notes.sort_by(|a, b| a.path.cmp(&b.path));

    VaultHealthReport {
        note_count: notes.len(),
        duplicate_ids,
        duplicate_aliases,
        broken_links,
        stranded_notes,
    }
}

fn build_connectivity_by_path(notes: &[Note], vault_path: &Path) -> HashMap<PathBuf, NoteConnectivity> {
    let mut connectivity_by_path: HashMap<PathBuf, NoteConnectivity> = notes
        .iter()
        .map(|note| {
            (
                note.path.clone(),
                NoteConnectivity {
                    backlink_count: 0,
                    has_outgoing_note_link: has_outgoing_note_link(note),
                },
            )
        })
        .collect();
    let note_paths = connectivity_by_path.keys().cloned().collect::<HashSet<_>>();
    let wiki_target_index = build_wiki_target_index(notes);
    let markdown_target_index = build_markdown_target_index(notes, vault_path);

    for source in notes {
        let mut linked_targets = HashSet::new();
        for located_link in &source.links {
            match &located_link.link {
                Link::Wiki { target, .. } => {
                    if let Some(target_paths) = wiki_target_index.get(target.as_str()) {
                        for target_path in target_paths {
                            insert_linked_target(&mut linked_targets, &source.path, target_path);
                        }
                    }
                }
                Link::Markdown { url, .. } => {
                    let Some(url_path) = local_markdown_url_path(url) else {
                        continue;
                    };
                    let source_dir = source.path.parent().unwrap_or(source.path.as_path());
                    let source_relative_candidate =
                        common::normalize_path(source_dir.join(&url_path), Some(vault_path));
                    if note_paths.contains(&source_relative_candidate) {
                        insert_linked_target(&mut linked_targets, &source.path, &source_relative_candidate);
                    }
                    if let Some(target_path) = markdown_target_index.get(url_path.as_str()) {
                        insert_linked_target(&mut linked_targets, &source.path, target_path);
                    }
                }
                Link::Embed { .. } => {}
            }
        }

        for target_path in linked_targets {
            connectivity_by_path
                .get_mut(target_path.as_path())
                .expect("all linked note paths should have connectivity entries")
                .backlink_count += 1;
        }
    }

    connectivity_by_path
}

fn build_wiki_target_index(notes: &[Note]) -> HashMap<String, Vec<PathBuf>> {
    let mut target_index: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for note in notes {
        target_index.entry(note.id.clone()).or_default().push(note.path.clone());
        if let Some(stem) = note.path.file_stem().and_then(|stem| stem.to_str()) {
            target_index
                .entry(stem.to_string())
                .or_default()
                .push(note.path.clone());
        }
        for alias in &note.aliases {
            target_index.entry(alias.clone()).or_default().push(note.path.clone());
        }
    }
    target_index
}

fn build_markdown_target_index(notes: &[Note], vault_path: &Path) -> HashMap<String, PathBuf> {
    notes
        .iter()
        .map(|note| {
            (
                common::relative_path(vault_path, &note.path)
                    .to_string_lossy()
                    .to_string(),
                note.path.clone(),
            )
        })
        .collect()
}

fn insert_linked_target(linked_targets: &mut HashSet<PathBuf>, source_path: &Path, target_path: &Path) {
    if target_path != source_path {
        linked_targets.insert(target_path.to_path_buf());
    }
}

fn has_outgoing_note_link(note: &Note) -> bool {
    note.links.iter().any(|link| match &link.link {
        Link::Wiki { target, .. } => !target.is_empty(),
        Link::Markdown { url, .. } => is_local_markdown_note_url(url),
        Link::Embed { .. } => false,
    })
}

fn is_local_markdown_note_url(url: &str) -> bool {
    local_markdown_url_path(url).is_some()
}

fn local_markdown_url_path(url: &str) -> Option<String> {
    if url.contains("://") || url.starts_with('/') {
        return None;
    }
    let url_path_raw = match url.find('#') {
        Some(i) => &url[..i],
        None => url,
    };
    let url_path = common::percent_decode(url_path_raw);
    if !url_path.is_empty() && url_path.ends_with(".md") {
        Some(url_path)
    } else {
        None
    }
}

fn is_stranded_note_exempt(path: &Path) -> bool {
    path.file_stem().and_then(|stem| stem.to_str()).is_some_and(|stem| {
        stem.eq_ignore_ascii_case("README")
            || stem.eq_ignore_ascii_case("CLAUDE")
            || stem.eq_ignore_ascii_case("AGENTS")
            || stem.eq_ignore_ascii_case("CHANGELOG")
    })
}

/// Returns all notes in `notes` that link to `target`, paired with the specific
/// links within each note that point to it.
pub fn backlinks_from<'a>(
    notes: &'a [Note],
    target: &Note,
    vault_path: &Path,
) -> Vec<(&'a Note, Vec<crate::LocatedLink>)> {
    notes
        .iter()
        .filter_map(|source| {
            let matching = search::find_matching_links(source, target, vault_path);
            if matching.is_empty() {
                None
            } else {
                Some((source, matching))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_notes_reports_stranded_notes_but_ignores_readmes() {
        let vault_dir = tempfile::tempdir().unwrap();
        let notes = vec![
            Note::parse(vault_dir.path().join("linked-source.md"), "See [[linked-target]].\n"),
            Note::parse(vault_dir.path().join("linked-target.md"), "Body.\n"),
            Note::parse(vault_dir.path().join("isolated.md"), "No note links here.\n"),
            Note::parse(vault_dir.path().join("README.md"), "Project overview.\n"),
        ];

        let report = check_notes(vault_dir.path(), &notes);
        let stranded_paths: Vec<_> = report
            .stranded_notes
            .iter()
            .map(|note| note.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        assert_eq!(stranded_paths, vec!["isolated.md"]);
        assert!(report.has_issues());
    }

    #[test]
    fn check_notes_treats_broken_and_self_links_as_outgoing_links() {
        let vault_dir = tempfile::tempdir().unwrap();
        let notes = vec![
            Note::parse(vault_dir.path().join("broken.md"), "See [[missing-note]].\n"),
            Note::parse(vault_dir.path().join("self-link.md"), "See [[self-link]].\n"),
        ];

        let report = check_notes(vault_dir.path(), &notes);
        assert_eq!(report.broken_links.len(), 1);
        assert!(report.stranded_notes.is_empty());
    }

    #[test]
    fn check_notes_counts_each_source_target_backlink_once() {
        let vault_dir = tempfile::tempdir().unwrap();
        let notes = vec![
            Note::parse(vault_dir.path().join("wiki-source.md"), "See [[dup]] and [[dup]].\n"),
            Note::parse(
                vault_dir.path().join("markdown-source.md"),
                "See [A](dupe-a.md) and [B](dupe-b.md).\n",
            ),
            Note::parse(vault_dir.path().join("dupe-a.md"), "---\nid: dup\n---\n"),
            Note::parse(vault_dir.path().join("dupe-b.md"), "---\nid: dup\n---\n"),
        ];

        let report = check_notes(vault_dir.path(), &notes);
        let duplicate = report
            .duplicate_ids
            .iter()
            .find(|duplicate| duplicate.id == "dup")
            .expect("duplicate ID should be reported");
        let backlink_counts: Vec<_> = duplicate
            .notes
            .iter()
            .map(|note| {
                (
                    note.path.file_name().unwrap().to_string_lossy().to_string(),
                    note.backlink_count,
                )
            })
            .collect();

        assert_eq!(
            backlink_counts,
            vec![("dupe-a.md".to_string(), 2), ("dupe-b.md".to_string(), 2)]
        );
    }
}
