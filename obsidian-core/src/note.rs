use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::NoteError;

use gray_matter::{Matter, Pod, engine::YAML};
use indexmap::IndexMap;

pub struct Note {
    pub path: PathBuf,
    pub id: String,
    pub title: Option<String>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    /// Body text stripped of frontmatter. `None` when the note was loaded without content
    /// (the default). Use [`Note::from_path_with_content`] or [`Note::load_content`] to
    /// populate this field. Required for [`Note::write`].
    pub content: Option<String>,
    /// Links extracted from the body at load time (always populated).
    pub links: Vec<crate::LocatedLink>,
    /// Inline tags extracted from the body at load time (always populated).
    pub inline_tags: Vec<crate::LocatedTag>,
    pub frontmatter: Option<IndexMap<String, Pod>>,
    /// Number of lines occupied by the frontmatter block (including delimiters).
    /// Used to offset link locations so they reflect positions in the original file.
    pub frontmatter_line_count: usize,
}

impl Note {
    /// Parses a note from a raw file string, always retaining the body content.
    ///
    /// Useful for constructing notes from in-memory strings (e.g. in tests).
    /// For file-backed notes prefer [`Note::from_path`] (no content) or
    /// [`Note::from_path_with_content`] (with content).
    pub fn parse(path: impl AsRef<Path>, content: &str) -> Self {
        Self::parse_impl(path.as_ref(), content, true)
    }

    fn parse_impl(path: &Path, content: &str, include_content: bool) -> Self {
        let matter = Matter::<YAML>::new();
        let (body, frontmatter) = match matter.parse(content) {
            Ok(parsed) => {
                let fm = parsed.data.and_then(|pod: Pod| pod.as_hashmap().ok()).map(|hm| {
                    let mut entries: Vec<_> = hm.into_iter().collect();
                    entries.sort_by(|a, b| a.0.cmp(&b.0));
                    entries.into_iter().collect::<IndexMap<_, _>>()
                });
                (parsed.content, fm)
            }
            Err(_) => (content.to_string(), None),
        };
        let frontmatter_line_count = content.lines().count().saturating_sub(body.lines().count());
        let id = frontmatter
            .as_ref()
            .and_then(|fm| fm.get("id"))
            .and_then(|p| p.as_string().ok())
            .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
            .unwrap_or_default();
        let mut title = frontmatter
            .as_ref()
            .and_then(|fm| fm.get("title"))
            .and_then(|p| p.as_string().ok())
            .or_else(|| find_h1(&body));
        let aliases = {
            let mut v: Vec<String> = frontmatter
                .as_ref()
                .and_then(|fm| fm.get("aliases"))
                .and_then(|p| p.as_vec().ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.as_string().ok())
                .collect();

            // If there's a title, it should be an alias too, and if there's not a title we should
            // infer it from the first alias
            if let Some(ref t) = title {
                let clean = strip_title_md(t);
                if !v.contains(&clean) {
                    v.push(clean);
                }
            } else if !v.is_empty() {
                title = Some(v[0].clone());
            }
            v
        };
        let tags: Vec<String> = frontmatter
            .as_ref()
            .and_then(|fm| fm.get("tags"))
            .and_then(|p| p.as_vec().ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| p.as_string().ok())
            .collect();
        let offset = frontmatter_line_count;
        let links = crate::link::parse_links(&body)
            .into_iter()
            .map(|mut ll| {
                ll.location.line += offset;
                ll
            })
            .collect();
        let inline_tags = crate::tag::parse_inline_tags(&body)
            .into_iter()
            .map(|mut lt| {
                lt.location.line += offset;
                lt
            })
            .collect();

        Note {
            path: path.to_path_buf(),
            id,
            title,
            aliases,
            tags,
            content: if include_content { Some(body) } else { None },
            links,
            inline_tags,
            frontmatter,
            frontmatter_line_count,
        }
    }

