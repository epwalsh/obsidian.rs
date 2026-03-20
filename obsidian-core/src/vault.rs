use std::path::{Path, PathBuf};

use gray_matter::Pod;

use crate::{Link, LocatedLink, Note, NoteError, VaultError, search};
use rayon::prelude::*;

pub struct Vault {
    pub path: PathBuf,
}

/// Normalizes a path by resolving `.` and `..` components without touching the filesystem.
fn normalize_path(path: &std::path::Path) -> PathBuf {
    let mut components: Vec<std::path::Component> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(components.last(), Some(std::path::Component::Normal(_))) {
                    components.pop();
                }
            }
            c => components.push(c),
        }
    }
    components.iter().collect()
}

/// Computes a relative path from `from_dir` to `to_file`.
/// Both arguments must be absolute paths.
fn relative_path(from_dir: &Path, to_file: &Path) -> PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = to_file.components().collect();
    let common = from.iter().zip(to.iter()).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in 0..(from.len() - common) {
        result.push("..");
    }
    for c in &to[common..] {
        result.push(c);
    }
    result
}

/// Rewrites link spans in `raw_content` according to `replacements`.
/// Each entry is a `(LocatedLink, new_text)` pair; `new_text` replaces the original span.
/// Multiple replacements on the same line are applied right-to-left to preserve offsets.
fn rewrite_links(raw_content: &str, replacements: Vec<(LocatedLink, String)>) -> String {
    use std::collections::HashMap;

    // Map line number (1-indexed) → indices into `replacements`
    let mut by_line: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, (ll, _)) in replacements.iter().enumerate() {
        by_line.entry(ll.location.line).or_default().push(i);
    }

    let trailing_newline = raw_content.ends_with('\n');
    let mut result_lines: Vec<String> = Vec::new();

    for (line_idx, line) in raw_content.lines().enumerate() {
        let line_num = line_idx + 1;
        if let Some(indices) = by_line.get(&line_num) {
            // Sort right-to-left so each splice doesn't shift earlier column offsets
            let mut sorted = indices.clone();
            sorted.sort_by(|&a, &b| {
                replacements[b]
                    .0
                    .location
                    .col_start
                    .cmp(&replacements[a].0.location.col_start)
            });

            let mut chars: Vec<char> = line.chars().collect();
            for idx in sorted {
                let (ll, new_text) = &replacements[idx];
                let new_chars: Vec<char> = new_text.chars().collect();
                chars.splice(ll.location.col_start..ll.location.col_end, new_chars);
            }
            result_lines.push(chars.into_iter().collect());
        } else {
            result_lines.push(line.to_string());
        }
    }

    let mut result = result_lines.join("\n");
    if trailing_newline {
        result.push('\n');
    }
    result
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

impl Vault {
    /// Opens a vault at the given path, returning an error if the path does not exist or is not a
    /// directory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VaultError> {
        let path = path.as_ref().to_path_buf();
        if !path.is_dir() {
            return Err(VaultError::NotADirectory(path));
        }
        Ok(Vault { path })
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

    /// Loads all notes in the vault in parallel.
    pub fn notes(&self) -> Vec<Result<Note, NoteError>> {
        search::find_notes(&self.path)
    }

    /// Returns a [`SearchQuery`](search::SearchQuery) rooted at this vault's path.
    pub fn search(&self) -> search::SearchQuery {
        search::SearchQuery::new(&self.path)
    }

    /// Returns all notes in the vault that link to `target`, paired with the specific
    /// [`LocatedLink`]s within each note that point to it.
    ///
    /// Only wiki links (`[[target]]`) and markdown links (`[text](target.md)`) are
    /// considered. Embed links are excluded. Notes that fail to load are silently skipped.
    pub fn backlinks(&self, target: &Note) -> Vec<(Note, Vec<LocatedLink>)> {
        let target_stem = target.path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string());

