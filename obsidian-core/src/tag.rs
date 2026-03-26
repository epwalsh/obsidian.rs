use std::sync::LazyLock;

use regex::Regex;

use crate::link::{FENCED_CODE_RE, INLINE_CODE_RE, byte_to_line_col};
use crate::{InlineLocation, Location};

#[derive(Clone, Debug, PartialEq)]
pub struct LocatedTag {
    pub tag: String,
    pub location: Location,
}

pub(crate) fn clean_tag(tag: &str) -> String {
    // Remove leading '#'
    tag.trim_start_matches('#').to_string()
}

// Tags must start with a letter, then may contain letters, digits, hyphens, underscores,
// or slashes (for nested tags like #project/work).
// The leading `(?:^|\s)` ensures we don't match `#` in URLs like `https://example.com/#anchor`.
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)(#[a-zA-Z][a-zA-Z0-9_\-/]*)").unwrap());

pub(crate) fn parse_inline_tags(content: &str) -> Vec<LocatedTag> {
    let mut sanitized = content.to_string();
    for m in FENCED_CODE_RE.find_iter(content) {
        sanitized.replace_range(m.range(), &" ".repeat(m.len()));
    }
    for m in INLINE_CODE_RE.find_iter(&sanitized.clone()) {
        sanitized.replace_range(m.range(), &" ".repeat(m.len()));
    }

    let mut tags = Vec::new();
    for caps in TAG_RE.captures_iter(&sanitized) {
        let tag_match = caps.get(1).unwrap();
        // tag_match covers the full `#foo`; positions are identical in content and sanitized
        // because sanitization preserves byte lengths.
        let (line, col_start) = byte_to_line_col(content, tag_match.start());
        let col_end = col_start + content[tag_match.start()..tag_match.end()].chars().count();
        // Store the tag name without the leading '#', for consistency with frontmatter tags.
        let tag_name = content[tag_match.start() + 1..tag_match.end()].to_string();
        tags.push(LocatedTag {
            tag: tag_name,
            location: Location::Inline(InlineLocation {
                line,
                col_start,
                col_end,
            }),
        });
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Note;

    #[test]
    fn inline_tag_basic() {
        let tags = parse_inline_tags("Hello #foo world.");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "foo");
    }

    #[test]
    fn inline_tag_at_start() {
        let tags = parse_inline_tags("#foo at start.");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "foo");
    }

    #[test]
    fn inline_tag_multiple() {
        let tags = parse_inline_tags("See #foo and #bar here.");
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].tag, "foo");
        assert_eq!(tags[1].tag, "bar");
    }

    #[test]
    fn inline_tag_nested() {
        let tags = parse_inline_tags("Topic: #project/work.");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "project/work");
    }

    #[test]
    fn inline_tag_number_only_not_matched() {
        let tags = parse_inline_tags("Not a tag: #123.");
        assert!(tags.is_empty());
    }

    #[test]
    fn inline_tag_in_url_not_matched() {
        let tags = parse_inline_tags("See https://example.com/#anchor here.");
        assert!(tags.is_empty());
    }

    #[test]
    fn inline_tag_in_fenced_code_excluded() {
        let content = "Before.\n```\n#hidden\n```\nAfter.";
        let tags = parse_inline_tags(content);
        assert!(tags.is_empty());
    }

    #[test]
    fn inline_tag_in_inline_code_excluded() {
        let content = "Text `#hidden` more.";
        let tags = parse_inline_tags(content);
        assert!(tags.is_empty());
    }

    #[test]
    fn inline_tag_location_first_line() {
        let tags = parse_inline_tags("#foo");
        assert_eq!(tags.len(), 1);
        let Location::Inline(ref loc) = tags[0].location else {
            panic!("expected inline")
        };
        assert_eq!(loc.line, 1);
        assert_eq!(loc.col_start, 0);
        assert_eq!(loc.col_end, 4); // "#foo" is 4 chars, col_end is exclusive
    }

    #[test]
    fn inline_tag_location_with_prefix() {
        let tags = parse_inline_tags("See #foo here.");
        let Location::Inline(ref loc) = tags[0].location else {
            panic!("expected inline")
        };
        assert_eq!(loc.line, 1);
        assert_eq!(loc.col_start, 4); // "See " is 4 chars
        assert_eq!(loc.col_end, 8); // "#foo" adds 4 more
    }

    #[test]
    fn inline_tag_location_second_line() {
        let content = "First line.\n#foo";
        let tags = parse_inline_tags(content);
        assert_eq!(tags.len(), 1);
        let Location::Inline(ref loc) = tags[0].location else {
            panic!("expected inline")
        };
        assert_eq!(loc.line, 2);
        assert_eq!(loc.col_start, 0);
        assert_eq!(loc.col_end, 4);
    }

    #[test]
    fn note_inline_tags_offset_by_frontmatter() {
        // Frontmatter is lines 1-3; body starts on line 4 with "#foo".
        let content = "---\ntitle: T\n---\n#foo";
        let note = Note::parse("/vault/note.md", content);
        let inline_tag = note
            .tags
            .iter()
            .find(|t| matches!(t.location, Location::Inline(_)))
            .expect("inline tag");
        let Location::Inline(ref loc) = inline_tag.location else {
            unreachable!()
        };
        assert_eq!(loc.line, 4);
        assert_eq!(loc.col_start, 0);
        assert_eq!(loc.col_end, 4);
    }
}