    /// Loads a note from disk without retaining the body content.
    ///
    /// Links and inline tags are still extracted and stored. This is the
    /// memory-efficient default for bulk operations. Use
    /// [`from_path_with_content`](Self::from_path_with_content) when the body
    /// text is needed (e.g. for content search or writing).
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, NoteError> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)?;
        Ok(Self::parse_impl(path, &raw, false))
    }

    /// Loads a note from disk, retaining the full body content in [`Note::content`].
    pub fn from_path_with_content(path: impl AsRef<Path>) -> Result<Self, NoteError> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)?;
        Ok(Self::parse_impl(path, &raw, true))
    }

    /// Reloads the note from its path without retaining body content.
    pub fn reload(self) -> Result<Self, NoteError> {
        Self::from_path(&self.path)
    }

    /// Reloads the note from its path while retaining body content.
    pub fn reload_with_content(self) -> Result<Self, NoteError> {
        Self::from_path_with_content(&self.path)
    }

    /// Populates [`Note::content`] by reading the note's body from disk.
    /// Does nothing if content is already loaded.
    pub fn load_content(&mut self) -> Result<(), NoteError> {
        if self.content.is_none() {
            let raw = std::fs::read_to_string(&self.path)?;
            let matter = Matter::<YAML>::new();
            let body = match matter.parse::<Pod>(&raw) {
                Ok(parsed) => parsed.content,
                Err(_) => raw,
            };
            self.content = Some(body);
        }
        Ok(())
    }

    /// Add an alias.
    pub fn add_alias(&mut self, alias: String) {
        if !self.aliases.contains(&alias) {
            self.aliases.push(alias);
        }
    }

    /// Add a tag.
    pub fn add_tag(&mut self, tag: String) {
        if !self.tags.contains(&tag) {
            self.tags.push(tag);
        }
    }

    /// Remove a tag.
    pub fn remove_tag(&mut self, tag: &str) {
        self.tags.retain(|t| t != tag);
    }

    /// Set an arbitrary frontmatter field to a value (which can be any YAML type).
    /// A null value removes the field from the frontmatter.
    pub fn set_field(&mut self, key: &str, value: &serde_yaml::Value) -> Result<(), NoteError> {
        // Guard against invalid field names that would cause YAML serialization to fail (e.g. containing newlines),
        // or that would be confusing to users (e.g. "id", "aliases", "tags" which are derived from other fields and would be ignored).
        if key.contains('\n') {
            return Err(NoteError::InvalidFieldName(
                "field names cannot contain newlines".to_string(),
            ));
        }
        if ["id", "title", "aliases", "tags"].contains(&key) {
            return Err(NoteError::InvalidFieldName(format!(
                "'{}' is a reserved field name and cannot be set this way",
                key
            )));
        }

        if self.frontmatter.is_none() {
            self.frontmatter = Some(IndexMap::new());
        }

        if value.is_null() {
            // Remove the field if value is null.
            self.frontmatter.as_mut().unwrap().shift_remove(key);
        } else {
            self.frontmatter
                .as_mut()
                .unwrap()
                .insert(key.to_string(), yaml_to_pod_value(value));
        }

        Ok(())
    }

    /// Atomically writes the note to `self.path`, including serialized frontmatter.
    ///
    /// Requires [`Note::content`] to be populated. Returns
    /// [`NoteError::ContentNotLoaded`] if content is `None`.
    ///
    /// Frontmatter keys are serialized in a deterministic order: `id` first, then
    /// `title` (if present), then `aliases`, then `tags`, then all remaining keys
    /// sorted alphabetically.
    pub fn write(&self) -> Result<(), NoteError> {
        let content = self.read(true)?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(content.as_bytes())?;
        tmp.persist(&self.path).map_err(|e| e.error)?;
        Ok(())
    }

    /// Atomically writes updated frontmatter to `self.path`, reading the current body
    /// from disk. Use this when only frontmatter has changed and content was not loaded.
    pub fn write_frontmatter(&self) -> Result<(), NoteError> {
        let raw = std::fs::read_to_string(&self.path)?;
        let matter = Matter::<YAML>::new();
        let body = match matter.parse::<Pod>(&raw) {
            Ok(parsed) => parsed.content,
            Err(_) => raw.clone(),
        };
        let file_content = self.to_file_content(&body)?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(file_content.as_bytes())?;
        tmp.persist(&self.path).map_err(|e| e.error)?;
        Ok(())
    }

    /// Read the contents of the note as a string, optionally including frontmatter.
    /// Requires [`Note::content`] to be populated. Returns
    /// [`NoteError::ContentNotLoaded`] if content is `None`.
    pub fn read(&self, include_frontmatter: bool) -> Result<String, NoteError> {
        let body = self.content.as_deref().ok_or(NoteError::ContentNotLoaded)?;
        if include_frontmatter {
            let file_content = self.to_file_content(body)?;
            Ok(file_content)
        } else {
            Ok(body.to_string())
        }
    }

    /// Get the note's frontmatter map.
    pub fn frontmatter_map(&self) -> IndexMap<String, Pod> {
        let mut fm = if let Some(fm) = &self.frontmatter {
            fm.clone()
        } else {
            // No frontmatter; create it.
            IndexMap::new()
        };

        // Make sure fields are up-to-date.
        fm.insert("id".to_string(), Pod::String(self.id.clone()));
        if self.aliases.is_empty() {
            // No aliases; remove the field to avoid emitting an empty array.
            fm.shift_remove("aliases");
        } else {
            fm.insert(
                "aliases".to_string(),
                Pod::Array(self.aliases.iter().cloned().map(Pod::String).collect()),
            );
        }
        if self.tags.is_empty() {
            // No tags; remove the field to avoid emitting an empty array.
            fm.shift_remove("tags");
        } else {
            fm.insert(
                "tags".to_string(),
                Pod::Array(self.tags.iter().cloned().map(Pod::String).collect()),
            );
        }
        fm
    }

    /// Get the note's frontmatter map in a form suitable for YAML serialization.
    pub fn frontmatter_yaml(&self) -> Result<serde_yaml::Mapping, serde_yaml::Error> {
        let fm = self.frontmatter_map();

        const PRIORITY_KEYS: &[&str] = &["id", "title", "aliases", "tags"];
        let mut mapping = serde_yaml::Mapping::new();
        // Emit priority keys in fixed order, only if present.
        for key in PRIORITY_KEYS {
            if let Some(v) = fm.get(*key) {
                mapping.insert(serde_yaml::Value::String((*key).to_string()), pod_to_yaml_value(v));
            }
        }
        // Emit remaining keys in alphabetical order.
        let mut rest: Vec<_> = fm
            .iter()
            .filter(|(k, _)| !PRIORITY_KEYS.contains(&k.as_str()))
            .collect();
        rest.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in rest {
            mapping.insert(serde_yaml::Value::String(k.clone()), pod_to_yaml_value(v));
        }
        Ok(mapping)
    }

    /// Get the note's frontmatter map in a form suitable for JSON serialization.
    pub fn frontmatter_json(&self) -> Result<serde_json::Map<String, serde_json::Value>, NoteError> {
        let fm = self.frontmatter_map();
        let mut mapping = serde_json::Map::new();
        for (k, v) in fm {
            mapping.insert(k, pod_to_json_value(&v)?);
        }
        Ok(mapping)
    }

    /// Get the note's frontmatter as a YAML string (without delimiters).
    pub fn frontmatter_string(&self) -> Result<String, serde_yaml::Error> {
        let fm = self.frontmatter_yaml()?;
        let yaml = serde_yaml::to_string(&fm)?;
        // Strip leading "---\n" if emitted by serde_yaml, since we'll add our own delimiters.
        Ok(yaml.strip_prefix("---\n").unwrap_or(&yaml).to_string())
    }

    fn to_file_content(&self, body: &str) -> Result<String, serde_yaml::Error> {
        let fm = self.frontmatter_string()?;
        Ok(format!("---\n{}---\n\n{}", fm, body))
    }
}

