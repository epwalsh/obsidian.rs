use super::*;

#[derive(Clone, Debug)]
pub(in crate::state) struct HeadingSymbol {
    pub(in crate::state) name: String,
    pub(in crate::state) range: Range,
}

#[derive(Clone, Debug)]
pub(in crate::state) struct HeadingMatch {
    pub(in crate::state) path: String,
}

pub(in crate::state) fn resolve_heading_fragment_text(text: &str, fragment: &str) -> Option<String> {
    let expected_segments = parse_heading_fragment_segments(fragment);
    if expected_segments.is_empty() {
        return None;
    }

    let mut seen_anchors = HashMap::new();
    let mut current_path = Vec::new();

    for line in text.lines() {
        let Some((level, _, heading_text)) = heading_line_parts(line) else {
            continue;
        };
        let Some(resolved_anchor) = resolve_heading_anchor(heading_text, &mut seen_anchors) else {
            continue;
        };

        current_path.truncate(level.saturating_sub(1));
        current_path.push(HeadingPathSegment {
            text: heading_text,
            normalized_anchor: normalize_heading_anchor(heading_text),
            resolved_anchor,
        });

        if heading_path_matches(&current_path, &expected_segments) {
            return Some(
                current_path[current_path.len() - expected_segments.len()..]
                    .iter()
                    .map(|segment| segment.text)
                    .collect::<Vec<_>>()
                    .join("#"),
            );
        }
    }

    None
}

pub(in crate::state) fn find_heading_at_position(text: &str, position: Position) -> Option<HeadingMatch> {
    let mut seen_anchors = HashMap::new();
    let mut current_path = Vec::new();
    let mut in_frontmatter = false;

    for (line_index, line) in text.lines().enumerate() {
        if line_index == 0 && line == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if line == "---" || line == "..." {
                in_frontmatter = false;
            }
            continue;
        }

        let Some((level, _col_start, heading_text)) = heading_line_parts(line) else {
            continue;
        };
        let Some(resolved_anchor) = resolve_heading_anchor(heading_text, &mut seen_anchors) else {
            continue;
        };

        current_path.truncate(level.saturating_sub(1));
        current_path.push(HeadingPathSegment {
            text: heading_text,
            normalized_anchor: normalize_heading_anchor(heading_text),
            resolved_anchor,
        });

        if position.line == line_index as u32 {
            return Some(HeadingMatch {
                path: current_path
                    .iter()
                    .map(|segment| segment.text)
                    .collect::<Vec<_>>()
                    .join("#"),
            });
        }
    }

    None
}

struct HeadingFragmentSegment<'a> {
    raw: &'a str,
    normalized: String,
}

struct HeadingPathSegment<'a> {
    text: &'a str,
    normalized_anchor: String,
    resolved_anchor: String,
}

pub(in crate::state) fn find_heading_range(text: &str, heading: &str) -> Option<Range> {
    let expected_segments = parse_heading_fragment_segments(heading);
    if expected_segments.is_empty() {
        return None;
    }

    let mut seen_anchors = HashMap::new();
    let mut current_path = Vec::new();

    for (line_index, line) in text.lines().enumerate() {
        let Some((level, col_start, heading_text)) = heading_line_parts(line) else {
            continue;
        };
        let Some(resolved_anchor) = resolve_heading_anchor(heading_text, &mut seen_anchors) else {
            continue;
        };

        current_path.truncate(level.saturating_sub(1));
        current_path.push(HeadingPathSegment {
            text: heading_text,
            normalized_anchor: normalize_heading_anchor(heading_text),
            resolved_anchor,
        });

        if heading_path_matches(&current_path, &expected_segments) {
            return Some(range_for_span(line_index, col_start, heading_text.chars().count()));
        }
    }

    None
}

fn parse_heading_fragment_segments(heading: &str) -> Vec<HeadingFragmentSegment<'_>> {
    heading
        .split('#')
        .filter(|segment| !segment.is_empty())
        .map(|segment| HeadingFragmentSegment {
            raw: segment,
            normalized: normalize_heading_anchor(segment),
        })
        .collect()
}

fn heading_path_matches(path: &[HeadingPathSegment<'_>], expected: &[HeadingFragmentSegment<'_>]) -> bool {
    if expected.len() > path.len() {
        return false;
    }

    path[path.len() - expected.len()..]
        .iter()
        .zip(expected.iter())
        .all(|(candidate, expected_segment)| heading_segment_matches(candidate, expected_segment))
}

fn heading_segment_matches(candidate: &HeadingPathSegment<'_>, expected: &HeadingFragmentSegment<'_>) -> bool {
    candidate.text == expected.raw
        || candidate.normalized_anchor == expected.normalized
        || candidate.resolved_anchor == expected.normalized
}

fn heading_line_parts(line: &str) -> Option<(usize, usize, &str)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }

    let marker_bytes = trimmed
        .char_indices()
        .take_while(|(_, ch)| *ch == '#')
        .last()
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let level = trimmed[..marker_bytes].chars().count();
    let after_markers = &trimmed[marker_bytes..];
    let content = after_markers.trim_start();
    if content.is_empty() {
        return None;
    }

    let heading_text = strip_optional_heading_closing_hashes(content);
    if heading_text.is_empty() {
        return None;
    }

    let leading_bytes = line.len() - trimmed.len();
    let whitespace_bytes = after_markers.len() - content.len();
    let heading_start = leading_bytes + marker_bytes + whitespace_bytes;

    Some((level, line[..heading_start].chars().count(), heading_text))
}

fn strip_optional_heading_closing_hashes(text: &str) -> &str {
    let trimmed = text.trim_end();
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() == trimmed.len() || !without_hashes.chars().last().is_some_and(char::is_whitespace) {
        return trimmed;
    }

    without_hashes.trim_end()
}

fn resolve_heading_anchor(heading_text: &str, seen_anchors: &mut HashMap<String, usize>) -> Option<String> {
    let base_anchor = normalize_heading_anchor(heading_text);
    if base_anchor.is_empty() {
        return None;
    }

    let seen_count = seen_anchors.entry(base_anchor.clone()).or_default();
    let anchor = if *seen_count == 0 {
        base_anchor
    } else {
        format!("{base_anchor}-{seen_count}")
    };
    *seen_count += 1;

    Some(anchor)
}

fn normalize_heading_anchor(text: &str) -> String {
    let mut anchor = String::new();
    let mut last_was_separator = true;

    for ch in text.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '_' {
            anchor.push(ch);
            last_was_separator = false;
        } else if (ch.is_whitespace() || ch == '-') && !last_was_separator && !anchor.is_empty() {
            anchor.push('-');
            last_was_separator = true;
        }
    }

    while anchor.ends_with('-') {
        anchor.pop();
    }

    anchor
}

pub(in crate::state) fn parse_heading_symbols(text: &str) -> Vec<HeadingSymbol> {
    let mut in_frontmatter = false;
    let mut headings = Vec::new();

    for (i, line) in text.lines().enumerate() {
        if i == 0 && line == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if line == "---" || line == "..." {
                in_frontmatter = false;
            }
            continue;
        }
        if let Some((_, col_start, heading_text)) = heading_line_parts(line) {
            headings.push(HeadingSymbol {
                name: heading_text.to_string(),
                range: range_for_span(i, col_start, heading_text.chars().count()),
            });
        }
    }

    headings
}
