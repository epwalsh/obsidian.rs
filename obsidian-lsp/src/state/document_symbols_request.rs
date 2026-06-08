use super::*;

#[derive(Debug)]
pub struct DocumentSymbolsRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
}

impl DocumentSymbolsRequest {
    pub fn compute(self) -> Result<DocumentSymbolResponse, StateError> {
        let note = self.snapshot.note_for_path(&self.path)?;
        let text = self.snapshot.text_for_path(&self.path)?;
        Ok(DocumentSymbolResponse::Nested(document_symbols_for_note(&note, &text)))
    }
}

pub(in crate::state) fn document_symbols_for_note(note: &Note, text: &str) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();

    for key_range in frontmatter_key_ranges(text) {
        symbols.push(document_symbol(
            key_range.key,
            Some("frontmatter".to_string()),
            SymbolKind::KEY,
            key_range.range,
        ));
    }

    for alias in frontmatter_alias_ranges(text) {
        symbols.push(document_symbol(
            alias.value,
            Some("alias".to_string()),
            SymbolKind::STRING,
            alias.range,
        ));
    }

    for tag in symbol_tag_ranges(note, text) {
        symbols.push(document_symbol(
            format!("#{}", tag.value),
            Some("tag".to_string()),
            SymbolKind::ENUM_MEMBER,
            tag.range,
        ));
    }

    for heading in parse_heading_symbols(text) {
        symbols.push(document_symbol(
            heading.name,
            Some("heading".to_string()),
            SymbolKind::STRING,
            heading.range,
        ));
    }

    for link in &note.links {
        symbols.push(document_symbol(
            symbol_link_name(&link.link),
            Some("outbound link".to_string()),
            SymbolKind::FILE,
            location_to_range(&link.location),
        ));
    }

    symbols.sort_by(symbol_range_order);
    symbols
}

pub(in crate::state) fn symbol_tag_ranges(note: &Note, text: &str) -> Vec<FrontmatterValueRange> {
    let mut ranges = Vec::new();

    let frontmatter_tags = note
        .tags
        .iter()
        .filter_map(|tag| match tag.location {
            CoreLocation::Frontmatter => Some(tag.tag.as_str()),
            CoreLocation::Inline(_) => None,
        })
        .collect::<Vec<_>>();
    let mut used_frontmatter_ranges = Vec::new();
    for tag_range in frontmatter_tag_ranges(text) {
        if !frontmatter_tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(&tag_range.tag))
            || used_frontmatter_ranges.contains(&tag_range.range)
        {
            continue;
        }
        used_frontmatter_ranges.push(tag_range.range);
        ranges.push(FrontmatterValueRange {
            value: tag_range.tag,
            range: tag_range.range,
        });
    }

    for tag in &note.tags {
        if let CoreLocation::Inline(location) = &tag.location {
            ranges.push(FrontmatterValueRange {
                value: tag.tag.clone(),
                range: location_to_range(location),
            });
        }
    }

    ranges.sort_by(|left, right| {
        left.range
            .start
            .line
            .cmp(&right.range.start.line)
            .then(left.range.start.character.cmp(&right.range.start.character))
    });
    ranges
}

pub(in crate::state) fn symbol_link_name(link: &Link) -> String {
    match link {
        Link::Wiki { target, heading, alias } => {
            let mut text = format!("[[{target}");
            if let Some(heading) = heading {
                text.push('#');
                text.push_str(heading);
            }
            if let Some(alias) = alias {
                text.push('|');
                text.push_str(alias);
            }
            text.push_str("]]");
            text
        }
        Link::Markdown { text, url } => format!("[{text}]({url})"),
        Link::Embed { target, heading, alias } => {
            let mut text = format!("![[{target}");
            if let Some(heading) = heading {
                text.push('#');
                text.push_str(heading);
            }
            if let Some(alias) = alias {
                text.push('|');
                text.push_str(alias);
            }
            text.push_str("]]");
            text
        }
    }
}

pub(in crate::state) fn symbol_range_order(left: &DocumentSymbol, right: &DocumentSymbol) -> std::cmp::Ordering {
    left.range
        .start
        .line
        .cmp(&right.range.start.line)
        .then(left.range.start.character.cmp(&right.range.start.character))
        .then(left.name.cmp(&right.name))
}

#[allow(deprecated)]
pub(in crate::state) fn document_symbol(
    name: String,
    detail: Option<String>,
    kind: SymbolKind,
    range: Range,
) -> DocumentSymbol {
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    }
}
