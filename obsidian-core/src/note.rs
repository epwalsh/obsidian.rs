use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gray_matter::{Matter, Pod, engine::YAML};

pub struct Note {
    pub path: PathBuf,
    pub id: String,
    pub title: Option<String>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub content: String,
    pub frontmatter: Option<HashMap<String, Pod>>,
    /// Number of lines occupied by the frontmatter block (including delimiters).
    /// Used to offset link locations so they reflect positions in the original file.
    pub frontmatter_line_count: usize,
}

impl Note {
    pub fn parse(path: impl AsRef<Path>, content: &str) -> Self {
        let path = path.as_ref();
        let matter = Matter::<YAML>::new();
        let (body, frontmatter) = match matter.parse(content) {
            Ok(parsed) => {
                let fm = parsed.data.and_then(|pod: Pod| pod.as_hashmap().ok());
                (parsed.content, fm)
            }
            Err(_) => (content.to_string(), None),
        };
        let frontmatter_line_count = content.lines().count().saturating_sub(body.lines().count());
        let id = frontmatter
            .as_ref()
            .and_then(|fm| fm.get("id"))
            .and_then(|p| p.as_string().ok())
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        let title = frontmatter
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
            if let Some(ref t) = title
                && !v.contains(t)
            {
                v.push(t.clone());
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
        Note {
            path: path.to_path_buf(),
            id,
            title,
            aliases,
            tags,
            content: body,
            frontmatter,
            frontmatter_line_count,
        }
    }

    pub fn links(&self) -> Vec<crate::LocatedLink> {
        let offset = self.frontmatter_line_count;
        crate::link::parse_links(&self.content)
            .into_iter()
            .map(|mut ll| {
                ll.location.line += offset;
                ll
            })
            .collect()
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(&path)?;
        Ok(Self::parse(path, &content))
    }
}

fn find_h1(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("# ").map(|t| t.trim_end().to_string()))
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
        assert_eq!(note.content.trim(), "Hello, world!");

        let fm = note.frontmatter.expect("should have frontmatter");
        assert!(fm.contains_key("title"));
        assert!(fm.contains_key("tags"));
    }

    #[test]
    fn parse_without_frontmatter() {
        let input = "Just some plain markdown content.";
        let note = Note::parse("/vault/plain.md", input);

        assert!(note.frontmatter.is_none());
        assert_eq!(note.content, input);
    }

    #[test]
    fn from_path_reads_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "---\nauthor: Pete\n---\n\nBody text.").unwrap();

        let note = Note::from_path(tmp.path()).expect("should read file");
        let fm = note.frontmatter.expect("should have frontmatter");
        assert!(fm.contains_key("author"));
        assert!(note.content.contains("Body text."));
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
    fn links_location_offset_by_frontmatter() {
        // Frontmatter is lines 1-3; "[[target]]" is on line 4 and "[text](url)" on line 5.
        let content = "---\ntitle: T\n---\n[[target]]\n[text](url)";
        let note = Note::parse("/vault/note.md", content);
        let links = note.links();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].location.line, 4);
        assert_eq!(links[0].location.col_start, 0);
        assert_eq!(links[0].location.col_end, 10);
        assert_eq!(links[1].location.line, 5);
        assert_eq!(links[1].location.col_start, 0);
        assert_eq!(links[1].location.col_end, 11);
    }
}
