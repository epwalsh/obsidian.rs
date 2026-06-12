use super::*;

#[derive(Debug)]
pub struct CompletionRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) position: Position,
}

pub(in crate::state) enum LinkContext {
    Wiki {
        note_query: String,
        heading_query: Option<String>,
        link_start_byte: usize,
    },
    Markdown {
        query: String,
        link_start_byte: usize,
    },
    Tag {
        query: String,
        tag_start_byte: usize,
    },
}

impl CompletionRequest {
    pub fn compute(self) -> Result<Option<Vec<CompletionItem>>, StateError> {
        let CompletionRequest {
            snapshot,
            path,
            position,
        } = self;
        let text = snapshot.text_for_path(&path)?;

        let line = text.lines().nth(position.line as usize).unwrap_or("");
        let cursor_byte = lsp_character_to_byte_index(line, position.character);
        let cursor_character = byte_index_to_lsp_character(line, cursor_byte);
        let line_prefix = &line[..cursor_byte];
        let text_after_cursor = &line[cursor_byte..];

        let context = match detect_link_context(line_prefix) {
            Some(ctx) => ctx,
            None => return Ok(None),
        };

        if let LinkContext::Tag { query, tag_start_byte } = &context {
            let prefix_range = Range::new(
                Position::new(position.line, byte_index_to_lsp_character(line, *tag_start_byte)),
                Position::new(position.line, cursor_character),
            );
            let all_tags = snapshot.list_tags();
            return Ok(Some(tag_completions(&all_tags, query, prefix_range)));
        }

        let (note_query, heading_query, link_start_byte) = match &context {
            LinkContext::Wiki {
                note_query,
                heading_query,
                link_start_byte,
            } => (note_query.as_str(), heading_query.as_deref(), *link_start_byte),
            LinkContext::Markdown { query, link_start_byte } => (query.as_str(), None, *link_start_byte),
            LinkContext::Tag { .. } => unreachable!(),
        };

        let close_len_bytes = closing_bracket_len(text_after_cursor, &context);
        let prefix_range = Range::new(
            Position::new(position.line, byte_index_to_lsp_character(line, link_start_byte)),
            Position::new(
                position.line,
                byte_index_to_lsp_character(line, cursor_byte + close_len_bytes),
            ),
        );

        let notes = snapshot.notes();

        let items: Vec<CompletionItem> = if let Some(hq) = heading_query {
            if note_query.is_empty() {
                // Anchor-only link [[#heading]]: complete headings within the current document.
                anchor_completions(&text, hq, prefix_range)
            } else {
                notes
                    .iter()
                    .filter(|note| note_matches_query(note, note_query))
                    .flat_map(|note| match snapshot.text_for_path(&note.path) {
                        Ok(note_text) => heading_completions_for_note(note, &note_text, hq, prefix_range),
                        Err(_) => Vec::new(),
                    })
                    .collect()
            }
        } else {
            notes
                .iter()
                .filter(|note| note_matches_query(note, note_query))
                .flat_map(|note| match &context {
                    LinkContext::Wiki { .. } => wiki_completions_for_note(note, prefix_range),
                    LinkContext::Markdown { .. } => {
                        markdown_completions_for_note(note, snapshot.vault_path.as_path(), prefix_range)
                    }
                    LinkContext::Tag { .. } => unreachable!(),
                })
                .collect()
        };

        Ok(Some(items))
    }
}

pub(in crate::state) fn detect_link_context(line_prefix: &str) -> Option<LinkContext> {
    // Check for open wiki link: last [[ with no ]] or | after it.
    if let Some(start) = line_prefix.rfind("[[") {
        let after_open = &line_prefix[start + 2..];
        if !after_open.contains("]]") && !after_open.contains('|') {
            let (note_query, heading_query) = match after_open.find('#') {
                Some(hash) => (&after_open[..hash], Some(after_open[hash + 1..].to_string())),
                None => (after_open, None),
            };
            return Some(LinkContext::Wiki {
                note_query: note_query.to_string(),
                heading_query,
                link_start_byte: start,
            });
        }
    }

    // Check for open markdown-link display text: last [ not part of [[ with no ] after it.
    let bytes = line_prefix.as_bytes();
    let mut i = line_prefix.len();
    while i > 0 {
        i -= 1;
        if bytes[i] != b'[' {
            continue;
        }
        // Skip if this [ is part of [[.
        if i > 0 && bytes[i - 1] == b'[' {
            continue;
        }
        if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            continue;
        }
        let after_open = &line_prefix[i + 1..];
        if !after_open.contains(']') {
            return Some(LinkContext::Markdown {
                query: after_open.to_string(),
                link_start_byte: i,
            });
        }
        break;
    }

    // Check for tag context: `#` at line start or after whitespace, followed by valid tag chars.
    for i in (0..bytes.len()).rev() {
        if bytes[i] != b'#' {
            continue;
        }
        let preceded_ok = i == 0 || bytes[i - 1].is_ascii_whitespace();
        if !preceded_ok {
            continue;
        }
        let after_hash = &line_prefix[i + 1..];
        let first_ok = after_hash.is_empty() || after_hash.as_bytes()[0].is_ascii_alphabetic();
        let rest_ok = after_hash
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/');
        if first_ok && rest_ok {
            return Some(LinkContext::Tag {
                query: after_hash.to_string(),
                tag_start_byte: i,
            });
        }
        break;
    }

    None
}

