use std::sync::LazyLock;

use regex::Regex;

pub enum Link {
    Wiki {
        target: String,
        heading: Option<String>,
        alias: Option<String>,
    },
    Markdown {
        text: String,
        url: String,
    },
    Embed {
        target: String,
        heading: Option<String>,
        alias: Option<String>,
    },
}

static FENCED_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```[^\n]*\n.*?```").unwrap());

static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`\n]+`").unwrap());

// Combined link regex. Embed alternative is listed first so ![[...]] is consumed
// before the wiki alternative can match [[...]] within it.
// Groups: (1) full embed, (2) embed target, (3) embed heading, (4) embed alias,
//         (5) full wiki,  (6) wiki target,  (7) wiki heading,  (8) wiki alias,
//         (9) full md,   (10) md text,     (11) md url.
static LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(!\[\[([^\]#|]*?)(?:#([^\]|]*?))?(?:\|([^\]]*?))?\]\])|(\[\[([^\]#|]*?)(?:#([^\]|]*?))?(?:\|([^\]]*?))?\]\])|(\[([^\]]+?)\]\(([^)\n]+?)\))",
    )
    .unwrap()
});

pub(crate) fn parse_links(content: &str) -> Vec<Link> {
    // Replace code block content with spaces to neutralize links inside them
    // while preserving byte positions.
    let mut sanitized = content.to_string();
    for m in FENCED_CODE_RE.find_iter(content) {
        sanitized.replace_range(m.range(), &" ".repeat(m.len()));
    }
    for m in INLINE_CODE_RE.find_iter(&sanitized.clone()) {
        sanitized.replace_range(m.range(), &" ".repeat(m.len()));
    }

    let mut links = Vec::new();
    for caps in LINK_RE.captures_iter(&sanitized) {
        if caps.get(1).is_some() {
            // Embed
            let target = caps.get(2).map_or("", |m| m.as_str()).to_string();
            let heading = caps.get(3).map(|m| m.as_str().to_string());
            let alias = caps.get(4).map(|m| m.as_str().to_string());
            links.push(Link::Embed {
                target,
                heading,
                alias,
            });
        } else if caps.get(5).is_some() {
            // Wiki
            let target = caps.get(6).map_or("", |m| m.as_str()).to_string();
            let heading = caps.get(7).map(|m| m.as_str().to_string());
            let alias = caps.get(8).map(|m| m.as_str().to_string());
            links.push(Link::Wiki {
                target,
                heading,
                alias,
            });
        } else if caps.get(9).is_some() {
            // Markdown
            let text = caps.get(10).map_or("", |m| m.as_str()).to_string();
            let url = caps.get(11).map_or("", |m| m.as_str()).to_string();
            links.push(Link::Markdown { text, url });
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Note;

    fn assert_wiki(link: &Link, target: &str, heading: Option<&str>, alias: Option<&str>) {
        match link {
            Link::Wiki {
                target: t,
                heading: h,
                alias: a,
            } => {
                assert_eq!(t, target);
                assert_eq!(h.as_deref(), heading);
                assert_eq!(a.as_deref(), alias);
            }
            _ => panic!("expected Wiki link"),
        }
    }

    fn assert_md(link: &Link, text: &str, url: &str) {
        match link {
            Link::Markdown { text: t, url: u } => {
                assert_eq!(t, text);
                assert_eq!(u, url);
            }
            _ => panic!("expected Markdown link"),
        }
    }

    fn assert_embed(link: &Link, target: &str, heading: Option<&str>, alias: Option<&str>) {
        match link {
            Link::Embed {
                target: t,
                heading: h,
                alias: a,
            } => {
                assert_eq!(t, target);
                assert_eq!(h.as_deref(), heading);
                assert_eq!(a.as_deref(), alias);
            }
            _ => panic!("expected Embed link"),
        }
    }

    #[test]
    fn wiki_basic() {
        let links = parse_links("See [[target]].");
        assert_eq!(links.len(), 1);
        assert_wiki(&links[0], "target", None, None);
    }

    #[test]
    fn wiki_basic_multi_word() {
        let links = parse_links("See [[some target]].");
        assert_eq!(links.len(), 1);
        assert_wiki(&links[0], "some target", None, None);
    }

    #[test]
    fn wiki_with_heading() {
        let links = parse_links("See [[target#heading]].");
        assert_eq!(links.len(), 1);
        assert_wiki(&links[0], "target", Some("heading"), None);
    }

    #[test]
    fn wiki_with_alias() {
        let links = parse_links("See [[target|alias]].");
        assert_eq!(links.len(), 1);
        assert_wiki(&links[0], "target", None, Some("alias"));
    }

    #[test]
    fn wiki_with_multi_word_alias() {
        let links = parse_links("See [[target|some alias]].");
        assert_eq!(links.len(), 1);
        assert_wiki(&links[0], "target", None, Some("some alias"));
    }

    #[test]
    fn wiki_multi_word_with_alias() {
        let links = parse_links("See [[some target|alias]].");
        assert_eq!(links.len(), 1);
        assert_wiki(&links[0], "some target", None, Some("alias"));
    }

    #[test]
    fn wiki_with_heading_and_alias() {
        let links = parse_links("See [[target#heading|alias]].");
        assert_eq!(links.len(), 1);
        assert_wiki(&links[0], "target", Some("heading"), Some("alias"));
    }

    #[test]
    fn markdown_link() {
        let links = parse_links("See [some text](https://example.com).");
        assert_eq!(links.len(), 1);
        assert_md(&links[0], "some text", "https://example.com");
    }

    #[test]
    fn embed_basic() {
        let links = parse_links("![[image.png]]");
        assert_eq!(links.len(), 1);
        assert_embed(&links[0], "image.png", None, None);
    }

    #[test]
    fn embed_with_heading_and_alias() {
        let links = parse_links("![[note#section|caption]]");
        assert_eq!(links.len(), 1);
        assert_embed(&links[0], "note", Some("section"), Some("caption"));
    }

    #[test]
    fn links_inside_fenced_code_block_excluded() {
        let content = "Before.\n```\n[[hidden]]\n```\nAfter.";
        let links = parse_links(content);
        assert!(links.is_empty(), "expected no links, got {}", links.len());
    }

    #[test]
    fn links_inside_inline_code_excluded() {
        let content = "Text `[[hidden]]` more.";
        let links = parse_links(content);
        assert!(links.is_empty(), "expected no links, got {}", links.len());
    }

    #[test]
    fn mixed_content() {
        let content = "[[wiki]] and [md](url) and ![[embed]]";
        let links = parse_links(content);
        assert_eq!(links.len(), 3);
        assert_wiki(&links[0], "wiki", None, None);
        assert_md(&links[1], "md", "url");
        assert_embed(&links[2], "embed", None, None);
    }

    #[test]
    fn empty_content() {
        let links = parse_links("");
        assert!(links.is_empty());
    }

    #[test]
    fn note_links_delegates() {
        let note = Note::parse("/vault/note.md", "See [[target]] and [text](url).");
        let links = note.links();
        assert_eq!(links.len(), 2);
        assert_wiki(&links[0], "target", None, None);
        assert_md(&links[1], "text", "url");
    }
}
