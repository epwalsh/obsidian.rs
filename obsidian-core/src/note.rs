use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use deunicode::deunicode;
use regex::Regex;

use crate::{LocatedTag, Location, NoteError, common};

use gray_matter::{Matter, Pod, engine::YAML};
use indexmap::IndexMap;

#[derive(Clone)]
pub struct Note {
    pub path: PathBuf,
    pub id: String,
    pub title: Option<String>,
    pub aliases: Vec<String>,
    /// All tags: frontmatter tags have `location: Location::Frontmatter`; inline tags have
    /// `location: Location::Inline(...)`. Always populated.
    pub tags: Vec<LocatedTag>,
    /// The entire, exact contents of the note *including* frontmatter. This is the single source
    /// of truth for a note's contents; all other content-derived fields (`id`, `title`, `aliases`,
    /// `tags`, `links`, `frontmatter`, `frontmatter_line_count`) are parsed from it. It is private
    /// so callers cannot mutate it out of sync with the derived fields; use [`Note::text`] to read
    /// it and [`Note::body`] for the frontmatter-stripped body.
    text: String,
    /// Links extracted from the body at load time (always populated).
    pub links: Vec<crate::LocatedLink>,
    pub frontmatter: Option<IndexMap<String, Pod>>,
    /// Number of lines occupied by the frontmatter block (including delimiters).
    /// Used to offset link locations so they reflect positions in the original file.
    pub frontmatter_line_count: usize,
}

#[derive(Clone)]
pub struct NoteBuilder {
    pub path: PathBuf,
    pub id: String,
    pub title: Option<String>,
    pub aliases: Vec<String>,
    pub tags: Vec<LocatedTag>,
    pub body: Option<String>,
}

const FALLBACK_NOTE_ID: &str = "note";

pub fn normalize_note_id(candidate: &str) -> String {
    let transliterated = deunicode(candidate);
    let mut normalized = String::new();
    let mut last_was_separator = false;

    for ch in transliterated.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator && !normalized.is_empty() {
            normalized.push('-');
            last_was_separator = true;
        }
    }

    while normalized.ends_with('-') {
        normalized.pop();
    }

    if normalized.is_empty() {
        FALLBACK_NOTE_ID.to_string()
    } else {
        normalized
    }
}

pub fn default_note_id_for_path(path: impl AsRef<Path>) -> Result<String, NoteError> {
    let path = path.as_ref();
    let stem = path
        .file_stem()
        .ok_or(NoteError::InvalidPath(path.to_path_buf()))?
        .to_string_lossy();
    Ok(normalize_note_id(&stem))
}

impl NoteBuilder {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, NoteError> {
        let path = path.as_ref();
        Ok(Self {
            path: path.to_path_buf(),
            id: default_note_id_for_path(path)?,
            title: None,
            aliases: Vec::new(),
            tags: Vec::new(),
            body: None,
        })
    }

    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn title(mut self, title: &str) -> Self {
        self.title = Some(title.to_string());
        self
    }

    pub fn alias(mut self, alias: &str) -> Self {
        self.aliases.push(alias.to_string());
        self
    }

    pub fn aliases(mut self, aliases: &[String]) -> Self {
        for alias in aliases {
            self = self.alias(alias);
        }
        self
    }

    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(LocatedTag {
            tag: tag.to_string(),
            location: Location::Frontmatter,
        });
        self
    }

    pub fn tags(mut self, tags: &[&str]) -> Self {
        for tag in tags {
            self = self.tag(tag);
        }
        self
    }

    pub fn located_tag(mut self, tag: &LocatedTag) -> Self {
        self.tags.push(tag.clone());
        self
    }

    pub fn located_tags(mut self, tags: &[LocatedTag]) -> Self {
        for tag in tags {
            self = self.located_tag(tag);
        }
        self
    }

    pub fn body(mut self, body: &str) -> Self {
        self.body = Some(body.to_string());
        self
    }

    pub fn build(self) -> Result<Note, NoteError> {
        let Self {
            path,
            id,
            title,
            aliases,
            tags,
            body,
        } = self;

        let mut note = Note {
            path,
            id,
            title,
            aliases,
            tags,
            text: String::new(),
            links: Vec::new(),
            frontmatter: None,
            frontmatter_line_count: 0,
        };
        // Always build `text` (defaulting to an empty body) so the note is internally consistent.
        note.update_content(Some(body.as_deref().unwrap_or_default()), None)?;
        Ok(note)
    }
}