pub(in crate::state) fn note_matches_query(note: &Note, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query_lower = query.to_lowercase();
    if note.id.to_lowercase().contains(&query_lower) {
        return true;
    }
    if note
        .title
        .as_deref()
        .is_some_and(|title| title.to_lowercase().contains(&query_lower))
    {
        return true;
    }
    note.aliases
        .iter()
        .any(|alias| alias.to_lowercase().contains(&query_lower))
}

pub(in crate::state) fn wiki_completions_for_note(note: &Note, prefix_range: Range) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    let mut push = |label: String, sort_prefix: &str| {
        if seen.insert(label.clone()) {
            items.push(CompletionItem {
                label: label.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                sort_text: Some(format!("{sort_prefix} {label}")),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: prefix_range,
                    new_text: label,
                })),
                ..Default::default()
            });
        }
    };

    push(format!("[[{}]]", note.id), "0");

    if let Some(title) = note.title.as_deref()
        && title != note.id
    {
        push(format!("[[{}]]", title), "1");
    }

    for alias in &note.aliases {
        push(format!("[[{}|{}]]", note.id, alias), "1");
        if alias != &note.id && note.title.as_deref() != Some(alias.as_str()) {
            push(format!("[[{}]]", alias), "1");
        }
    }

    items
}

pub(in crate::state) fn markdown_completions_for_note(
    note: &Note,
    vault_path: &Path,
    prefix_range: Range,
) -> Vec<CompletionItem> {
    let rel_path = note.path.strip_prefix(vault_path).unwrap_or(&note.path);
    let path_str = rel_path.display().to_string();

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    let mut push = |label: String| {
        if seen.insert(label.clone()) {
            items.push(CompletionItem {
                label: label.clone(),
                kind: Some(CompletionItemKind::FILE),
                sort_text: Some(label.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: prefix_range,
                    new_text: label,
                })),
                ..Default::default()
            });
        }
    };

    push(format!("[{}]({})", note.id, path_str));

    if let Some(title) = note.title.as_deref()
        && title != note.id
    {
        push(format!("[{}]({})", title, path_str));
    }

    for alias in &note.aliases {
        push(format!("[{}]({})", alias, path_str));
    }

    items
}

pub(in crate::state) fn anchor_completions(
    text: &str,
    heading_query: &str,
    prefix_range: Range,
) -> Vec<CompletionItem> {
    let headings = parse_headings(text);
    let query_lower = heading_query.to_lowercase();

    headings
        .iter()
        .filter(|h| heading_query.is_empty() || h.to_lowercase().contains(&query_lower))
        .map(|heading| {
            let label = format!("[[#{}]]", heading);
            CompletionItem {
                label: label.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                sort_text: Some(label.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: prefix_range,
                    new_text: label,
                })),
                ..Default::default()
            }
        })
        .collect()
}

pub(in crate::state) fn tag_completions(tags: &[String], query: &str, prefix_range: Range) -> Vec<CompletionItem> {
    let query_lower = query.to_lowercase();
    tags.iter()
        .filter(|tag| query.is_empty() || tag.starts_with(&query_lower))
        .map(|tag| {
            let new_text = format!("#{tag}");
            CompletionItem {
                label: new_text.clone(),
                kind: Some(CompletionItemKind::KEYWORD),
                sort_text: Some(new_text.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: prefix_range,
                    new_text,
                })),
                ..Default::default()
            }
        })
        .collect()
}

pub(in crate::state) fn closing_bracket_len(text_after_cursor: &str, context: &LinkContext) -> usize {
    match context {
        LinkContext::Wiki { .. } => {
            if text_after_cursor.starts_with("]]") {
                2
            } else {
                0
            }
        }
        LinkContext::Markdown { .. } => {
            if !text_after_cursor.starts_with(']') {
                return 0;
            }
            let rest = &text_after_cursor[1..];
            if rest.starts_with('(')
                && let Some(close) = rest.find(')')
            {
                return 1 + 1 + close + 1;
            }
            1
        }
        LinkContext::Tag { .. } => 0,
    }
}

pub(in crate::state) fn parse_headings(text: &str) -> Vec<String> {
    parse_heading_symbols(text)
        .into_iter()
        .map(|heading| heading.name)
        .collect()
}

pub(in crate::state) fn heading_completions_for_note(
    note: &Note,
    text: &str,
    heading_query: &str,
    prefix_range: Range,
) -> Vec<CompletionItem> {
    let headings = parse_headings(text);
    let query_lower = heading_query.to_lowercase();

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    for heading in &headings {
        if !heading_query.is_empty() && !heading.to_lowercase().contains(&query_lower) {
            continue;
        }

        let mut push = |target: &str| {
            let label = format!("[[{}#{}]]", target, heading);
            if seen.insert(label.clone()) {
                items.push(CompletionItem {
                    label: label.clone(),
                    kind: Some(CompletionItemKind::REFERENCE),
                    sort_text: Some(label.clone()),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: prefix_range,
                        new_text: label,
                    })),
                    ..Default::default()
                });
            }
        };

        push(&note.id);
        if let Some(title) = note.title.as_deref()
            && title != note.id
        {
            push(title);
        }
        for alias in &note.aliases {
            if alias != &note.id && note.title.as_deref() != Some(alias.as_str()) {
                push(alias);
            }
        }
    }

    items
}