        let paths: Vec<_> = search::find_note_paths(&self.path).collect();
        paths
            .into_par_iter()
            .filter_map(|path| {
                let source = Note::from_path(&path).ok()?;
                if source.path == target.path {
                    return None;
                }
                let matching: Vec<LocatedLink> = source
                    .links()
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
                            (normalize_path(&source_dir.join(url_path)) == target.path)
                                || (url_path == relative_path(self.path.as_path(), &target.path).to_string_lossy())
                        }
                        _ => false,
                    })
                    .collect();
                if matching.is_empty() {
                    None
                } else {
                    Some((source, matching))
                }
            })
            .collect()
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
        let frontmatter_id_will_update =
            id_needs_update && note.frontmatter.as_ref().is_some_and(|fm| fm.contains_key("id"));

        let backlinks = self.backlinks(note);
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
                        let new_url = relative_path(self.path.as_path(), new_path);
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
            frontmatter_id_will_update,
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
    pub fn rename(&self, note: &Note, new_path: &Path) -> Result<Note, VaultError> {
        let op = self.compute_rename_op(note, new_path)?;

        std::fs::rename(&note.path, new_path)?;

        let mut renamed = Note::from_path(new_path)?;

        // Update explicit frontmatter `id` when it matched the old stem.
        if op.frontmatter_id_will_update {
            renamed
                .frontmatter
                .as_mut()
                .unwrap()
                .insert("id".to_string(), Pod::String(op.new_stem.clone()));
            renamed.write()?;
            renamed = Note::from_path(new_path)?;
        }

        for (source_note, replacements) in op.per_note_replacements {
            let raw_content = std::fs::read_to_string(&source_note.path)?;
            let new_content = rewrite_links(&raw_content, replacements);
            std::fs::write(&source_note.path, new_content)?;
        }

        Ok(renamed)
    }

    /// Returns a preview of what [`rename`](Self::rename) would change without touching the filesystem.
    ///
    /// Same validation and error variants as `rename`.
    pub fn rename_preview(&self, note: &Note, new_path: &Path) -> Result<RenamePreview, VaultError> {
        let op = self.compute_rename_op(note, new_path)?;

        let mut updated_notes: Vec<(PathBuf, usize)> = op
            .per_note_replacements
            .iter()
            .map(|(source_note, replacements)| (source_note.path.clone(), replacements.len()))
            .collect();
        updated_notes.sort_by(|(a, _), (b, _)| a.cmp(b));

        Ok(RenamePreview {
            new_path: new_path.to_path_buf(),
            id_will_update: op.frontmatter_id_will_update,
            updated_notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
        let vault = Vault::open(dir.path()).expect("should open valid directory");
        assert_eq!(vault.path, dir.path());
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

    #[test]
    fn normalize_path_removes_dot() {
        assert_eq!(normalize_path(&PathBuf::from("/a/./b")), PathBuf::from("/a/b"));
    }

    #[test]
    fn normalize_path_resolves_double_dot() {
        assert_eq!(normalize_path(&PathBuf::from("/a/b/../c")), PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_path_deep_traversal() {
        assert_eq!(normalize_path(&PathBuf::from("/a/b/c/../../d")), PathBuf::from("/a/d"));
    }

    #[test]
    fn normalize_path_traversal_beyond_root_stops_at_root() {
        // /a/../../b: after processing, ends up as /b (the extra .. can't go above /)
        assert_eq!(normalize_path(&PathBuf::from("/a/../../b")), PathBuf::from("/b"));
    }

    #[test]
    fn backlinks_wiki_by_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "---\nid: my-id\n---\nTarget.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[my-id]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

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
        let backlinks = vault.backlinks(&target);

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
        let backlinks = vault.backlinks(&target);

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_wiki_by_title() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "# My Title\n\nContent.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[My Title]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_wiki_with_heading_suffix() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[target#section]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_excludes_self() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Self link: [[target]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_excludes_notes_with_no_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("other.md"), "No links here.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_returns_all_matching_links_from_one_note() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "See [[target]] and also [[target]].").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

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
        let backlinks = vault.backlinks(&target);

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_markdown_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](target.md)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

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
        let backlinks = vault.backlinks(&target);

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
        let backlinks = vault.backlinks(&target);

        assert_eq!(backlinks.len(), 1);
    }

    #[test]
    fn backlinks_markdown_external_url_excluded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](https://example.com/target.md)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_markdown_absolute_path_excluded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](/absolute/target.md)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

        assert!(backlinks.is_empty());
    }

    #[test]
    fn backlinks_markdown_extension_less_excluded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.md"), "Target.").unwrap();
        fs::write(dir.path().join("source.md"), "[link](target)").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let target = Note::from_path(dir.path().join("target.md")).unwrap();
        let backlinks = vault.backlinks(&target);

        assert!(backlinks.is_empty());
    }

    // --- rename tests ---

    #[test]
    fn rename_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Content.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        let result = vault.rename(&note, &dir.path().join("nonexistent/new.md"));

        assert!(matches!(result, Err(VaultError::DirectoryNotFound(_))));
    }

    #[test]
    fn rename_target_already_exists_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "Old.").unwrap();
        fs::write(dir.path().join("new.md"), "Already exists.").unwrap();

        let vault = Vault::open(dir.path()).unwrap();
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
        let note = Note::from_path(dir.path().join("old.md")).unwrap();
        let preview = vault.rename_preview(&note, &dir.path().join("new.md")).unwrap();

        assert_eq!(preview.new_path, dir.path().join("new.md"));
        assert!(preview.updated_notes.is_empty());
        assert!(!preview.id_will_update);
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

        let vault = Vault::open(dir.path()).unwrap();

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

        let vault = Vault::open(dir.path()).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
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

        let vault = Vault::open(dir.path()).unwrap();
        let note = Note::from_path(dir.path().join("old-stem.md")).unwrap();
        vault.rename(&note, &dir.path().join("new-stem.md")).unwrap();

        let source_content = fs::read_to_string(dir.path().join("source.md")).unwrap();
        assert_eq!(source_content, "See [[new-stem#h1|display]].");
    }
}
