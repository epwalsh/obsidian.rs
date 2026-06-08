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

/// Scans an already-loaded note set for health issues.
///
/// This is the same duplicate ID, duplicate alias, and broken-link logic used by
/// [`Vault::check`](crate::Vault::check), but it lets callers reuse cached note
/// snapshots instead of walking the filesystem for every check.
pub fn check_notes(vault_path: impl AsRef<Path>, notes: &[Note]) -> VaultHealthReport {
    let vault_path = vault_path.as_ref();
    let note_paths: HashSet<PathBuf> = notes.iter().map(|note| note.path.clone()).collect();

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
                .map(|path| {
                    let note = notes.iter().find(|n| n.path == path).unwrap();
                    NoteRef {
                        path: path.clone(),
                        backlink_count: backlinks_from(notes, note, vault_path).len(),
                    }
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
                .map(|path| {
                    let note = notes.iter().find(|n| n.path == path).unwrap();
                    NoteRef {
                        path: path.clone(),
                        backlink_count: backlinks_from(notes, note, vault_path).len(),
                    }
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

    VaultHealthReport {
        note_count: notes.len(),
        duplicate_ids,
        duplicate_aliases,
        broken_links,
    }
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
