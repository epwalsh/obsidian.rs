use std::path::{Path, PathBuf};

use crate::{Link, LocatedLink, Note, search};
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

impl Vault {
    /// Opens a vault at the given path, returning an error if the path does not exist or is not a
    /// directory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let path = path.as_ref().to_path_buf();
        if !path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not a directory", path.display()),
            ));
        }
        Ok(Vault { path })
    }

    /// Loads all notes in the vault in parallel.
    pub fn notes(&self) -> Vec<Result<Note, std::io::Error>> {
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
        let target_stem = target
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());

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
                            target: wiki_target,
                            ..
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
                            normalize_path(&source_dir.join(url_path)) == target.path
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
        assert_eq!(
            normalize_path(&PathBuf::from("/a/./b")),
            PathBuf::from("/a/b")
        );
    }

    #[test]
    fn normalize_path_resolves_double_dot() {
        assert_eq!(
            normalize_path(&PathBuf::from("/a/b/../c")),
            PathBuf::from("/a/c")
        );
    }

    #[test]
    fn normalize_path_deep_traversal() {
        assert_eq!(
            normalize_path(&PathBuf::from("/a/b/c/../../d")),
            PathBuf::from("/a/d")
        );
    }

    #[test]
    fn normalize_path_traversal_beyond_root_stops_at_root() {
        // /a/../../b: after processing, ends up as /b (the extra .. can't go above /)
        assert_eq!(
            normalize_path(&PathBuf::from("/a/../../b")),
            PathBuf::from("/b")
        );
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
        fs::write(
            dir.path().join("my-note.md"),
            "---\nid: custom-id\n---\nTarget.",
        )
        .unwrap();
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
        fs::write(
            dir.path().join("target.md"),
            "---\naliases: [t-alias]\n---\nTarget.",
        )
        .unwrap();
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
        fs::write(
            dir.path().join("source.md"),
            "See [[target]] and also [[target]].",
        )
        .unwrap();

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
        fs::write(
            dir.path().join("source.md"),
            "[link](https://example.com/target.md)",
        )
        .unwrap();

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
}