fn pod_to_yaml_value(pod: &Pod) -> serde_yaml::Value {
    match pod {
        Pod::Null => serde_yaml::Value::Null,
        Pod::String(s) => serde_yaml::Value::String(s.clone()),
        Pod::Integer(i) => serde_yaml::Value::Number((*i).into()),
        Pod::Float(f) => serde_yaml::Value::Number(serde_yaml::Number::from(*f)),
        Pod::Boolean(b) => serde_yaml::Value::Bool(*b),
        Pod::Array(arr) => serde_yaml::Value::Sequence(arr.iter().map(pod_to_yaml_value).collect()),
        Pod::Hash(map) => serde_yaml::Value::Mapping(
            map.iter()
                .map(|(k, v)| (serde_yaml::Value::String(k.clone()), pod_to_yaml_value(v)))
                .collect(),
        ),
    }
}

fn yaml_to_pod_value(yaml: &serde_yaml::Value) -> Pod {
    match yaml {
        serde_yaml::Value::Null => Pod::Null,
        serde_yaml::Value::String(s) => Pod::String(s.clone()),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Pod::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Pod::Float(f)
            } else {
                // This should never happen since serde_yaml::Number can only be i64 or f64.
                Pod::Null
            }
        }
        serde_yaml::Value::Bool(b) => Pod::Boolean(*b),
        serde_yaml::Value::Sequence(seq) => Pod::Array(seq.iter().map(yaml_to_pod_value).collect()),
        serde_yaml::Value::Mapping(map) => Pod::Hash(
            map.iter()
                .filter_map(|(k, v)| k.as_str().map(|ks| (ks.to_string(), yaml_to_pod_value(v))))
                .collect(),
        ),
        serde_yaml::Value::Tagged(_) => {
            // YAML tags are not supported in our frontmatter; treat them as null.
            Pod::Null
        }
    }
}

