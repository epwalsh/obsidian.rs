use super::*;

#[derive(Clone, Debug)]
pub(in crate::state) struct FrontmatterTagRange {
    pub(in crate::state) tag: String,
    pub(in crate::state) range: Range,
}
#[derive(Clone, Debug)]
pub(in crate::state) struct FrontmatterKeyRange {
    pub(in crate::state) key: String,
    pub(in crate::state) range: Range,
}
#[derive(Clone, Debug)]
pub(in crate::state) struct FrontmatterValueRange {
    pub(in crate::state) value: String,
    pub(in crate::state) range: Range,
}
pub(in crate::state) fn find_frontmatter_key_range(text: &str, key: &str) -> Option<Range> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }

    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" {
            break;
        }
        if let Some(col_start) = line.find(&format!("{key}:")) {
            return Some(range_for_span(line_index, col_start, key.len()));
        }
    }

    None
}

pub(in crate::state) fn find_frontmatter_key_value_range(text: &str, key: &str, expected_value: &str) -> Option<Range> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }

    let key_prefix = format!("{key}:");
    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" || line == "..." {
            break;
        }

        let trimmed = line.trim_start();
        if !trimmed.starts_with(&key_prefix) {
            continue;
        }

        let leading_width = line.len() - trimmed.len();
        let after_key = &trimmed[key_prefix.len()..];
        let value_start = after_key.find(expected_value)?;
        let col_start = leading_width + key_prefix.len() + after_key[..value_start].chars().count();
        return Some(range_for_span(line_index, col_start, expected_value.chars().count()));
    }

    None
}

pub(in crate::state) fn frontmatter_key_ranges(text: &str) -> Vec<FrontmatterKeyRange> {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" || line == "..." {
            break;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with('-') || !is_frontmatter_key_line(trimmed) {
            continue;
        }
        let Some((key, _)) = trimmed.split_once(':') else {
            continue;
        };
        let leading_bytes = line.len() - trimmed.len();
        ranges.push(FrontmatterKeyRange {
            key: key.to_string(),
            range: range_for_span(line_index, line[..leading_bytes].chars().count(), key.chars().count()),
        });
    }

    ranges
}

pub(in crate::state) fn frontmatter_tag_ranges(text: &str) -> Vec<FrontmatterTagRange> {
    frontmatter_sequence_value_ranges(text, "tags", true)
        .into_iter()
        .map(|value| FrontmatterTagRange {
            tag: value.value,
            range: value.range,
        })
        .collect()
}

pub(in crate::state) fn frontmatter_alias_ranges(text: &str) -> Vec<FrontmatterValueRange> {
    frontmatter_sequence_value_ranges(text, "aliases", false)
}

pub(in crate::state) fn frontmatter_sequence_value_ranges(
    text: &str,
    key: &str,
    trim_leading_hash: bool,
) -> Vec<FrontmatterValueRange> {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let mut in_value_block = false;
    let key_prefix = format!("{key}:");

    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" || line == "..." {
            break;
        }

        let trimmed = line.trim_start();
        let leading_bytes = line.len() - trimmed.len();

        if in_value_block {
            if trimmed.is_empty() {
                continue;
            }
            if is_frontmatter_key_line(trimmed) && !trimmed.starts_with('-') {
                in_value_block = false;
            } else if let Some(after_dash) = trimmed.strip_prefix('-') {
                let segment_start = leading_bytes + 1;
                ranges.extend(scan_frontmatter_value_tokens(
                    line,
                    line_index,
                    segment_start,
                    after_dash,
                    trim_leading_hash,
                ));
                continue;
            } else {
                continue;
            }
        }

        let Some(after_key) = trimmed.strip_prefix(&key_prefix) else {
            continue;
        };
        let segment_start = leading_bytes + key_prefix.len();
        let after_key_trimmed = after_key.trim_start();
        if after_key_trimmed.is_empty() {
            in_value_block = true;
        } else if after_key_trimmed.starts_with('[') {
            ranges.extend(scan_frontmatter_value_tokens(
                line,
                line_index,
                segment_start,
                after_key,
                trim_leading_hash,
            ));
        }
    }

    ranges
}

pub(in crate::state) fn scan_frontmatter_value_tokens(
    line: &str,
    line_index: usize,
    segment_start: usize,
    segment: &str,
    trim_leading_hash: bool,
) -> Vec<FrontmatterValueRange> {
    let mut ranges = Vec::new();
    let mut index = 0;

    while index < segment.len() {
        let Some((offset, ch)) = segment[index..]
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace() && !matches!(ch, '[' | ']' | ','))
        else {
            break;
        };
        index += offset;

        let token_start = index;
        let (value_start, value_end, token_end) = if ch == '\'' || ch == '"' {
            let quote = ch;
            let content_start = token_start + quote.len_utf8();
            let mut end = content_start;
            let mut close_end = content_start;
            for (relative, candidate) in segment[content_start..].char_indices() {
                if candidate == quote {
                    end = content_start + relative;
                    close_end = end + quote.len_utf8();
                    break;
                }
            }
            if close_end == content_start {
                break;
            }
            (content_start, end, close_end)
        } else {
            let token_end = segment[token_start..]
                .char_indices()
                .find_map(|(relative, candidate)| matches!(candidate, ',' | ']').then_some(token_start + relative))
                .unwrap_or(segment.len());
            let (value_start, value_end) = trim_span(segment, token_start, token_end);
            (value_start, value_end, token_end)
        };

        if value_start < value_end {
            let value = if trim_leading_hash {
                segment[value_start..value_end].trim_start_matches('#').to_string()
            } else {
                segment[value_start..value_end].to_string()
            };
            if !value.is_empty() {
                let col_start = line[..segment_start + value_start].chars().count();
                ranges.push(FrontmatterValueRange {
                    value,
                    range: range_for_span(line_index, col_start, segment[value_start..value_end].chars().count()),
                });
            }
        }

        index = token_end;
    }

    ranges
}

pub(in crate::state) fn trim_span(text: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end {
        let Some(ch) = text[start..end].chars().next() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        start += ch.len_utf8();
    }
    while start < end {
        let Some(ch) = text[start..end].chars().next_back() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        end -= ch.len_utf8();
    }
    (start, end)
}

pub(in crate::state) fn is_frontmatter_key_line(trimmed_line: &str) -> bool {
    let Some((key, _)) = trimmed_line.split_once(':') else {
        return false;
    };
    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

pub(in crate::state) fn find_alias_range(text: &str, alias: &str) -> Option<Range> {
    find_frontmatter_value_range(text, alias).or_else(|| find_title_or_heading_range(text, alias))
}

pub(in crate::state) fn find_frontmatter_value_range(text: &str, value: &str) -> Option<Range> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }

    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" {
            break;
        }
        if let Some(col_start) = line.find(value) {
            return Some(range_for_span(line_index, col_start, value.chars().count()));
        }
    }

    None
}

pub(in crate::state) fn find_title_or_heading_range(text: &str, value: &str) -> Option<Range> {
    for (line_index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("title:") && !trimmed.starts_with("# ") {
            continue;
        }
        if let Some(col_start) = line.find(value) {
            return Some(range_for_span(line_index, col_start, value.chars().count()));
        }
    }

    None
}