impl Note {
    pub fn builder(path: impl AsRef<Path>) -> Result<NoteBuilder, NoteError> {
        NoteBuilder::new(path)
    }

    /// Parses a note from a raw file string.
    ///
    /// Useful for constructing notes from in-memory strings (e.g. in tests). For file-backed
    /// notes prefer [`Note::from_path`].
    pub fn parse(path: impl AsRef<Path>, content: &str) -> Self {
        Self::parse_impl(path, content)
    }

    /// Loads a note from disk, retaining the full contents in [`Note::text`].
    ///
    /// Links and inline tags are extracted and stored. Note the entire file is read and retained
    /// in memory.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, NoteError> {
        let path = common::normalize_path(path.as_ref(), None);
        let raw = std::fs::read_to_string(&path)?;
        Ok(Self::parse_impl(&path, &raw))
    }

    fn parse_impl(path: impl AsRef<Path>, content: &str) -> Self {
        let matter = Matter::<YAML>::new();
        let frontmatter = matter.parse(content).ok().and_then(|parsed| {
            parsed.data.and_then(|pod: Pod| pod.as_hashmap().ok()).map(|hm| {
                let mut entries: Vec<_> = hm.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                entries.into_iter().collect::<IndexMap<_, _>>()
            })
        });
        // Locate the body directly in the raw text so that a note's `text` and derived `body()`
        // stay exactly consistent. Deriving the split from gray_matter's parsed content is unsafe
        // because it trims trailing whitespace, which would inflate the line count and slice into
        // the body.
        let body_start = frontmatter_prefix_len(content);
        let body = content[body_start..].to_string();
        let frontmatter_line_count = content[..body_start].bytes().filter(|&b| b == b'\n').count();
        let id = frontmatter
            .as_ref()
            .and_then(|fm| fm.get("id"))
            .and_then(|p| p.as_string().ok())
            .or_else(|| default_note_id_for_path(path.as_ref()).ok())
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
        let fm_tags: Vec<LocatedTag> = frontmatter
            .as_ref()
            .and_then(|fm| fm.get("tags"))
            .and_then(|p| p.as_vec().ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| p.as_string().ok())
            .map(|tag| LocatedTag {
                tag,
                location: Location::Frontmatter,
            })
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
                if let Location::Inline(ref mut loc) = lt.location {
                    loc.line += offset;
                }
                lt
            })
            .collect::<Vec<_>>();
        let mut tags = fm_tags;
        tags.extend(inline_tags);

        Note {
            path: path.as_ref().to_path_buf(),
            id,
            title,
            aliases,
            tags,
            text: content.to_string(),
            links,
            frontmatter,
            frontmatter_line_count,
        }
    }

    /// The entire, exact contents of the note including frontmatter.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The note's body — its contents with the frontmatter block stripped.
    ///
    /// Returned as a zero-copy slice of [`text`](Self::text).
    pub fn body(&self) -> &str {
        let body_start: usize = self
            .text
            .split_inclusive('\n')
            .take(self.frontmatter_line_count)
            .map(str::len)
            .sum();
        &self.text[body_start..]
    }

    pub fn update_content(
        &mut self,
        body: Option<&str>,
        frontmatter: Option<IndexMap<String, Pod>>,
    ) -> Result<(), NoteError> {
        if body.is_none() && frontmatter.is_none() {
            return Ok(());
        }

        if let Some(frontmatter) = frontmatter {
            self.frontmatter = Some(frontmatter);
        }
        let body = match body {
            Some(body) => body.to_string(),
            None => self.body().to_string(),
        };
        self.rebuild_text(&body)
    }

    /// Reserializes `text` from the note's current derived state (frontmatter fields plus `body`)
    /// and reparses so every derived field stays in sync with `text`.
    fn rebuild_text(&mut self, body: &str) -> Result<(), NoteError> {
        let file_content = self.to_file_content(body)?;
        *self = Self::parse_impl(&self.path, &file_content);
        Ok(())
    }

    /// Reloads the note from its path.
    pub fn reload(self) -> Result<Self, NoteError> {
        Self::from_path(&self.path)
    }

    /// Add an alias, keeping [`text`](Self::text) in sync.
    pub fn add_alias(&mut self, alias: String) -> Result<(), NoteError> {
        if !self.aliases.contains(&alias) {
            self.aliases.push(alias);
            let body = self.body().to_string();
            self.rebuild_text(&body)?;
        }
        Ok(())
    }

    /// Add a frontmatter tag, keeping [`text`](Self::text) in sync.
    pub fn add_tag(&mut self, tag: impl Into<String>) -> Result<(), NoteError> {
        let tag = crate::tag::clean_tag(&tag.into());
        let already_present = self
            .tags
            .iter()
            .any(|t| t.tag.eq_ignore_ascii_case(&tag) && matches!(t.location, Location::Frontmatter));
        if !already_present {
            self.tags.push(LocatedTag {
                tag,
                location: Location::Frontmatter,
            });
            let body = self.body().to_string();
            self.rebuild_text(&body)?;
        }
        Ok(())
    }

    /// Remove a frontmatter tag, keeping [`text`](Self::text) in sync.
    pub fn remove_tag(&mut self, tag: &str) -> Result<(), NoteError> {
        let tag = crate::tag::clean_tag(tag);
        let before = self.tags.len();
        self.tags
            .retain(|t| !(t.tag.eq_ignore_ascii_case(&tag) && matches!(t.location, Location::Frontmatter)));
        if self.tags.len() != before {
            let body = self.body().to_string();
            self.rebuild_text(&body)?;
        }
        Ok(())
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

        let body = self.body().to_string();
        self.rebuild_text(&body)
    }

    /// Atomically writes the note to `self.path`, including serialized frontmatter.
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

    /// Atomically writes the note to `self.path`, reconstructing the file from the in-memory
    /// [`text`](Self::text). Retained as a distinct entry point for frontmatter-only updates;
    /// with `text` as the source of truth it is equivalent to [`write`](Self::write).
    pub fn write_frontmatter(&self) -> Result<(), NoteError> {
        self.write()
    }

    /// Read the contents of the note as a string, optionally including frontmatter.
    pub fn read(&self, include_frontmatter: bool) -> Result<String, NoteError> {
        if include_frontmatter {
            Ok(self.to_file_content(self.body())?)
        } else {
            Ok(self.body().to_string())
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
            // Preserve an explicitly empty aliases array, otherwise omit the field.
            if !matches!(fm.get("aliases"), Some(Pod::Array(values)) if values.is_empty()) {
                fm.shift_remove("aliases");
            }
        } else {
            fm.insert(
                "aliases".to_string(),
                Pod::Array(self.aliases.iter().cloned().map(Pod::String).collect()),
            );
        }
        let fm_tags: Vec<String> = self
            .tags
            .iter()
            .filter(|t| matches!(t.location, Location::Frontmatter))
            .map(|t| t.tag.clone())
            .collect();
        if fm_tags.is_empty() {
            // Preserve an explicitly empty tags array, otherwise omit the field.
            if !matches!(fm.get("tags"), Some(Pod::Array(values)) if values.is_empty()) {
                fm.shift_remove("tags");
            }
        } else {
            fm.insert(
                "tags".to_string(),
                Pod::Array(fm_tags.into_iter().map(Pod::String).collect()),
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

    /// Get the last modified time of the note's file on disk.
    pub fn last_modified_time(&self) -> std::time::SystemTime {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    }

    /// Get the creation time of the note.
    pub fn creation_time(&self) -> std::time::SystemTime {
        std::fs::metadata(&self.path)
            .and_then(|m| m.created())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
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

/// Returns the byte length of the leading frontmatter region of `content` — the opening `---`
/// delimiter line through the closing `---` delimiter line and any blank separator lines that
/// follow it. Returns `0` when `content` has no frontmatter block. The returned offset marks where
/// the note body begins.
fn frontmatter_prefix_len(content: &str) -> usize {
    let mut lines = content.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return 0;
    };
    if first.trim_end() != "---" {
        return 0;
    }

    let mut offset = first.len();
    let mut close_offset = None;
    for line in lines {
        offset += line.len();
        if line.trim_end() == "---" {
            close_offset = Some(offset);
            break;
        }
    }
    let Some(mut body_start) = close_offset else {
        // No closing delimiter: not a frontmatter block.
        return 0;
    };

    // Consume blank separator lines between the frontmatter and the body.
    for line in content[body_start..].split_inclusive('\n') {
        if line.contains('\n') && line.trim().is_empty() {
            body_start += line.len();
        } else {
            break;
        }
    }
    body_start
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
        assert_eq!(note.body().trim(), "Hello, world!");

        let fm = note.frontmatter.expect("should have frontmatter");
        assert!(fm.contains_key("title"));
        assert!(fm.contains_key("tags"));
    }

    #[test]
    fn parse_without_frontmatter() {
        let input = "Just some plain markdown content.";
        let note = Note::parse("/vault/plain.md", input);

        assert!(note.frontmatter.is_none());
        assert_eq!(note.body(), input);
    }

    #[test]
    fn from_path_reads_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "---\nauthor: Pete\n---\n\nBody text.").unwrap();

        let note = Note::from_path(tmp.path()).expect("should read file");
        assert!(note.body().contains("Body text."));
        let fm = note.frontmatter.expect("should have frontmatter");
        assert!(fm.contains_key("author"));
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
        let fm_tags: Vec<&str> = note
            .tags
            .iter()
            .filter(|t| matches!(t.location, crate::Location::Frontmatter))
            .map(|t| t.tag.as_str())
            .collect();
        assert_eq!(fm_tags, vec!["rust", "obsidian"]);
    }

    #[test]
    fn tags_empty_when_absent() {
        let note = Note::parse("/vault/note.md", "No frontmatter here.");
        assert!(
            !note
                .tags
                .iter()
                .any(|t| matches!(t.location, crate::Location::Frontmatter))
        );
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

        let note = Note::from_path(tmp.path()).unwrap();
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

        let note = Note::from_path(tmp.path()).unwrap();
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

        let note = Note::from_path(tmp.path()).unwrap();
        note.write().unwrap();

        let on_disk = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(
            on_disk,
            format!(
                "---\nid: {}\n---\n\n{}",
                default_note_id_for_path(tmp.path()).unwrap(),
                original
            )
        );
    }

    #[test]
    fn normalize_note_id_transliterates_and_hyphenates() {
        assert_eq!(normalize_note_id("Café Note"), "cafe-note");
        assert_eq!(normalize_note_id("Alpha_beta 123"), "alpha-beta-123");
    }

    #[test]
    fn normalize_note_id_falls_back_when_no_ascii_alphanumerics_remain() {
        assert_eq!(normalize_note_id("!!!"), "note");
        assert_eq!(normalize_note_id("你好"), "ni-hao");
    }

    #[test]
    fn note_builder_normalizes_default_id_from_path() {
        let builder = Note::builder("/vault/Café Note.md").unwrap();
        assert_eq!(builder.id, "cafe-note");
    }

    #[test]
    fn parse_uses_normalized_filename_id_when_frontmatter_id_is_missing() {
        let note = Note::parse("/vault/Café Note.md", "# Cafe\n");
        assert_eq!(note.id, "cafe-note");
    }

    #[test]
    fn write_round_trips_note_with_frontmatter() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let original = "---\ntitle: My Note\n---\n\nBody text.";
        std::fs::write(tmp.path(), original).unwrap();

        let note = Note::from_path(tmp.path()).unwrap();
        note.write().unwrap();

        // Re-parse to verify the on-disk content is valid and retains key fields.
        let reparsed = Note::from_path(tmp.path()).unwrap();
        assert_eq!(reparsed.title.as_deref(), Some("My Note"));
        assert_eq!(reparsed.body().trim(), "Body text.");
    }

    #[test]
    fn write_preserves_explicit_empty_frontmatter_arrays() {
        let note = Note::parse("/vault/note.md", "---\naliases: []\ntags: []\n---\n\nBody text.");

        assert_eq!(
            note.read(true).unwrap(),
            "---\nid: note\naliases: []\ntags: []\n---\n\nBody text."
        );
        assert_eq!(
            note.frontmatter_json().unwrap().get("tags"),
            Some(&serde_json::json!([]))
        );
        assert_eq!(
            note.frontmatter_json().unwrap().get("aliases"),
            Some(&serde_json::json!([]))
        );
    }

    #[test]
    fn write_reflects_frontmatter_mutation() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "---\ntitle: Old Title\n---\n\nContent.").unwrap();

        let mut note = Note::from_path(tmp.path()).unwrap();
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
