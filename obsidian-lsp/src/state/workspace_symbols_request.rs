use super::*;

#[derive(Debug)]
pub struct WorkspaceSymbolsRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) query: String,
}

const WORKSPACE_SYMBOL_LIMIT: usize = 500;

impl WorkspaceSymbolsRequest {
    pub fn compute(self) -> Result<Vec<SymbolInformation>, StateError> {
        let notes = self.snapshot.notes();
        let query = self.query.to_lowercase();
        let mut symbols = Vec::new();

        for note in notes {
            workspace_symbols_for_note(&self.snapshot, &note, &query, &mut symbols)?;
            if symbols.len() >= WORKSPACE_SYMBOL_LIMIT {
                symbols.truncate(WORKSPACE_SYMBOL_LIMIT);
                break;
            }
        }

        Ok(symbols)
    }
}

pub(in crate::state) fn workspace_symbols_for_note(
    snapshot: &StateSnapshot,
    note: &Note,
    query: &str,
    symbols: &mut Vec<SymbolInformation>,
) -> Result<(), StateError> {
    let text = snapshot.text_for_path(&note.path)?;
    let uri = snapshot.uri_for_path(&note.path)?;
    let container = Some(relative_display(snapshot.vault_path.as_path(), &note.path));
    let mut seen_names = HashSet::new();

    push_workspace_symbol(
        symbols,
        query,
        workspace_symbol(
            note.id.clone(),
            SymbolKind::FILE,
            uri.clone(),
            find_frontmatter_key_value_range(&text, "id", &note.id).unwrap_or_else(document_start_range),
            container.clone(),
        ),
    );
    seen_names.insert(note.id.to_lowercase());

    if let Some(title) = note.title.as_deref()
        && seen_names.insert(title.to_lowercase())
    {
        push_workspace_symbol(
            symbols,
            query,
            workspace_symbol(
                title.to_string(),
                SymbolKind::STRING,
                uri.clone(),
                find_title_or_heading_range(&text, title).unwrap_or_else(document_start_range),
                container.clone(),
            ),
        );
    }

    for alias in frontmatter_alias_ranges(&text) {
        if !seen_names.insert(alias.value.to_lowercase()) {
            continue;
        }
        push_workspace_symbol(
            symbols,
            query,
            workspace_symbol(
                alias.value,
                SymbolKind::STRING,
                uri.clone(),
                alias.range,
                container.clone(),
            ),
        );
    }

    let mut seen_tags = HashSet::new();
    for tag in symbol_tag_ranges(note, &text) {
        if !seen_tags.insert(tag.value.to_lowercase()) {
            continue;
        }
        push_workspace_symbol(
            symbols,
            query,
            workspace_symbol(
                format!("#{}", tag.value),
                SymbolKind::ENUM_MEMBER,
                uri.clone(),
                tag.range,
                container.clone(),
            ),
        );
    }

    for heading in parse_heading_symbols(&text) {
        push_workspace_symbol(
            symbols,
            query,
            workspace_symbol(
                heading.name,
                SymbolKind::STRING,
                uri.clone(),
                heading.range,
                container.clone(),
            ),
        );
    }

    Ok(())
}

pub(in crate::state) fn push_workspace_symbol(
    symbols: &mut Vec<SymbolInformation>,
    query: &str,
    symbol: SymbolInformation,
) {
    if symbol_matches_query(&symbol.name, query) && symbols.len() < WORKSPACE_SYMBOL_LIMIT {
        symbols.push(symbol);
    }
}

pub(in crate::state) fn symbol_matches_query(name: &str, query: &str) -> bool {
    query.is_empty() || name.to_lowercase().contains(query)
}

#[allow(deprecated)]
pub(in crate::state) fn workspace_symbol(
    name: String,
    kind: SymbolKind,
    uri: Url,
    range: Range,
    container_name: Option<String>,
) -> SymbolInformation {
    SymbolInformation {
        name,
        kind,
        tags: None,
        deprecated: None,
        location: Location { uri, range },
        container_name,
    }
}