fn pod_to_json_value(pod: &Pod) -> Result<serde_json::Value, NoteError> {
    match pod {
        Pod::Null => Ok(serde_json::Value::Null),
        Pod::String(s) => Ok(serde_json::Value::String(s.clone())),
        Pod::Integer(i) => Ok(serde_json::Value::Number((*i).into())),
        Pod::Float(f) => Ok(serde_json::Value::Number(
            serde_json::Number::from_f64(*f).ok_or_else(|| NoteError::Json(format!("invalid float value: {}", f)))?,
        )),
        Pod::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        Pod::Array(arr) => {
            let result: Result<Vec<serde_json::Value>, NoteError> = arr.iter().map(pod_to_json_value).collect();
            Ok(serde_json::Value::Array(result?))
        }
        Pod::Hash(map) => {
            let result: Result<serde_json::Map<String, serde_json::Value>, NoteError> = map
                .iter()
                .map(|(k, v)| pod_to_json_value(v).map(|json_v| (k.clone(), json_v)))
                .collect();
            result.map(serde_json::Value::Object)
        }
    }
}

fn find_h1(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("# ").map(|t| t.trim_end().to_string()))
}

fn strip_title_md(s: &str) -> String {
    // [[target|alias]] → alias, [[target]] or [[target#heading]] → target
    static WIKI_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"!?\[\[([^\]#|]*?)(?:#[^\]|]*?)?(?:\|([^\]]*?))?\]\]").unwrap());
    // [text](url) → text
    static MD_LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+?)\]\([^)]*?\)").unwrap());
    // `code` → code
    static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`\n]+)`").unwrap());

    let s = WIKI_RE.replace_all(s, |caps: &regex::Captures| {
        caps.get(2)
            .or_else(|| caps.get(1))
            .map_or("", |m| m.as_str())
            .to_string()
    });
    let s = MD_LINK_RE.replace_all(&s, "$1");
    let s = INLINE_CODE_RE.replace_all(&s, "$1");
    s.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_with_frontmatter() {
        let input = "---\ntitle: My Note\ntags: [rust, obsidian]\n---\n\nHello, world!";
        let note = Note::parse("/vault/my-note.md", input);

        assert_eq!(note.path, PathBuf::from("/vault/my-note.md"));
        assert_eq!(note.content.as_deref().unwrap().trim(), "Hello, world!");

        let fm = note.frontmatter.expect("should have frontmatter");
        assert!(fm.contains_key("title"));
        assert!(fm.contains_key("tags"));
    }

    #[test]
    fn parse_without_frontmatter() {
        let input = "Just some plain markdown content.";
        let note = Note::parse("/vault/plain.md", input);

        assert!(note.frontmatter.is_none());
        assert_eq!(note.content.as_deref().unwrap(), input);
    }

    #[test]
    fn from_path_reads_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "---\nauthor: Pete\n---\n\nBody text.").unwrap();

        let note = Note::from_path_with_content(tmp.path()).expect("should read file");
        let fm = note.frontmatter.expect("should have frontmatter");
        assert!(fm.contains_key("author"));
        assert!(note.content.unwrap().contains("Body text."));
    }

    #[test]
    fn id_from_frontmatter() {
        let input = "---\nid: custom-id\n---\n\nContent.";
        let note = Note::parse("/vault/my-note.md", input);
        assert_eq!(note.id, "custom-id");
    }

    #[test]
    fn id_falls_back_to_filename_stem() {
        let input = "---\nauthor: Pete\n---\n\nContent.";
        let note = Note::parse("/vault/my-note.md", input);
        assert_eq!(note.id, "my-note");
    }

    #[test]
    fn id_from_stem_when_no_frontmatter() {
        let note = Note::parse("/vault/another-note.md", "Just content.");
        assert_eq!(note.id, "another-note");
    }

    #[test]
    fn title_from_frontmatter() {
        let input = "---\ntitle: FM Title\n---\n\n# H1 Title\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        // frontmatter takes precedence over H1
        assert_eq!(note.title.as_deref(), Some("FM Title"));
    }

    #[test]
    fn title_from_h1() {
        let input = "# My Heading\n\nSome content.";
        let note = Note::parse("/vault/note.md", input);
        assert_eq!(note.title.as_deref(), Some("My Heading"));
    }

    #[test]
    fn title_none_when_absent() {
        let note = Note::parse("/vault/note.md", "No heading here.");
        assert!(note.title.is_none());
    }

    #[test]
    fn aliases_from_frontmatter_include_title() {
        let input = "---\ntitle: My Note\naliases: [alias-one, alias-two]\n---\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert!(note.aliases.contains(&"alias-one".to_string()));
        assert!(note.aliases.contains(&"alias-two".to_string()));
        assert!(note.aliases.contains(&"My Note".to_string()));
    }

    #[test]
    fn aliases_title_not_duplicated_when_already_present() {
        let input = "---\ntitle: My Note\naliases: [My Note, other-alias]\n---\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert_eq!(note.aliases.iter().filter(|a| *a == "My Note").count(), 1);
    }

    #[test]
    fn aliases_just_title_when_no_frontmatter_aliases() {
        let input = "---\ntitle: My Note\n---\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert_eq!(note.aliases, vec!["My Note".to_string()]);
    }

    #[test]
    fn aliases_empty_when_no_title_and_no_frontmatter_aliases() {
        let note = Note::parse("/vault/note.md", "No heading here.");
        assert!(note.aliases.is_empty());
    }

    #[test]
    fn aliases_includes_h1_title_when_no_frontmatter() {
        let input = "# H1 Title\n\nSome content.";
        let note = Note::parse("/vault/note.md", input);
        assert_eq!(note.aliases, vec!["H1 Title".to_string()]);
    }

    #[test]
    fn tags_from_frontmatter() {
        let input = "---\ntags: [rust, obsidian]\n---\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert_eq!(note.tags, vec!["rust".to_string(), "obsidian".to_string()]);
    }

    #[test]
    fn tags_empty_when_absent() {
        let note = Note::parse("/vault/note.md", "No frontmatter here.");
        assert!(note.tags.is_empty());
    }

    #[test]
    fn write_frontmatter_key_ordering() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // Provide keys out of order; verify they are written in the canonical order.
        std::fs::write(
            tmp.path(),
            "---\nzebra: last\ntags: [t]\naliases: [a]\ntitle: T\nid: my-id\nauthor: Pete\n---\n\nContent.",
        )
        .unwrap();

        let note = Note::from_path_with_content(tmp.path()).unwrap();
        note.write().unwrap();

        let on_disk = std::fs::read_to_string(tmp.path()).unwrap();
        // Extract only key lines (not list item lines that start with '-').
        let keys: Vec<&str> = on_disk
            .lines()
            .skip(1) // skip opening "---"
            .take_while(|l| *l != "---")
            .filter(|l| !l.starts_with('-'))
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(keys, vec!["id", "title", "aliases", "tags", "author", "zebra"]);
    }

    #[test]
    fn write_frontmatter_key_ordering_no_title() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "---\ntags: [t]\nid: my-id\nzebra: last\n---\n\nContent.").unwrap();

        let note = Note::from_path_with_content(tmp.path()).unwrap();
        note.write().unwrap();

        let on_disk = std::fs::read_to_string(tmp.path()).unwrap();
        let keys: Vec<&str> = on_disk
            .lines()
            .skip(1)
            .take_while(|l| *l != "---")
            .filter(|l| !l.starts_with('-'))
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(keys, vec!["id", "tags", "zebra"]);
    }

    #[test]
    fn write_round_trips_note_without_frontmatter() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let original = "Just some plain content.";
        std::fs::write(tmp.path(), original).unwrap();

        let note = Note::from_path_with_content(tmp.path()).unwrap();
        note.write().unwrap();

        let on_disk = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(
            on_disk,
            format!(
                "---\nid: {}\n---\n\n{}",
                tmp.path().file_stem().unwrap().display().to_string(),
                original
            )
        );
    }

    #[test]
    fn write_round_trips_note_with_frontmatter() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let original = "---\ntitle: My Note\n---\n\nBody text.";
        std::fs::write(tmp.path(), original).unwrap();

        let note = Note::from_path_with_content(tmp.path()).unwrap();
        note.write().unwrap();

        // Re-parse to verify the on-disk content is valid and retains key fields.
        let reparsed = Note::from_path_with_content(tmp.path()).unwrap();
        assert_eq!(reparsed.title.as_deref(), Some("My Note"));
        assert_eq!(reparsed.content.as_deref().unwrap().trim(), "Body text.");
    }

    #[test]
    fn write_reflects_frontmatter_mutation() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "---\ntitle: Old Title\n---\n\nContent.").unwrap();

        let mut note = Note::from_path_with_content(tmp.path()).unwrap();
        note.frontmatter
            .as_mut()
            .unwrap()
            .insert("title".to_string(), Pod::String("New Title".to_string()));
        note.write().unwrap();

        let reparsed = Note::from_path(tmp.path()).unwrap();
        assert_eq!(reparsed.title.as_deref(), Some("New Title"));
    }

    // strip_title_md unit tests

    #[test]
    fn strip_title_md_plain_is_unchanged() {
        assert_eq!(strip_title_md("My Note"), "My Note");
    }

    #[test]
    fn strip_title_md_wiki_link_no_alias() {
        assert_eq!(strip_title_md("[[linked note]]"), "linked note");
    }

    #[test]
    fn strip_title_md_wiki_link_with_alias() {
        assert_eq!(strip_title_md("[[note|display text]]"), "display text");
    }

    #[test]
    fn strip_title_md_wiki_link_with_heading() {
        assert_eq!(strip_title_md("[[note#heading]]"), "note");
    }

    #[test]
    fn strip_title_md_markdown_link() {
        assert_eq!(strip_title_md("[text](https://example.com)"), "text");
    }

    #[test]
    fn strip_title_md_inline_code() {
        assert_eq!(strip_title_md("`code` stuff"), "code stuff");
    }

    #[test]
    fn strip_title_md_mixed() {
        assert_eq!(strip_title_md("My [[note|ref]] and `stuff`"), "My ref and stuff");
    }

    // Integration tests: aliases use cleaned title

    #[test]
    fn alias_from_h1_with_wiki_link_no_alias() {
        let input = "# [[linked note]]\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert_eq!(note.title.as_deref(), Some("[[linked note]]"));
        assert!(note.aliases.contains(&"linked note".to_string()));
    }

    #[test]
    fn alias_from_h1_with_wiki_link_with_alias() {
        let input = "# [[note|display text]]\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert!(note.aliases.contains(&"display text".to_string()));
    }

    #[test]
    fn alias_from_h1_with_markdown_link() {
        let input = "# [text](https://example.com)\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert!(note.aliases.contains(&"text".to_string()));
    }

    #[test]
    fn alias_from_h1_with_inline_code() {
        let input = "# `code` stuff\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert!(note.aliases.contains(&"code stuff".to_string()));
    }

    #[test]
    fn alias_from_h1_mixed_markdown() {
        let input = "# My [[note|ref]] and `stuff`\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert!(note.aliases.contains(&"My ref and stuff".to_string()));
    }

    #[test]
    fn alias_from_frontmatter_title_with_wiki_link() {
        let input = "---\ntitle: \"[[note|display]]\"\n---\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert!(note.aliases.contains(&"display".to_string()));
    }

    #[test]
    fn alias_plain_title_unchanged() {
        let input = "# My Note\n\nContent.";
        let note = Note::parse("/vault/note.md", input);
        assert!(note.aliases.contains(&"My Note".to_string()));
    }

    #[test]
    fn links_location_offset_by_frontmatter() {
        // Frontmatter is lines 1-3; "[[target]]" is on line 4 and "[text](url)" on line 5.
        let content = "---\ntitle: T\n---\n[[target]]\n[text](url)";
        let note = Note::parse("/vault/note.md", content);
        assert_eq!(note.links.len(), 2);
        assert_eq!(note.links[0].location.line, 4);
        assert_eq!(note.links[0].location.col_start, 0);
        assert_eq!(note.links[0].location.col_end, 10);
        assert_eq!(note.links[1].location.line, 5);
        assert_eq!(note.links[1].location.col_start, 0);
        assert_eq!(note.links[1].location.col_end, 11);
    }
}
