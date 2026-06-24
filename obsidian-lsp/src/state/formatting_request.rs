use std::collections::BTreeMap;

use super::*;

#[derive(Debug)]
pub struct FormattingRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
}

struct FrontmatterBlock<'a> {
    raw: &'a str,
    remainder: &'a str,
    closing_has_newline: bool,
}

const FRONTMATTER_PRIORITY_KEYS: &[&str] = &["id", "title", "aliases", "tags"];

impl FormattingRequest {
    pub fn compute(self) -> Result<Vec<TextEdit>, StateError> {
        let text = self.snapshot.text_for_path(&self.path)?;
        let normalized = normalize_document_text(&text);
        if normalized == text {
            return Ok(Vec::new());
        }

        Ok(vec![TextEdit {
            range: Range::new(Position::new(0, 0), document_end_range(&text).end),
            new_text: normalized,
        }])
    }
}

pub(in crate::state) fn normalize_document_text(text: &str) -> String {
    let trimmed = trim_trailing_whitespace(text);
    normalize_frontmatter(&trimmed)
}

fn normalize_frontmatter(text: &str) -> String {
    let Some(block) = split_frontmatter(text) else {
        return text.to_string();
    };
    let Some(mapping) = parse_frontmatter_mapping(block.raw) else {
        return text.to_string();
    };
    let Some(frontmatter) = serialize_frontmatter_mapping(&mapping) else {
        return text.to_string();
    };

    if block.closing_has_newline || !block.remainder.is_empty() {
        format!("---\n{}---\n{}", frontmatter, block.remainder)
    } else {
        format!("---\n{}---", frontmatter)
    }
}

pub(in crate::state) fn trim_trailing_whitespace(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut offset = 0;
    while offset < text.len() {
        let (line, next_offset) = next_line(text, offset);
        let (line_content, line_ending) = split_line_ending(line);
        normalized.push_str(line_content.trim_end_matches(is_trimmable_trailing_whitespace));
        normalized.push_str(line_ending);
        offset = next_offset;
    }

    normalized
}

fn split_frontmatter(text: &str) -> Option<FrontmatterBlock<'_>> {
    let (first_line, mut offset) = next_line(text, 0);
    if split_line_ending(first_line).0 != "---" {
        return None;
    }

    let frontmatter_start = offset;
    while offset < text.len() {
        let line_start = offset;
        let (line, next_offset) = next_line(text, offset);
        let (line_content, line_ending) = split_line_ending(line);
        if line_content == "---" || line_content == "..." {
            return Some(FrontmatterBlock {
                raw: &text[frontmatter_start..line_start],
                remainder: &text[next_offset..],
                closing_has_newline: !line_ending.is_empty(),
            });
        }
        offset = next_offset;
    }

    None
}

fn parse_frontmatter_mapping(text: &str) -> Option<serde_yaml::Mapping> {
    if text.trim().is_empty() {
        return Some(serde_yaml::Mapping::new());
    }

    match serde_yaml::from_str::<serde_yaml::Value>(text).ok()? {
        serde_yaml::Value::Mapping(mapping) => Some(mapping),
        _ => None,
    }
}

fn serialize_frontmatter_mapping(mapping: &serde_yaml::Mapping) -> Option<String> {
    let ordered = order_frontmatter_mapping(mapping)?;
    if ordered.is_empty() {
        return Some(String::new());
    }

    let yaml = serde_yaml::to_string(&ordered).ok()?;
    Some(yaml.strip_prefix("---\n").unwrap_or(&yaml).to_string())
}

fn order_frontmatter_mapping(mapping: &serde_yaml::Mapping) -> Option<serde_yaml::Mapping> {
    let mut entries = BTreeMap::new();
    for (key, value) in mapping {
        let serde_yaml::Value::String(key) = key else {
            return None;
        };
        entries.insert(key.clone(), value.clone());
    }

    let mut ordered = serde_yaml::Mapping::new();
    for key in FRONTMATTER_PRIORITY_KEYS {
        if let Some(value) = entries.remove(*key) {
            ordered.insert(serde_yaml::Value::String((*key).to_string()), value);
        }
    }
    for (key, value) in entries {
        ordered.insert(serde_yaml::Value::String(key), value);
    }

    Some(ordered)
}

fn next_line(text: &str, offset: usize) -> (&str, usize) {
    if let Some(relative) = text[offset..].find('\n') {
        let end = offset + relative + 1;
        (&text[offset..end], end)
    } else {
        (&text[offset..], text.len())
    }
}

fn split_line_ending(line: &str) -> (&str, &str) {
    if let Some(line) = line.strip_suffix("\r\n") {
        (line, "\r\n")
    } else if let Some(line) = line.strip_suffix('\n') {
        (line, "\n")
    } else if let Some(line) = line.strip_suffix('\r') {
        (line, "\r")
    } else {
        (line, "")
    }
}

fn is_trimmable_trailing_whitespace(ch: char) -> bool {
    ch.is_whitespace() && ch != '\n' && ch != '\r'
}
