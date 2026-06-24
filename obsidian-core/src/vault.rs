use std::collections::{BTreeSet, HashMap, HashSet};
use std::env::current_dir;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gray_matter::Pod;
use indexmap::IndexMap;
use rayon::prelude::*;

use crate::health::VaultHealthReport;
use crate::{InlineLocation, Link, LocatedLink, LocatedTag, Location, Note, NoteError, VaultError, common, search};

#[derive(Clone)]
struct CachedDiskNote {
    note: Note,
    text: String,
}

#[derive(Clone)]
pub struct Vault {
    path: PathBuf,
    note_overrides: HashMap<PathBuf, Note>,
    cached_disk_notes: Option<HashMap<PathBuf, Arc<CachedDiskNote>>>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("note_override_count", &self.note_overrides.len())
            .field(
                "cached_disk_note_count",
                &self.cached_disk_notes.as_ref().map(HashMap::len),
            )
            .finish()
    }
}

impl Vault {
    /// Opens a vault at the given path, returning an error if the path does not exist or is not a
    /// directory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VaultError> {
        let path = common::normalize_path(path, None);
        if !path.is_dir() {
            return Err(VaultError::NotADirectory(path));
        }
        Ok(Vault {
            path,
            note_overrides: HashMap::new(),
            cached_disk_notes: None,
        })
    }

    /// Opens a vault and immediately caches all notes in memory.
    ///
    /// This is intended for long-lived processes, such as LSP or MCP servers, that repeatedly query
    /// the same vault and receive explicit file-change notifications to refresh stale entries.
    pub fn open_cached(path: impl AsRef<Path>) -> Result<Self, VaultError> {
        let mut vault = Self::open(path)?;
        vault.cache_notes();
        Ok(vault)
    }

    /// Opens the nearest vault by walking up from the current directory, looking for an
    /// `.obsidian/` directory. Falls back to the current directory if none is found.
    pub fn open_from_cwd() -> Result<Self, VaultError> {
        let cwd = std::env::current_dir()?;
        let mut current = cwd.as_path();
        loop {
            if current.join(".obsidian").is_dir() {
                return Self::open(current);
            }
            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }
        Self::open(&cwd)
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn is_note_path(path: impl AsRef<Path>) -> bool {
        path.as_ref().extension().and_then(|ext| ext.to_str()) == Some("md")
    }

    pub fn normalize_path(&self, path: impl AsRef<Path>) -> PathBuf {
        common::normalize_path(path, Some(&self.path))
    }

    pub fn cache_notes(&mut self) {
        let notes = search::find_note_paths(&self.path)
            .collect::<Vec<_>>()
            .into_par_iter()
            .filter_map(|path| load_cached_disk_note(path).ok())
            .map(|cached| (cached.note.path.clone(), Arc::new(cached)))
            .collect();
        self.cached_disk_notes = Some(notes);
    }

    pub fn has_cached_note(&self, path: impl AsRef<Path>) -> bool {
        let path = self.normalize_path(path);
        self.cached_disk_notes
            .as_ref()
            .is_some_and(|notes| notes.contains_key(&path))
    }

    fn has_known_note_path(&self, path: &Path) -> bool {
        let path = common::normalize_path(path, None);
        path.is_file()
            || self.note_overrides.contains_key(&path)
            || self
                .cached_disk_notes
                .as_ref()
                .is_some_and(|notes| notes.contains_key(&path))
    }

    pub fn note_for_path(&self, path: impl AsRef<Path>) -> Result<Note, VaultError> {
        let path = self.normalize_path(path);
        if let Some(note) = self.note_overrides.get(&path) {
            return Ok(note.clone());
        }
        if let Some(note) = self
            .cached_disk_notes
            .as_ref()
            .and_then(|notes| notes.get(&path))
            .map(|cached| cached.note.clone())
        {
            return Ok(note);
        }
        Ok(Note::from_path(path)?)
    }

    pub fn text_for_path(&self, path: impl AsRef<Path>) -> Result<String, VaultError> {
        let path = self.normalize_path(path);
        if let Some(note) = self.note_overrides.get(&path) {
            return Ok(note.read(true)?);
        }
        if let Some(text) = self
            .cached_disk_notes
            .as_ref()
            .and_then(|notes| notes.get(&path))
            .map(|cached| cached.text.clone())
        {
            return Ok(text);
        }
        Ok(fs::read_to_string(path)?)
    }

    pub fn refresh_cached_note(&mut self, path: impl AsRef<Path>) -> Result<bool, VaultError> {
        let path = self.normalize_path(path);
        let Some(cached_disk_notes) = self.cached_disk_notes.as_mut() else {
            return Ok(false);
        };
        if !Self::is_note_path(&path) || !path.is_file() {
            cached_disk_notes.remove(&path);
            return Ok(false);
        }

        let cached = load_cached_disk_note(&path)?;
        cached_disk_notes.insert(cached.note.path.clone(), Arc::new(cached));
        Ok(true)
    }

    pub fn remove_cached_note(&mut self, path: impl AsRef<Path>) -> bool {
        let path = self.normalize_path(path);
        self.cached_disk_notes
            .as_mut()
            .is_some_and(|cached_disk_notes| cached_disk_notes.remove(&path).is_some())
    }

    /// Resolve a note based on a path, filename, ID, title, or alias.
    pub fn resolve_note(&self, note: &str) -> Result<Note, VaultError> {
        // First try as a path.
        if let Ok((path, _)) = self.resolve_note_path(note, true) {
            return self.note_for_path(path);
        }

        // Then search by ID, aliases, and potentially filename.
        let mut search = self.search().or_has_id(note).or_has_alias(note).ignore_case();
        if note.ends_with(".md") && !note.contains('/') {
            let glob = format!("**/{}", note);
            let stem = note.trim_end_matches(".md");
            search = search.or_glob(glob).or_has_id(stem).or_has_alias(stem);
        }

        let results = search.execute().map_err(VaultError::Search)?;
        let mut notes: Vec<Note> = results.into_iter().filter_map(|r| r.ok()).collect();

        if notes.is_empty() {
            return Err(VaultError::NoteNotFound(note.to_string()));
        }

        if notes.len() == 1 {
            return Ok(notes.remove(0));
        }

        // If we have more than one match, check for *exact* (case-sensitive) matches on ID and aliases before giving up.
        let paths = notes.iter().map(|n| n.path.clone()).collect();
        let mut notes: Vec<_> = notes
            .into_iter()
            .filter(|n| n.id == note || n.aliases.iter().any(|a| a == note))
            .collect();

        if notes.len() == 1 {
            return Ok(notes.remove(0));
        }

        Err(VaultError::AmbiguousNoteIdentifier(note.to_string(), paths))
    }

    /// Resolve a note path argument, which may be absolute or relative to either the current working
    /// directory or the vault root.
    /// Returns the resolved absolute path and the root it was resolved against, if any.
    pub fn resolve_note_path(
        &self,
        path: impl AsRef<Path>,
        strict: bool,
    ) -> Result<(std::path::PathBuf, Option<std::path::PathBuf>), VaultError> {
        let path = path.as_ref().to_path_buf();
        if path.is_absolute() {
            if self.has_known_note_path(&path) || !strict {
                return Ok((common::normalize_path(&path, None), None));
            } else {
                return Err(VaultError::NoteNotFound(path.to_string_lossy().to_string()));
            }
        }

        // If the cwd is inside of the vault root, prefer resolving against the cwd to avoid surprising
        // behavior where a note exists in the vault but can't be found because the user is working in
        // a subdirectory.
        let cwd = current_dir()?;
        let mut cwd_resolved = common::normalize_path(&path, Some(&cwd));
        if cwd_resolved.starts_with(&self.path) {
            // Return right away if it exists, otherwise check if the extension is missing.
            if self.has_known_note_path(&cwd_resolved) {
                return Ok((cwd_resolved, Some(cwd)));
            } else if cwd_resolved.extension().is_none() {
                cwd_resolved.set_extension("md");
                if self.has_known_note_path(&cwd_resolved) {
                    return Ok((cwd_resolved, Some(cwd)));
                }
            }

            // In strict mode, if we still haven't found an existing path, try the same thing against the vault root.
            // Otherwise return the cwd-resolved path even if it doesn't exist, since
            // that's more likely what the user intended than the vault root.
            let mut vault_resolved = common::normalize_path(&path, Some(&self.path));
            if strict {
                if self.has_known_note_path(&vault_resolved) {
                    return Ok((vault_resolved, Some(self.path.clone())));
                } else if vault_resolved.extension().is_none() {
                    vault_resolved.set_extension("md");
                    if self.has_known_note_path(&vault_resolved) {
                        return Ok((vault_resolved, Some(self.path.clone())));
                    }
                }
            } else {
                return Ok((cwd_resolved, Some(cwd)));
            }
        } else {
            let mut vault_resolved = common::normalize_path(&path, Some(&self.path));
            if self.has_known_note_path(&vault_resolved) {
                return Ok((vault_resolved, Some(self.path.clone())));
            } else if vault_resolved.extension().is_none() {
                vault_resolved.set_extension("md");
                if self.has_known_note_path(&vault_resolved) {
                    return Ok((vault_resolved, Some(self.path.clone())));
                }
            }

            if !strict {
                return Ok((vault_resolved, Some(self.path.clone())));
            }
        }

        Err(VaultError::NoteNotFound(path.to_string_lossy().to_string()))
    }

    /// Loads all notes in the vault in parallel, without retaining body content.
    ///
    /// Links and inline tags are still extracted and available on each note.
    /// Use [`notes_with_content`](Self::notes_with_content) when body text is needed.
    pub fn notes(&self) -> Vec<Result<Note, NoteError>> {
        self.notes_filtered(|_| true)
    }

    /// Like [`notes`](Self::notes), but retains body content in each [`Note::content`].
    pub fn notes_with_content(&self) -> Vec<Result<Note, NoteError>> {
        self.notes_filtered_with_content(|_| true)
    }

    /// Inserts or replaces an in-memory note. While present, this note shadows its on-disk
    /// counterpart (matched by `note.path`) across all vault search operations. Notes with a path
    /// that does not exist on disk are included as additional candidates.
    pub fn load_note(&mut self, mut note: Note) {
        let resolved_path = self
            .resolve_note_path(&note.path, false)
            .map(|(n, _)| n)
            .unwrap_or_else(|_| note.path.clone());
        note.path = resolved_path;
        self.note_overrides.insert(note.path.clone(), note);
    }

    /// Removes a previously loaded in-memory note, restoring the on-disk version for searches.
    /// Does nothing if the path is not currently loaded.
    pub fn unload_note(&mut self, path: &Path) {
        let resolved_path = self
            .resolve_note_path(path, false)
            .map(|(n, _)| n)
            .unwrap_or_else(|_| path.into());
        self.note_overrides.remove(&resolved_path);
    }

    pub fn note_is_loaded(&self, path: impl AsRef<Path>) -> bool {
        let path = self.normalize_path(path);
        self.note_overrides.contains_key(&path)
    }

    /// Like [`notes`](Self::notes), but skips notes whose path does not satisfy `filter`.
    /// Filtering happens at the filesystem traversal level, before any file is read.
    pub fn notes_filtered(&self, filter: impl Fn(&Path) -> bool) -> Vec<Result<Note, NoteError>> {
        if let Some(cached_disk_notes) = &self.cached_disk_notes {
            return self.cached_disk_notes_filtered(cached_disk_notes, filter, false);
        }
        search::find_notes_filtered(&self.path, filter, Some(&self.note_overrides))
    }

    /// Like [`notes_filtered`](Self::notes_filtered), but retains body content in each [`Note::content`].
    pub fn notes_filtered_with_content(&self, filter: impl Fn(&Path) -> bool) -> Vec<Result<Note, NoteError>> {
        if let Some(cached_disk_notes) = &self.cached_disk_notes {
            return self.cached_disk_notes_filtered(cached_disk_notes, filter, true);
        }
        search::find_notes_filtered_with_content(&self.path, filter, Some(&self.note_overrides))
    }

    fn cached_disk_notes_filtered(
        &self,
        cached_disk_notes: &HashMap<PathBuf, Arc<CachedDiskNote>>,
        filter: impl Fn(&Path) -> bool,
        with_body: bool,
    ) -> Vec<Result<Note, NoteError>> {
        let override_paths = self.note_overrides.keys().cloned().collect::<HashSet<_>>();
        let mut results = Vec::new();

        for cached in cached_disk_notes.values() {
            if override_paths.contains(&cached.note.path) || !filter(&cached.note.path) {
                continue;
            }
            let note = if with_body {
                Note::parse(&cached.note.path, &cached.text)
            } else {
                cached.note.clone()
            };
            results.push(Ok(note));
        }

        for note in self.note_overrides.values() {
            if !filter(&note.path) {
                continue;
            }
            if with_body && note.body.is_none() {
                results.push(Err(NoteError::BodyNotLoaded));
            } else {
                results.push(Ok(note.clone()));
            }
        }

        results.sort_by(|left, right| {
            let left_path = left.as_ref().ok().map(|note| &note.path);
            let right_path = right.as_ref().ok().map(|note| &note.path);
            left_path.cmp(&right_path)
        });
        results
    }

    /// Scans the vault for health issues: duplicate IDs, duplicate aliases, broken links, and stranded notes.
    ///
    /// Only notes whose path satisfies `filter` are included. Pass `|_| true` to scan
    /// everything, or use a glob-based closure (see [`notes_filtered`](Self::notes_filtered))
    /// to exclude specific paths.
    ///
    /// Note-load failures are silently skipped (consistent with other vault scan methods).
    pub fn check(&self, filter: impl Fn(&Path) -> bool) -> VaultHealthReport {
        let notes: Vec<Note> = self.notes_filtered(filter).into_iter().filter_map(|r| r.ok()).collect();
        crate::health::check_notes(&self.path, &notes)
    }

    /// Returns a [`SearchQuery`](search::SearchQuery) rooted at this vault's path.
    /// Any notes previously registered via [`load_note`](Self::load_note) are automatically
    /// included, shadowing their on-disk counterparts.
    pub fn search(&self) -> search::SearchQuery<'_> {
        let query = search::SearchQuery::new(&self.path).with_loaded_notes(&self.note_overrides);
        if let Some(cached_disk_notes) = &self.cached_disk_notes {
            query.with_cached_notes(
                cached_disk_notes
                    .values()
                    .map(|cached| search::CachedSearchNote::new(&cached.note, &cached.text))
                    .collect(),
            )
        } else {
            query
        }
    }

    /// Returns all unique tags used in the vault, aggregated from frontmatter and inline tags.
    pub fn list_tags(&self) -> Result<Vec<String>, VaultError> {
        if self.cached_disk_notes.is_some() {
            let mut tags = BTreeSet::new();
            for note in self.notes_filtered(|_| true).into_iter().filter_map(Result::ok) {
                tags.extend(note.tags.into_iter().map(|tag| tag.tag.to_lowercase()));
            }
            return Ok(tags.into_iter().collect());
        }
        search::find_all_tags(&self.path, Some(&self.note_overrides)).map_err(VaultError::Note)
    }

    /// Find all occurrences of specific tags, grouped by the note they appear in. Tags are matched
    /// case-insensitively, and sub-tags are gathered as well.
    pub fn find_tags(&self, tags: &[String]) -> Result<Vec<(Note, Vec<LocatedTag>)>, VaultError> {
        search::find_tags_with_query(self.search(), tags).map_err(VaultError::Search)
    }

    /// Find and replaces all occurrences of `old_tag` with the new `new_tag` and return
    /// the occurrences and location of the new tag.
    pub fn rename_tag(&mut self, old_tag: &str, new_tag: &str) -> Result<Vec<(Note, Vec<LocatedTag>)>, VaultError> {
        let mut results: Vec<(Note, Vec<LocatedTag>)> = Vec::new();
        for (mut note, tags) in self.find_tags(&[old_tag.into()])? {
            let mut tags_by_line: HashMap<usize, Vec<InlineLocation>> = HashMap::new();
            for lt in tags {
                match lt.location {
                    // Replace frontmatter tags immediately.
                    Location::Frontmatter => {
                        note.remove_tag(&lt.tag);
                        note.add_tag(new_tag);
                    }
                    // And gather tags in the body by their line number (1-indexed).
                    Location::Inline(loc) => {
                        tags_by_line.entry(loc.line).or_default();
                        tags_by_line.get_mut(&loc.line).unwrap().push(loc);
                    }
                };
            }

            if !tags_by_line.is_empty() {
                // Replace all occurrences of the old tag in the body with the new one.
                note.load_body()?;
                let mut lines: Vec<String> = note.body.as_ref().unwrap().lines().map(|s| s.to_string()).collect();
                for (lnum, locs) in tags_by_line.drain() {
                    let line = lines.get_mut(lnum - 1 - note.frontmatter_line_count).unwrap();
                    let mut offset = 0;
                    for loc in locs {
                        line.replace_range(
                            (offset + loc.col_start)..(offset + loc.col_end),
                            &format!("#{}", new_tag),
                        );
                        offset += new_tag.len() - old_tag.len();
                    }
                }

                let body = lines.join("\n");
                note.update_content(Some(&body), None)?;
            }

            // Re-collect the located tags.
            let tags = note
                .tags
                .iter()
                .filter_map(|lt| if lt.tag == new_tag { Some(lt.clone()) } else { None })
                .collect();

            // If note is loaded, need to update it. Otherwise write to disk.
            if self.note_is_loaded(&note.path) {
                self.load_note(note.clone());
            } else {
                note.write()?;
            }

            results.push((note, tags));
        }

        Ok(results)
    }

    /// Returns all notes in the vault that link to `target`, paired with the specific
    /// [`LocatedLink`]s within each note that point to it.
    ///
    /// Only wiki links (`[[target]]`) and markdown links (`[text](target.md)`) are
    /// considered. Embed links are excluded. Notes that fail to load are silently skipped.
    pub fn backlinks(&self, target: &Note) -> Result<Vec<(Note, Vec<LocatedLink>)>, VaultError> {
        if self.cached_disk_notes.is_some() {
            let notes: Vec<Note> = self.notes().into_iter().filter_map(Result::ok).collect();
            return Ok(self
                .backlinks_from(&notes, target)
                .into_iter()
                .map(|(note, links)| (note.clone(), links))
                .collect());
        }

        let results = self
            .search()
            .and_links_to(target.clone())
            .execute()
            .map_err(VaultError::Search)?;
        let notes: Vec<Note> = results.into_iter().filter_map(|r| r.ok()).collect();
        let results = notes
            .into_iter()
            .map(|source| {
                let matching = search::find_matching_links(&source, target, &self.path);
                (source, matching)
            })
            .collect();
        Ok(results)
    }

    /// Like [`backlinks`](Self::backlinks), but operates on an already-loaded slice of notes
    /// instead of reading from disk. Returns references into `notes`.
    pub fn backlinks_from<'a>(&self, notes: &'a [Note], target: &Note) -> Vec<(&'a Note, Vec<LocatedLink>)> {
        crate::health::backlinks_from(notes, target, &self.path)
    }

    /// Computes all replacement pairs for a rename without performing any I/O.
    fn compute_rename_op(&self, note: &Note, new_path: &Path) -> Result<RenameOp, VaultError> {
        let new_dir = new_path.parent().unwrap_or_else(|| Path::new("."));
        if !new_dir.is_dir() {
            return Err(VaultError::DirectoryNotFound(new_dir.to_path_buf()));
        }

        if new_path.exists() {
            return Err(VaultError::NoteAlreadyExists(new_path.to_path_buf()));
        }

        let new_stem = new_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        let old_stem = note
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        let id_needs_update = note.id == old_stem;
        // let frontmatter_id_will_update =
        //     id_needs_update && note.frontmatter.as_ref().is_some_and(|fm| fm.contains_key("id"));

        let backlinks = self.backlinks(note)?;
        let mut per_note_replacements: Vec<(Note, Vec<(LocatedLink, String)>)> = Vec::new();

        for (source_note, links) in backlinks {
            let mut replacements: Vec<(LocatedLink, String)> = Vec::new();

            for ll in links {
                let new_text = match &ll.link {
                    Link::Wiki { target, heading, alias } if id_needs_update && target == &old_stem => {
                        let mut wiki = format!("[[{}", new_stem);
                        if let Some(h) = heading {
                            wiki.push('#');
                            wiki.push_str(h);
                        }
                        if let Some(a) = alias {
                            wiki.push('|');
                            wiki.push_str(a);
                        }
                        wiki.push_str("]]");
                        Some(wiki)
                    }
                    Link::Wiki { .. } => None,
                    Link::Markdown { text, url } => {
                        let fragment = url.find('#').map(|i| url[i..].to_string());
                        let new_url = common::relative_path(&self.path, new_path);
                        let new_url_str = new_url.to_string_lossy().replace('\\', "/");
                        let full_url = match fragment {
                            Some(f) => format!("{}{}", new_url_str, f),
                            None => new_url_str,
                        };
                        Some(format!("[{}]({})", text, full_url))
                    }
                    _ => None,
                };
                if let Some(text) = new_text {
                    replacements.push((ll, text));
                }
            }

            if !replacements.is_empty() {
                per_note_replacements.push((source_note, replacements));
            }
        }

        Ok(RenameOp {
            new_stem,
            frontmatter_id_will_update: id_needs_update,
            per_note_replacements,
        })
    }

    /// Renames `note` to `new_path` (full destination path), updating all backlinks.
    ///
    /// Wiki links targeting the old ID are rewritten to the new stem. Markdown links pointing
    /// to the old path are rewritten to the new path. Wiki links targeting an alias are left
    /// unchanged. Returns the reloaded [`Note`] at the new path.
    ///
    /// Returns [`VaultError::DirectoryNotFound`] if the parent directory of `new_path` does not
    /// exist, and [`VaultError::NoteAlreadyExists`] if `new_path` is already occupied.
    pub fn rename(&mut self, note: &Note, new_path: &Path) -> Result<Note, VaultError> {
        let new_path = common::normalize_path(new_path, Some(&self.path));
        let op = self.compute_rename_op(note, &new_path)?;

        let mut renamed = note.clone();
        renamed.load_body()?;
        renamed.path = new_path;
        if op.frontmatter_id_will_update {
            renamed.id = op.new_stem;
        }

        if self.note_is_loaded(&note.path) {
            self.load_note(renamed.clone());
            _ = std::fs::remove_file(&note.path);
        } else {
            renamed.write()?;
            std::fs::remove_file(&note.path)?;
        }

        for (mut source_note, replacements) in op.per_note_replacements {
            if self.note_is_loaded(&source_note.path) {
                let new_content = common::rewrite_links(
                    &source_note
                        .body
                        .clone()
                        .ok_or(VaultError::Note(NoteError::BodyNotLoaded))?,
                    replacements,
                );
                source_note.update_content(Some(&new_content), None)?;
                self.load_note(source_note);
            } else {
                let raw_content = std::fs::read_to_string(&source_note.path)?;
                let new_content = common::rewrite_links(&raw_content, replacements);
                std::fs::write(&source_note.path, new_content)?;
            }
        }

        Ok(renamed)
    }

    /// Returns a preview of what [`rename`](Self::rename) would change without touching the filesystem.
    ///
    /// Same validation and error variants as `rename`.
    pub fn rename_preview(&self, note: &Note, new_path: &Path) -> Result<RenamePreview, VaultError> {
        let edits = self.rename_edits(note, new_path)?;
        let updated_notes = edits
            .backlink_edits
            .iter()
            .map(|(path, replacements)| (path.clone(), replacements.len()))
            .collect();

        Ok(RenamePreview {
            new_path: edits.new_path,
            id_will_update: edits.id_will_update,
            updated_notes,
        })
    }

    /// Returns the exact backlink edits that [`rename`](Self::rename) would make without touching the filesystem.
    ///
    /// Same validation and error variants as `rename`.
    pub fn rename_edits(&self, note: &Note, new_path: &Path) -> Result<RenameEdits, VaultError> {
        let new_path = common::normalize_path(new_path, Some(&self.path));
        let op = self.compute_rename_op(note, &new_path)?;

        let mut backlink_edits: Vec<(PathBuf, Vec<(LocatedLink, String)>)> = op
            .per_note_replacements
            .into_iter()
            .map(|(source_note, replacements)| (source_note.path, replacements))
            .collect();
        backlink_edits.sort_by(|(a, _), (b, _)| a.cmp(b));

        Ok(RenameEdits {
            new_path: new_path.to_path_buf(),
            new_stem: op.new_stem,
            id_will_update: op.frontmatter_id_will_update,
            backlink_edits,
        })
    }

    fn raw_note_sections<'a>(note_path: &Path, raw: &'a str) -> (&'a str, &'a str) {
        let parsed = Note::parse(note_path, raw);
        let body_start = raw
            .split_inclusive('\n')
            .take(parsed.frontmatter_line_count)
            .map(str::len)
            .sum();
        raw.split_at(body_start)
    }

    fn apply_raw_note_update(&mut self, note_path: &Path, raw: String) -> Result<Note, VaultError> {
        let note_path = self.normalize_path(note_path);
        let updated = Note::parse(&note_path, &raw);

        if self.note_is_loaded(&note_path) {
            self.load_note(updated.clone());
        } else {
            let parent = note_path.parent().unwrap_or_else(|| Path::new("."));
            let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
            tmp.write_all(raw.as_bytes())?;
            tmp.persist(&note_path).map_err(|e| e.error)?;
        }

        Ok(updated)
    }

    /// Replaces the first (and only) occurrence of `old_string` in the body text of `note`
    /// with `new_string`, preserving the rest of the raw note text unchanged.
    ///
    /// Returns [`VaultError::StringNotFound`] if `old_string` does not appear in the body, and
    /// [`VaultError::StringFoundMultipleTimes`] if it appears more than once. Both checks operate
    /// on the raw body text only (frontmatter excluded).
    pub fn patch_note(&mut self, note: &Note, old_string: &str, new_string: &str) -> Result<Note, VaultError> {
        let raw = self.text_for_path(&note.path)?;
        let (raw_frontmatter, raw_body) = Self::raw_note_sections(&note.path, &raw);

        let count = raw_body.matches(old_string).count();
        if count == 0 {
            return Err(VaultError::StringNotFound(note.path.clone()));
        }
        if count > 1 {
            return Err(VaultError::StringFoundMultipleTimes(note.path.clone()));
        }

        let patched_body = raw_body.replacen(old_string, new_string, 1);
        let patched_raw = format!("{raw_frontmatter}{patched_body}");
        self.apply_raw_note_update(&note.path, patched_raw)
    }

    /// Appends `content` exactly as provided to the end of `note`'s body text, preserving the raw
    /// frontmatter block unchanged.
    pub fn append_to_note(&mut self, note: &Note, content: &str) -> Result<Note, VaultError> {
        let raw = self.text_for_path(&note.path)?;
        let (raw_frontmatter, raw_body) = Self::raw_note_sections(&note.path, &raw);
        let appended_raw = format!("{raw_frontmatter}{raw_body}{content}");
        self.apply_raw_note_update(&note.path, appended_raw)
    }

    /// Computes all changes required to merge `sources` into `dest_path` without performing I/O.
    fn compute_merge_op(&self, sources: &[Note], dest_path: impl AsRef<Path>) -> Result<MergeOp, VaultError> {
        use std::collections::HashMap;

        let dest_path = dest_path.as_ref();
        let dest_dir = &dest_path.parent().unwrap_or_else(|| Path::new("."));
        if !dest_dir.is_dir() {
            return Err(VaultError::DirectoryNotFound(dest_dir.to_path_buf()));
        }

        for source in sources {
            if source.path == dest_path {
                return Err(VaultError::MergeSourceIsDestination(source.path.clone()));
            }
        }

        let dest_is_loaded = self.note_is_loaded(dest_path);
        let dest_is_new = !dest_is_loaded && !dest_path.exists();

        let dest_stem = dest_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        let source_paths: Vec<&Path> = sources.iter().map(|s| s.path.as_path()).collect();

        // Aggregate backlink replacements per linking note, skipping sources and dest.
        let mut replacements_by_path: HashMap<PathBuf, Vec<(LocatedLink, String)>> = HashMap::new();

        for source in sources {
            let backlinks = self.backlinks(source)?;
            for (linking_note, links) in backlinks {
                if source_paths.iter().any(|p| *p == linking_note.path) {
                    continue;
                }
                if linking_note.path == dest_path {
                    continue;
                }

                let entry = replacements_by_path.entry(linking_note.path.clone()).or_default();

                for ll in links {
                    let new_text = match &ll.link {
                        Link::Wiki { heading, alias, .. } => {
                            let mut wiki = format!("[[{}", dest_stem);
                            if let Some(h) = heading {
                                wiki.push('#');
                                wiki.push_str(h);
                            }
                            if let Some(a) = alias {
                                wiki.push('|');
                                wiki.push_str(a);
                            }
                            wiki.push_str("]]");
                            Some(wiki)
                        }
                        Link::Markdown { text, url } => {
                            let fragment = url.find('#').map(|i| url[i..].to_string());
                            let new_url = common::relative_path(&self.path, dest_path);
                            let new_url_str = new_url.to_string_lossy().replace('\\', "/");
                            let full_url = match fragment {
                                Some(f) => format!("{}{}", new_url_str, f),
                                None => new_url_str.to_string(),
                            };
                            Some(format!("[{}]({})", text, full_url))
                        }
                        _ => None,
                    };
                    if let Some(text) = new_text {
                        entry.push((ll, text));
                    }
                }
            }
        }

        let per_note_replacements: Vec<(PathBuf, Vec<(LocatedLink, String)>)> = replacements_by_path
            .into_iter()
            .filter(|(_, r)| !r.is_empty())
            .collect();

        // Load existing destination if present.
        let (dest_body, dest_fm_tags, dest_fm_aliases, dest_frontmatter) = if dest_is_new {
            (String::new(), Vec::<String>::new(), Vec::<String>::new(), None)
        } else {
            let d = Note::from_path_with_body(dest_path)?;
            let tags = d
                .frontmatter
                .as_ref()
                .and_then(|fm| fm.get("tags"))
                .and_then(|p| p.as_vec().ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.as_string().ok())
                .collect::<Vec<_>>();
            let aliases = d
                .frontmatter
                .as_ref()
                .and_then(|fm| fm.get("aliases"))
                .and_then(|p| p.as_vec().ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.as_string().ok())
                .collect::<Vec<_>>();
            let body = d.body.as_deref().unwrap_or("").trim_start().to_string();
            let fm = d.frontmatter;
            (body, tags, aliases, fm)
        };

        // Build merged body.
        let mut body_parts: Vec<String> = Vec::new();
        if !dest_body.is_empty() {
            body_parts.push(dest_body);
        }
        for source in sources {
            let body = source
                .body
                .as_deref()
                .ok_or(crate::NoteError::BodyNotLoaded)?
                .trim_start()
                .to_string();
            if !body.is_empty() {
                body_parts.push(body);
            }
        }
        let merged_content = body_parts.join("\n\n---\n\n");

        // Build merged frontmatter: dest wins on id/title, union on tags/aliases.
        let mut fm: IndexMap<String, Pod> = dest_frontmatter.unwrap_or_default();

        let mut tag_strings: Vec<String> = dest_fm_tags;
        for source in sources {
            for lt in source
                .tags
                .iter()
                .filter(|t| matches!(t.location, Location::Frontmatter))
            {
                if !tag_strings.contains(&lt.tag) {
                    tag_strings.push(lt.tag.clone());
                }
            }
        }
        if !tag_strings.is_empty() {
            fm.insert(
                "tags".to_string(),
                Pod::Array(tag_strings.clone().into_iter().map(Pod::String).collect()),
            );
        }

        let mut alias_strings: Vec<String> = dest_fm_aliases;
        for source in sources {
            let src_aliases: Vec<String> = source
                .frontmatter
                .as_ref()
                .and_then(|sfm| sfm.get("aliases"))
                .and_then(|p| p.as_vec().ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.as_string().ok())
                .collect();
            for alias in src_aliases {
                if !alias_strings.contains(&alias) {
                    alias_strings.push(alias);
                }
            }
        }
        if !alias_strings.is_empty() {
            fm.insert(
                "aliases".to_string(),
                Pod::Array(alias_strings.clone().into_iter().map(Pod::String).collect()),
            );
        }

        // Union remaining source frontmatter fields (dest wins on conflicts; id/tags/aliases are
        // excluded because they're handled above or must not be inherited from sources).
        const SKIP_KEYS: &[&str] = &["id", "tags", "aliases"];
        for source in sources {
            if let Some(sfm) = &source.frontmatter {
                for (k, v) in sfm {
                    if !SKIP_KEYS.contains(&k.as_str()) {
                        fm.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
        }

        let merged_frontmatter = if fm.is_empty() { None } else { Some(fm) };

        Ok(MergeOp {
            dest_is_new,
            dest_is_loaded,
            merged_content,
            merged_frontmatter,
            merged_tags: tag_strings
                .into_iter()
                .map(|tag| LocatedTag {
                    tag,
                    location: Location::Frontmatter,
                })
                .collect(),
            merged_aliases: alias_strings,
            per_note_replacements,
        })
    }

    /// Merges `sources` into `dest_path`: appends each source's body to the destination,
    /// union-merges tags and aliases, rewrites all backlinks to sources in other notes to
    /// point to the destination, and deletes the source files.
    ///
    /// The destination is created if it doesn't exist, or its content is appended to if it does.
    /// Returns the resulting destination [`Note`].
    pub fn merge(&mut self, sources: &[Note], dest_path: &impl AsRef<Path>) -> Result<Note, VaultError> {
        let dest_path = common::normalize_path(dest_path, Some(&self.path));
        let op = self.compute_merge_op(sources, &dest_path)?;

        // Build and write destination note.
        let dest_note = if op.dest_is_new {
            let mut dest = Note::builder(dest_path)?
                .aliases(&op.merged_aliases)
                .located_tags(&op.merged_tags)
                .build()?;
            dest.update_content(Some(&op.merged_content), op.merged_frontmatter)?;
            dest.write()?;
            dest
        } else if op.dest_is_loaded {
            let dest = self.note_overrides.get_mut(&dest_path).unwrap();
            dest.update_content(Some(&op.merged_content), op.merged_frontmatter)?;
            dest.clone()
        } else {
            let mut dest = Note::from_path_with_body(&dest_path)?;
            dest.update_content(Some(&op.merged_content), op.merged_frontmatter)?;
            dest.write()?;
            Note::from_path(&dest_path)?
        };

        // Rewrite backlinks in external notes.
        for (note_path, replacements) in op.per_note_replacements {
            let raw_content = std::fs::read_to_string(&note_path)?;
            let new_content = common::rewrite_links(&raw_content, replacements);
            std::fs::write(&note_path, new_content)?;
        }

        // Delete source files.
        for source in sources {
            std::fs::remove_file(&source.path)?;
        }

        Ok(dest_note)
    }

    /// Returns a preview of what [`merge`](Self::merge) would change without touching the filesystem.
    ///
    /// Same validation and error variants as `merge`.
    pub fn merge_preview(&self, sources: &[Note], dest_path: impl AsRef<Path>) -> Result<MergePreview, VaultError> {
        let dest_path = common::normalize_path(dest_path, Some(&self.path));
        let op = self.compute_merge_op(sources, &dest_path)?;

        let mut updated_notes: Vec<(PathBuf, usize)> = op
            .per_note_replacements
            .iter()
            .map(|(path, reps)| (path.clone(), reps.len()))
            .collect();
        updated_notes.sort_by(|(a, _), (b, _)| a.cmp(b));

        Ok(MergePreview {
            dest_path: dest_path.to_path_buf(),
            dest_is_new: op.dest_is_new,
            dest_is_loaded: op.dest_is_loaded,
            sources: sources.iter().map(|s| s.path.clone()).collect(),
            updated_notes,
        })
    }
}

fn load_cached_disk_note(path: impl AsRef<Path>) -> Result<CachedDiskNote, NoteError> {
    let path = common::normalize_path(path, None);
    let text = fs::read_to_string(&path)?;
    let mut note = Note::parse(&path, &text);
    note.body = None;
    Ok(CachedDiskNote { note, text })
}

struct RenameOp {
    new_stem: String,
    frontmatter_id_will_update: bool,
    /// Only notes with ≥1 replacement included.
    per_note_replacements: Vec<(Note, Vec<(LocatedLink, String)>)>,
}

/// Public summary of what a rename would change, without touching the filesystem.
pub struct RenamePreview {
    pub new_path: PathBuf,
    pub id_will_update: bool,
    /// Notes with backlinks that would be rewritten, sorted by path. Each entry is (path, link_count).
    pub updated_notes: Vec<(PathBuf, usize)>,
}

/// Exact text changes required for backlink rewrites during a note rename.
pub struct RenameEdits {
    pub new_path: PathBuf,
    pub new_stem: String,
    pub id_will_update: bool,
    /// Notes with backlinks that would be rewritten, sorted by path.
    pub backlink_edits: Vec<(PathBuf, Vec<(LocatedLink, String)>)>,
}

/// Public summary of what a merge would change, without touching the filesystem.
pub struct MergePreview {
    pub dest_path: PathBuf,
    pub dest_is_new: bool,
    pub dest_is_loaded: bool,
    /// Source paths that would be deleted.
    pub sources: Vec<PathBuf>,
    /// Notes with backlinks to any source that would be rewritten, sorted by path. Each entry is (path, link_count).
    pub updated_notes: Vec<(PathBuf, usize)>,
}

struct MergeOp {
    dest_is_new: bool,
    dest_is_loaded: bool,
    /// Combined body content for the destination note (no leading whitespace).
    merged_content: String,
    /// Merged frontmatter for the destination note.
    merged_frontmatter: Option<IndexMap<String, Pod>>,
    /// Merged frontmatter tags
    merged_tags: Vec<LocatedTag>,
    /// Merged aliases
    merged_aliases: Vec<String>,
    /// External notes (not sources, not dest) with backlinks to rewrite.
    per_note_replacements: Vec<(PathBuf, Vec<(LocatedLink, String)>)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // --- constructor tests ---

    #[test]
    fn open_from_cwd_finds_obsidian_dir() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("notes/daily");
        fs::create_dir_all(&subdir).unwrap();
        fs::create_dir(dir.path().join(".obsidian")).unwrap();

        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&subdir).unwrap();
        let vault = Vault::open_from_cwd().unwrap();
        std::env::set_current_dir(original_cwd).unwrap();

        assert_eq!(vault.path.canonicalize().unwrap(), dir.path().canonicalize().unwrap());
    }

    #[test]
    fn cached_vault_refresh_path_adds_created_note_and_updates_health() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();
        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "See [[target]].").unwrap();

        let mut vault = Vault::open_cached(vault_dir.path()).unwrap();
        assert_eq!(vault.notes().len(), 1);
        assert_eq!(vault.check(|_| true).broken_links.len(), 1);

        let target_path = vault_dir.path().join("target.md");
        fs::write(&target_path, "---\nid: target\n---\n").unwrap();
        assert!(vault.refresh_cached_note(&target_path).unwrap());

        assert_eq!(vault.notes().len(), 2);
        assert!(vault.check(|_| true).broken_links.is_empty());
    }

    #[test]
    fn cached_vault_remove_path_removes_note_and_updates_health() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();
        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "See [[target]].").unwrap();
        let target_path = vault_dir.path().join("target.md");
        fs::write(&target_path, "---\nid: target\n---\n").unwrap();

        let mut vault = Vault::open_cached(vault_dir.path()).unwrap();
        assert!(vault.check(|_| true).broken_links.is_empty());

        let target_path = target_path.canonicalize().unwrap();
        fs::remove_file(&target_path).unwrap();
        assert!(vault.remove_cached_note(&target_path));

        let report = vault.check(|_| true);
        assert_eq!(report.note_count, 1);
        assert_eq!(report.broken_links.len(), 1);
    }

    #[test]
    fn cached_vault_health_checks_markdown_links_against_cached_disk_notes() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();
        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "See [target](target.md).").unwrap();
        let target_path = vault_dir.path().join("target.md");
        fs::write(&target_path, "---\nid: target\n---\n").unwrap();

        let mut vault = Vault::open_cached(vault_dir.path()).unwrap();
        let target_path = target_path.canonicalize().unwrap();
        fs::remove_file(&target_path).unwrap();

        assert!(
            vault.check(|_| true).broken_links.is_empty(),
            "cached target should keep markdown link valid until cache is updated"
        );

        assert!(vault.remove_cached_note(&target_path));
        assert_eq!(vault.check(|_| true).broken_links.len(), 1);
    }

    #[test]
    fn cached_vault_search_uses_cached_disk_notes_until_refresh() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();
        let note_path = vault_dir.path().join("note.md");
        fs::write(
            &note_path,
            "---\nid: cached-id\ntags: [cached]\n---\n\ncached body #cached-inline\n",
        )
        .unwrap();

        let mut vault = Vault::open_cached(vault_dir.path()).unwrap();
        fs::write(
            &note_path,
            "---\nid: fresh-id\ntags: [fresh]\n---\n\nfresh body #fresh-inline\n",
        )
        .unwrap();

        let cached_id_results = vault.search().or_has_id("cached-id").execute().unwrap();
        assert_eq!(cached_id_results.into_iter().filter_map(Result::ok).count(), 1);
        assert!(
            vault
                .search()
                .or_has_id("fresh-id")
                .execute()
                .unwrap()
                .into_iter()
                .filter_map(Result::ok)
                .next()
                .is_none()
        );

        let cached_content_results = vault.search().and_content_contains("cached body").execute().unwrap();
        assert_eq!(cached_content_results.into_iter().filter_map(Result::ok).count(), 1);
        assert!(
            vault
                .search()
                .and_content_contains("fresh body")
                .execute()
                .unwrap()
                .into_iter()
                .filter_map(Result::ok)
                .next()
                .is_none()
        );

        assert_eq!(vault.find_tags(&["cached".to_string()]).unwrap().len(), 1);
        assert!(vault.find_tags(&["fresh".to_string()]).unwrap().is_empty());

        assert!(vault.refresh_cached_note(&note_path).unwrap());
        let fresh_id_results = vault.search().or_has_id("fresh-id").execute().unwrap();
        assert_eq!(fresh_id_results.into_iter().filter_map(Result::ok).count(), 1);
        assert_eq!(vault.find_tags(&["fresh".to_string()]).unwrap().len(), 1);
    }

    #[test]
    fn open_from_cwd_falls_back_to_cwd_when_no_obsidian_dir() {
        let dir = tempfile::tempdir().unwrap();

        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let vault = Vault::open_from_cwd().unwrap();
        std::env::set_current_dir(original_cwd).unwrap();

        assert_eq!(vault.path.canonicalize().unwrap(), dir.path().canonicalize().unwrap());
    }

    #[test]
    fn open_valid_directory() {
        let dir = tempfile::tempdir().unwrap();
        // std::fs::create_dir(&dir).unwrap();
        let vault = Vault::open(dir.path()).expect("should open valid directory");
        assert_eq!(vault.path, common::normalize_path(dir.path(), None));
    }

    #[test]
    fn open_nonexistent_path_errors() {
        let result = Vault::open("/nonexistent/path/to/vault");
        assert!(result.is_err());
    }

    #[test]
    fn open_file_path_errors() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let result = Vault::open(file.path());
        assert!(result.is_err());
    }

    // --- note resolution tests ---

    #[test]
    fn resolve_note_by_filename() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(dir.path().join("root.md"), "---\nid: root\n---\n\nRoot note.").unwrap();
        fs::write(subdir.join("nested.md"), "---\nid: nested\n---\n\nNested note.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = vault.resolve_note("root.md").expect("should resolve root.md");
        assert_eq!(note.id, "root");

        let note = vault
            .resolve_note("nested.md")
            .expect("should resolve subdir/nested.md");
        assert_eq!(note.id, "nested");
    }

    #[test]
    fn resolve_note_by_alias_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("note_a.md"),
            "---\nid: note_a\naliases: [Foo, A]\n---\n\nNote A.",
        )
        .unwrap();
        fs::write(
            dir.path().join("note_b.md"),
            "---\nid: note_b\naliases: [foo, B]\n---\n\nNote B.",
        )
        .unwrap();

        let vault = Vault::open(dir.path()).unwrap();

        let note = vault.resolve_note("Foo").expect("should resolve note");
        assert_eq!(note.id, "note_a");

        let note = vault.resolve_note("foo").expect("should resolve note");
        assert_eq!(note.id, "note_b");
    }

    // --- note loading tests ---

    #[test]
    fn notes_loads_md_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "# Note A\n\nContent A.").unwrap();
        fs::write(dir.path().join("b.md"), "# Note B\n\nContent B.").unwrap();
        fs::write(dir.path().join("not-a-note.txt"), "ignored").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let notes: Vec<Note> = vault.notes().into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(notes.len(), 2);
    }

    #[test]
    fn notes_finds_nested_md_files() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(dir.path().join("root.md"), "Root note.").unwrap();
        fs::write(subdir.join("nested.md"), "Nested note.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let notes: Vec<Note> = vault.notes().into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(notes.len(), 2);
    }

    // --- backlinks tests ---

    #[test]
    fn backlinks_wiki_by_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "---\nid: my-id\n---\nTarget.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[my-id]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
        assert!(backlinks[0].0.path.ends_with("source.md"));
        assert_eq!(backlinks[0].1.len(), 1);
    }

    #[test]
    fn backlinks_wiki_by_stem_when_id_differs() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("my-note.md"), "---\nid: custom-id\n---\nTarget.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[my-note]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("my-note.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
        assert!(backlinks[0].0.path.ends_with("source.md"));
    }

    #[test]
    fn backlinks_wiki_by_alias() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "---\naliases: [t-alias]\n---\nTarget.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[t-alias]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_wiki_by_title() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "# My Title\n\nContent.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[My Title]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_wiki_with_heading_suffix() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[target#section]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_excludes_self() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Self link: [[target]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_excludes_notes_with_no_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("other.md"), "No links here.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_returns_all_matching_links_from_one_note() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[target]] and also [[target]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].1.len(), 2);
    }

    #[test]
    fn backlinks_no_match_on_unrelated_wiki_link() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[other-note]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_markdown_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](target.md)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
        assert!(backlinks[0].0.path.ends_with("source.md"));
    }

    #[test]
    fn backlinks_markdown_fragment_stripped() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](target.md#section)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_markdown_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("sub");
        fs::create_dir(&subdir).unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(subdir.join("source.md"), "[link](../target.md)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_markdown_external_url_excluded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](https://example.com/target.md)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_markdown_absolute_path_excluded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](/absolute/target.md)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_markdown_extension_less_excluded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](target)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target).unwrap();

        assert!(backlinks.is_empty());
    }

    // --- rename tag tests ---

    #[test]
    fn rename_tag_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("note.md"),
            "---\nid: note\ntags:\n- foo\n- old-tag\n---\n\nHello world #old-tag here and #old-tag there.",
        )
        .unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        vault.rename_tag("old-tag", "new-tag").unwrap();

        let content = fs::read_to_string(dir.path().join("note.md")).unwrap();
        assert_eq!(
            content,
            "---\nid: note\ntags:\n- foo\n- new-tag\n---\n\nHello world #new-tag here and #new-tag there."
        );
    }

    // --- patch_note tests ---

    #[test]
    fn patch_note_replaces_string() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "Hello world.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path_with_body(dir.path().join("note.md")).unwrap();
        vault.patch_note(&note, "world", "Rust").unwrap();

        let content = fs::read_to_string(dir.path().join("note.md")).unwrap();
        assert_eq!(content, "Hello Rust.");
    }

    #[test]
    fn patch_note_string_not_found_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "Hello world.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path_with_body(dir.path().join("note.md")).unwrap();
        let result = vault.patch_note(&note, "missing", "replacement");

        assert!(matches!(result, Err(VaultError::StringNotFound(_))));
    }

    #[test]
    fn patch_note_multiple_matches_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "foo and foo").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path_with_body(dir.path().join("note.md")).unwrap();
        let result = vault.patch_note(&note, "foo", "bar");

        assert!(matches!(result, Err(VaultError::StringFoundMultipleTimes(_))));
    }

    #[test]
    fn patch_note_preserves_explicit_empty_frontmatter_arrays() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "---\ntags: []\n---\n\nHello world.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path_with_body(dir.path().join("note.md")).unwrap();
        let patched = vault.patch_note(&note, "world", "Rust").unwrap();

        let content = fs::read_to_string(dir.path().join("note.md")).unwrap();
        assert_eq!(content, "---\ntags: []\n---\n\nHello Rust.");
        assert_eq!(
            patched.frontmatter_json().unwrap().get("tags"),
            Some(&serde_json::json!([]))
        );
    }

    #[test]
    fn patch_note_does_not_work_in_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "---\ntitle: Old Title\n---\nBody.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path_with_body(dir.path().join("note.md")).unwrap();
        assert!(vault.patch_note(&note, "Old Title", "New Title").is_err());
    }

    #[test]
    fn patch_note_returns_reloaded_note() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "---\ntitle: Before\n---\n# Before\nBody.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path_with_body(dir.path().join("note.md")).unwrap();
        let patched = vault.patch_note(&note, "Before", "After").unwrap();

        assert_eq!(patched.body, Some("# After\nBody.".to_string()));
    }

    // --- append_to_note tests ---

    #[test]
    fn append_to_note_appends_content_and_returns_reloaded_note() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "Hello.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("note.md")).unwrap();
        let appended = vault.append_to_note(&note, "\nWorld.").unwrap();

        let content = fs::read_to_string(dir.path().join("note.md")).unwrap();
        assert_eq!(content, "Hello.\nWorld.");
        assert_eq!(appended.body, Some("Hello.\nWorld.".to_string()));
    }

    #[test]
    fn append_to_note_preserves_explicit_empty_frontmatter_arrays() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "---\ntags: []\naliases: []\n---\n\nHello.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("note.md")).unwrap();
        let appended = vault.append_to_note(&note, "\nWorld.").unwrap();

        let content = fs::read_to_string(dir.path().join("note.md")).unwrap();
        assert_eq!(content, "---\ntags: []\naliases: []\n---\n\nHello.\nWorld.");
        assert_eq!(
            appended.frontmatter_json().unwrap().get("tags"),
            Some(&serde_json::json!([]))
        );
        assert_eq!(
            appended.frontmatter_json().unwrap().get("aliases"),
            Some(&serde_json::json!([]))
        );
    }

    #[test]
    fn append_to_note_reparses_inline_links_and_tags() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "Start.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("note.md")).unwrap();
        let appended = vault.append_to_note(&note, "\nSee [[target]]. #new-tag").unwrap();

        assert_eq!(appended.links.len(), 1);
        assert!(
            appended
                .tags
                .iter()
                .any(|tag| { tag.tag == "new-tag" && matches!(tag.location, Location::Inline(_)) })
        );
    }

    // --- rename tests ---

    #[test]
    fn rename_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Content.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path_with_body(dir.path().join("old.md")).unwrap();
        let renamed = vault.rename(&note, &dir.path().join("new.md")).unwrap();

        assert!(!dir.path().join("old.md").exists());
        assert!(dir.path().join("new.md").exists());
        assert_eq!(renamed.id, "new");
    }

    #[test]
    fn rename_explicit_id_equals_stem_updated() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old-note.md"), "---\nid: old-note\n---\nContent.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[old-note]].").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old-note.md")).unwrap();
        let renamed = vault.rename(&note, &dir.path().join("new-note.md")).unwrap();

        assert!(!dir.path().join("old-note.md").exists());
        assert!(dir.path().join("new-note.md").exists());
        assert_eq!(renamed.id, "new-note");

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "See [[new-note]].");
    }

    #[test]
    fn rename_explicit_id_differs_from_stem_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("my-note.md"), "---\nid: custom-id\n---\nContent.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[my-note]].").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("my-note.md")).unwrap();
        let renamed = vault.rename(&note, &dir.path().join("renamed-note.md")).unwrap();

        assert_eq!(renamed.id, "custom-id");

        // Wiki link targeting the old stem should be unchanged
        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "See [[my-note]].");
    }

    #[test]
    fn rename_updates_markdown_backlinks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](old.md)").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        vault.rename(&note, &dir.path().join("new.md")).unwrap();

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "[link](new.md)");
    }

    #[test]
    fn rename_updates_wiki_backlinks_by_stem() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old-stem.md"), "Content.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[old-stem]].").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old-stem.md")).unwrap();
        vault.rename(&note, &dir.path().join("new-stem.md")).unwrap();

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "See [[new-stem]].");
    }

    #[test]
    fn rename_leaves_wiki_alias_links_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "---\naliases: [my-alias]\n---\nContent.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[my-alias]].").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("target.md")).unwrap();
        vault.rename(&note, &dir.path().join("renamed-target.md")).unwrap();

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "See [[my-alias]].");
    }

    #[test]
    fn rename_moves_to_different_directory() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("sub");
        fs::create_dir(&subdir).unwrap();
        fs::write(dir.path().join("root.md"), "Root.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](root.md)").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("root.md")).unwrap();
        vault.rename(&note, &subdir.join("root.md")).unwrap();

        assert!(!dir.path().join("root.md").exists());
        assert!(subdir.join("root.md").exists());

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "[link](sub/root.md)");
    }

    #[test]
    fn rename_directory_not_found_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Content.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        let result = vault.rename(&note, &dir.path().join("nonexistent/new.md"));

        assert!(matches!(result, Err(VaultError::DirectoryNotFound(_))));
    }

    #[test]
    fn rename_target_already_exists_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Old.").unwrap();
        fs::write(dir.path().join("new.md"), "Already exists.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        let result = vault.rename(&note, &dir.path().join("new.md"));

        assert!(matches!(result, Err(VaultError::NoteAlreadyExists(_))));
    }

    // --- rename_preview tests ---

    #[test]
    fn rename_preview_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Content.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path_with_body(dir.path().join("old.md")).unwrap();
        let preview = vault.rename_preview(&note, &dir.path().join("new.md")).unwrap();

        assert_eq!(
            preview.new_path,
            common::normalize_path(dir.path().join("new.md"), None)
        );
        assert!(preview.updated_notes.is_empty());
        assert!(preview.id_will_update);
    }

    #[test]
    fn rename_preview_with_wiki_backlink() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[target]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("target.md")).unwrap();
        let preview = vault.rename_preview(&note, &dir.path().join("renamed.md")).unwrap();

        assert_eq!(preview.updated_notes.len(), 1);
        assert!(preview.updated_notes[0].0.ends_with("source.md"));
        assert_eq!(preview.updated_notes[0].1, 1);
    }

    #[test]
    fn rename_preview_with_markdown_backlink() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](target.md)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("target.md")).unwrap();
        let preview = vault.rename_preview(&note, &dir.path().join("renamed.md")).unwrap();

        assert_eq!(preview.updated_notes.len(), 1);
        assert!(preview.updated_notes[0].0.ends_with("source.md"));
        assert_eq!(preview.updated_notes[0].1, 1);
    }

    #[test]
    fn rename_edits_include_exact_backlink_replacements() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "---\nid: target\n---\nTarget.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[target]] and [link](target.md).").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("target.md")).unwrap();
        let edits = vault.rename_edits(&note, &dir.path().join("renamed.md")).unwrap();

        assert_eq!(edits.new_stem, "renamed");
        assert!(edits.id_will_update);
        assert_eq!(edits.backlink_edits.len(), 1);
        assert!(edits.backlink_edits[0].0.ends_with("source.md"));
        let replacements = edits.backlink_edits[0]
            .1
            .iter()
            .map(|(_, new_text)| new_text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(replacements, vec!["[[renamed]]", "[link](renamed.md)"]);
    }

    #[test]
    fn rename_preview_id_will_update() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old-note.md"), "---\nid: old-note\n---\nContent.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old-note.md")).unwrap();
        let preview = vault.rename_preview(&note, &dir.path().join("new-note.md")).unwrap();

        assert!(preview.id_will_update);
    }

    #[test]
    fn rename_preview_id_will_not_update() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("my-note.md"), "---\nid: custom-id\n---\nContent.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("my-note.md")).unwrap();
        let preview = vault
            .rename_preview(&note, &dir.path().join("renamed-note.md"))
            .unwrap();

        assert!(!preview.id_will_update);
    }

    #[test]
    fn rename_preview_excludes_alias_only_links() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "---\naliases: [my-alias]\n---\nContent.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[my-alias]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("target.md")).unwrap();
        let preview = vault.rename_preview(&note, &dir.path().join("renamed.md")).unwrap();

        // The alias link is a backlink but won't be rewritten, so updated_notes is empty
        assert!(preview.updated_notes.is_empty());
    }

    #[test]
    fn rename_preview_does_not_modify_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Content.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[old]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        vault.rename_preview(&note, &dir.path().join("new.md")).unwrap();

        assert!(dir.path().join("old.md").exists());
        assert!(!dir.path().join("new.md").exists());

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "See [[old]].");
    }

    #[test]
    fn rename_preview_directory_not_found() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Content.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        let result = vault.rename_preview(&note, &dir.path().join("nonexistent/new.md"));

        assert!(matches!(result, Err(VaultError::DirectoryNotFound(_))));
    }

    #[test]
    fn rename_preview_target_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Old.").unwrap();
        fs::write(dir.path().join("new.md"), "Already exists.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        let result = vault.rename_preview(&note, &dir.path().join("new.md"));

        assert!(matches!(result, Err(VaultError::NoteAlreadyExists(_))));
    }

    #[test]
    fn rename_preview_updated_notes_sorted_by_path() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("z-source.md"), "See [[target]].").unwrap();
        fs::write(dir.path().join("a-source.md"), "See [[target]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("target.md")).unwrap();
        let preview = vault.rename_preview(&note, &dir.path().join("renamed.md")).unwrap();

        assert_eq!(preview.updated_notes.len(), 2);
        assert!(preview.updated_notes[0].0 < preview.updated_notes[1].0);
    }

    #[test]
    fn rename_markdown_link_with_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("sub");
        fs::create_dir(&subdir).unwrap();
        fs::write(dir.path().join("root.md"), "Root.").unwrap();
        fs::write(subdir.join("source.md"), "[link](root.md)\n[link](sub/target.md)").unwrap();
        fs::write(subdir.join("target.md"), "Target.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();

        {
            let note = Note::from_path(dir.path().join("root.md")).unwrap();
            vault.rename(&note, &dir.path().join("new-root.md")).unwrap();

            let source_content = fs::read_to_string(subdir.join("source.md")).unwrap();
            assert_eq!(source_content, "[link](new-root.md)\n[link](sub/target.md)");
        }

        {
            let note = Note::from_path(subdir.join("target.md")).unwrap();
            vault.rename(&note, &subdir.join("new-target.md")).unwrap();

            let source_content = fs::read_to_string(subdir.join("source.md")).unwrap();
            assert_eq!(source_content, "[link](new-root.md)\n[link](sub/new-target.md)");
        }
    }

    #[test]
    fn rename_multiple_links_same_source() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[first](target.md)\n[second](target.md)").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("target.md")).unwrap();
        vault.rename(&note, &dir.path().join("renamed.md")).unwrap();

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "[first](renamed.md)\n[second](renamed.md)");
    }

    #[test]
    fn rename_preserves_fragment() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Old.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](old.md#section)").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        vault.rename(&note, &dir.path().join("new.md")).unwrap();

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "[link](new.md#section)");
    }

    #[test]
    fn rename_wiki_preserves_heading_and_alias() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old-stem.md"), "Content.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[old-stem#h1|display]].").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old-stem.md")).unwrap();
        vault.rename(&note, &dir.path().join("new-stem.md")).unwrap();

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "See [[new-stem#h1|display]].");
    }

    // --- merge tests ---

    #[test]
    fn merge_basic_creates_dest_and_deletes_sources() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "Body A.").unwrap();
        fs::write(dir.path().join("b.md"), "Body B.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let a = Note::from_path_with_body(dir.path().join("a.md")).unwrap();
        let b = Note::from_path_with_body(dir.path().join("b.md")).unwrap();
        let dest_path = dir.path().join("combined.md");
        vault.merge(&[a, b], &dest_path).unwrap();

        assert!(!dir.path().join("a.md").exists());
        assert!(!dir.path().join("b.md").exists());
        assert!(dest_path.exists());
        let content = fs::read_to_string(&dest_path).unwrap();
        assert!(content.contains("Body A."));
        assert!(content.contains("Body B."));
    }

    #[test]
    fn merge_into_existing_appends_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("src.md"), "Source body.").unwrap();
        fs::write(dir.path().join("dest.md"), "Existing body.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let src = Note::from_path_with_body(dir.path().join("src.md")).unwrap();
        vault.merge(&[src], &dir.path().join("dest.md")).unwrap();

        assert!(!dir.path().join("src.md").exists());
        let content = fs::read_to_string(dir.path().join("dest.md")).unwrap();
        assert!(content.contains("Existing body."));
        assert!(content.contains("Source body."));
    }

    #[test]
    fn merge_unions_tags() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "---\ntags: [rust]\n---\nBody A.").unwrap();
        fs::write(dir.path().join("b.md"), "---\ntags: [obsidian]\n---\nBody B.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let a = Note::from_path_with_body(dir.path().join("a.md")).unwrap();
        let b = Note::from_path_with_body(dir.path().join("b.md")).unwrap();
        let dest_path = dir.path().join("combined.md");
        vault.merge(&[a, b], &dest_path).unwrap();

        let combined = Note::from_path(&dest_path).unwrap();
        assert!(
            combined
                .tags
                .iter()
                .any(|t| t.tag == "rust" && matches!(t.location, Location::Frontmatter))
        );
        assert!(
            combined
                .tags
                .iter()
                .any(|t| t.tag == "obsidian" && matches!(t.location, Location::Frontmatter))
        );
    }

    #[test]
    fn merges_not_inherit_source_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("src.md"),
            "---\nid: source-id\nauthor: alice\n---\nBody.",
        )
        .unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let src = Note::from_path_with_body(dir.path().join("src.md")).unwrap();
        let dest_path = dir.path().join("dest.md");
        vault.merge(&[src], &dest_path).unwrap();

        let dest = Note::from_path(&dest_path).unwrap();
        let fm = dest.frontmatter.unwrap();
        // id must NOT come from source
        assert_ne!(dest.id, "source-id");
        assert!(fm.contains_key("id"));
        // other fields ARE inherited when dest is new
        assert!(fm.contains_key("author"));
    }

    #[test]
    fn merge_other_frontmatter_fields_inherited_from_source_when_dest_is_new() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("src.md"),
            "---\nauthor: alice\ncreated: 2024-01-01\n---\nBody.",
        )
        .unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let src = Note::from_path_with_body(dir.path().join("src.md")).unwrap();
        let dest_path = dir.path().join("dest.md");
        vault.merge(&[src], &dest_path).unwrap();

        let dest = Note::from_path(&dest_path).unwrap();
        let fm = dest.frontmatter.unwrap();
        assert!(fm.contains_key("author"));
        assert!(fm.contains_key("created"));
    }

    #[test]
    fn merge_dest_wins_on_conflicting_fields() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("src.md"), "---\nauthor: alice\n---\nSource.").unwrap();
        fs::write(dir.path().join("dest.md"), "---\nauthor: bob\n---\nDest.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let src = Note::from_path_with_body(dir.path().join("src.md")).unwrap();
        vault.merge(&[src], &dir.path().join("dest.md")).unwrap();

        let dest = Note::from_path(dir.path().join("dest.md")).unwrap();
        let fm = dest.frontmatter.unwrap();
        assert_eq!(fm["author"].as_string().unwrap(), "bob");
    }

    #[test]
    fn merge_updates_wiki_backlinks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("src.md"), "Source.").unwrap();
        fs::write(dir.path().join("linker.md"), "See [[src]].").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let src = Note::from_path_with_body(dir.path().join("src.md")).unwrap();
        vault.merge(&[src], &dir.path().join("dest.md")).unwrap();

        let linker = fs::read_to_string(dir.path().join("linker.md")).unwrap();
        assert_eq!(linker, "See [[dest]].");
    }

    #[test]
    fn merge_source_is_dest_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "Content.").unwrap();

        let mut vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("note.md")).unwrap();
        let result = vault.merge(&[note], &dir.path().join("note.md"));

        assert!(matches!(result, Err(VaultError::MergeSourceIsDestination(_))));
    }

    #[test]
    fn merge_preview_does_not_modify_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("src.md"), "Source.").unwrap();
        fs::write(dir.path().join("linker.md"), "See [[src]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let src = Note::from_path_with_body(dir.path().join("src.md")).unwrap();
        vault.merge_preview(&[src], dir.path().join("dest.md")).unwrap();

        assert!(dir.path().join("src.md").exists());
        assert!(!dir.path().join("dest.md").exists());
        let linker = fs::read_to_string(dir.path().join("linker.md")).unwrap();
        assert_eq!(linker, "See [[src]].");
    }
}
