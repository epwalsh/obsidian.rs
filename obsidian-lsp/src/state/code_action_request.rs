use super::*;

#[derive(Debug)]
pub struct CodeActionRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) range: Range,
    pub(in crate::state) position: Position,
    pub(in crate::state) diagnostics: Vec<Diagnostic>,
}

impl CodeActionRequest {
    pub fn compute(self) -> Result<Option<Vec<CodeAction>>, StateError> {
        let CodeActionRequest {
            snapshot,
            path,
            range,
            position,
            diagnostics,
        } = self;
        let source_note = snapshot.note_for_path(&path)?;
        let notes = snapshot.notes();
        let mut actions = Vec::new();

        actions.extend(duplicate_diagnostic_actions(
            &snapshot,
            &source_note,
            &notes,
            range,
            position,
            &diagnostics,
        )?);

        if let Some(located_link) = find_link_at_position(&source_note, position) {
            let targets = resolve_link_targets(&path, &located_link.link, &notes, snapshot.vault_path.as_path());
            match targets.as_slice() {
                [] => {
                    if let Some(action) =
                        create_note_code_action(&path, located_link, snapshot.vault_path.as_path(), &diagnostics)?
                    {
                        actions.push(action);
                    }
                }
                [target] => {
                    if let Some(action) = convert_link_code_action(&snapshot, &path, located_link, target)? {
                        actions.push(action);
                    }
                    if let Some(action) = add_missing_heading_code_action(&snapshot, located_link, target)? {
                        actions.push(action);
                    }
                }
                _ => {}
            }
        }

        Ok((!actions.is_empty()).then_some(actions))
    }
}

pub(in crate::state) fn duplicate_diagnostic_actions(
    snapshot: &StateSnapshot,
    source_note: &Note,
    notes: &[Note],
    range: Range,
    position: Position,
    diagnostics: &[Diagnostic],
) -> Result<Vec<CodeAction>, StateError> {
    let diagnostics = duplicate_diagnostics_for_request(snapshot, source_note, notes, range, position, diagnostics)?;
    let mut actions = Vec::new();

    for diagnostic in &diagnostics {
        if diagnostic_code_is(diagnostic, "duplicate-id") {
            if let Some(action) = assign_unique_note_id_code_action(snapshot, source_note, notes, diagnostic)? {
                actions.push(action);
            }
        } else if diagnostic_code_is(diagnostic, "duplicate-alias") {
            actions.extend(change_duplicate_alias_code_actions(
                snapshot,
                source_note,
                notes,
                diagnostic,
            )?);
        }
    }

    Ok(actions)
}

pub(in crate::state) fn duplicate_diagnostics_for_request(
    snapshot: &StateSnapshot,
    source_note: &Note,
    notes: &[Note],
    range: Range,
    position: Position,
    diagnostics: &[Diagnostic],
) -> Result<Vec<Diagnostic>, StateError> {
    let mut applicable = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic_code_is(diagnostic, "duplicate-id") || diagnostic_code_is(diagnostic, "duplicate-alias")
        })
        .filter(|diagnostic| diagnostic_applies_to_request(diagnostic, range, position))
        .cloned()
        .collect::<Vec<_>>();

    let ignore_set = build_ignore_set(&snapshot.diagnostics_ignore);
    if diagnostics_path_is_ignored(snapshot, &ignore_set, &source_note.path) {
        return Ok(applicable);
    }

    let visible_notes = notes
        .iter()
        .filter(|note| !diagnostics_path_is_ignored(snapshot, &ignore_set, &note.path))
        .collect::<Vec<_>>();

    if visible_notes
        .iter()
        .filter(|note| note.id == source_note.id)
        .map(|note| &note.path)
        .collect::<HashSet<_>>()
        .len()
        > 1
    {
        let diagnostic_range = duplicate_id_range(snapshot, &source_note.path)?;
        if diagnostic_applies_to_request_range(diagnostic_range, range, position) {
            let other_paths = visible_notes
                .iter()
                .filter(|note| note.path != source_note.path && note.id == source_note.id)
                .map(|note| relative_display(snapshot.vault_path.as_path(), &note.path))
                .collect::<Vec<_>>();
            push_unique_diagnostic(
                &mut applicable,
                make_diagnostic(
                    diagnostic_range,
                    "duplicate-id",
                    format!(
                        "Duplicate note ID `{}` also used by {}.",
                        source_note.id,
                        other_paths.join(", ")
                    ),
                ),
            );
        }
    }

    for (alias, diagnostic_range) in duplicate_alias_ranges_for_note(snapshot, source_note)? {
        if !diagnostic_applies_to_request_range(diagnostic_range, range, position) {
            continue;
        }
        if visible_notes
            .iter()
            .filter(|note| {
                note.aliases
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&alias))
            })
            .map(|note| &note.path)
            .collect::<HashSet<_>>()
            .len()
            <= 1
        {
            continue;
        }

        let other_paths = visible_notes
            .iter()
            .filter(|note| note.path != source_note.path)
            .filter(|note| {
                note.aliases
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&alias))
            })
            .map(|note| relative_display(snapshot.vault_path.as_path(), &note.path))
            .collect::<Vec<_>>();
        push_unique_diagnostic(
            &mut applicable,
            make_diagnostic(
                diagnostic_range,
                "duplicate-alias",
                format!("Duplicate alias `{alias}` also used by {}.", other_paths.join(", ")),
            ),
        );
    }

    Ok(applicable)
}

pub(in crate::state) fn duplicate_alias_ranges_for_note(
    snapshot: &StateSnapshot,
    note: &Note,
) -> Result<Vec<(String, Range)>, StateError> {
    let text = snapshot.text_for_path(&note.path)?;
    let mut ranges: Vec<(String, Range)> = Vec::new();

    for alias_range in frontmatter_alias_ranges(&text) {
        if !note
            .aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(&alias_range.value))
        {
            continue;
        }
        if !ranges
            .iter()
            .any(|(alias, range)| alias.eq_ignore_ascii_case(&alias_range.value) && *range == alias_range.range)
        {
            ranges.push((alias_range.value, alias_range.range));
        }
    }

    if let Some(title) = note.title.as_deref()
        && note.aliases.iter().any(|alias| alias.eq_ignore_ascii_case(title))
        && let Some(range) = find_title_or_heading_range(&text, title)
        && !ranges
            .iter()
            .any(|(alias, existing_range)| alias.eq_ignore_ascii_case(title) && *existing_range == range)
    {
        ranges.push((title.to_string(), range));
    }

    Ok(ranges)
}

pub(in crate::state) fn push_unique_diagnostic(diagnostics: &mut Vec<Diagnostic>, diagnostic: Diagnostic) {
    if diagnostics
        .iter()
        .any(|existing| existing.code == diagnostic.code && existing.range == diagnostic.range)
    {
        return;
    }
    diagnostics.push(diagnostic);
}

pub(in crate::state) fn diagnostics_path_is_ignored(
    snapshot: &StateSnapshot,
    ignore_set: &globset::GlobSet,
    path: &Path,
) -> bool {
    let rel = path.strip_prefix(snapshot.vault_path.as_path()).unwrap_or(path);
    ignore_set.is_match(rel)
}

pub(in crate::state) fn assign_unique_note_id_code_action(
    snapshot: &StateSnapshot,
    note: &Note,
    notes: &[Note],
    diagnostic: &Diagnostic,
) -> Result<Option<CodeAction>, StateError> {
    let new_id = unique_note_id(note, notes);
    if new_id == note.id {
        return Ok(None);
    }

    let edit = assign_note_id_workspace_edit(snapshot, note, &new_id)?;
    Ok(Some(CodeAction {
        title: format!("Assign unique note ID '{new_id}'"),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(edit),
        is_preferred: Some(true),
        ..Default::default()
    }))
}

pub(in crate::state) fn change_duplicate_alias_code_actions(
    snapshot: &StateSnapshot,
    note: &Note,
    notes: &[Note],
    diagnostic: &Diagnostic,
) -> Result<Vec<CodeAction>, StateError> {
    let Some(target) = duplicate_alias_edit_target(snapshot, note, diagnostic)? else {
        return Ok(Vec::new());
    };

    let new_alias = unique_note_alias(&target.alias, notes);
    let mut actions = Vec::new();

    if new_alias != target.alias {
        let mut edits_by_path = HashMap::new();
        edits_by_path.insert(
            note.path.clone(),
            vec![TextEdit {
                range: target.range,
                new_text: new_alias.clone(),
            }],
        );
        actions.push(CodeAction {
            title: format!("Change duplicate alias '{}' to '{new_alias}'", target.alias),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(workspace_edit_from_text_edits(snapshot, edits_by_path)?),
            is_preferred: Some(true),
            ..Default::default()
        });
    }

    if let Some(removal_range) = target.removal_range {
        let mut edits_by_path = HashMap::new();
        edits_by_path.insert(
            note.path.clone(),
            vec![TextEdit {
                range: removal_range,
                new_text: String::new(),
            }],
        );
        actions.push(CodeAction {
            title: format!("Remove duplicate alias '{}'", target.alias),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(workspace_edit_from_text_edits(snapshot, edits_by_path)?),
            ..Default::default()
        });
    }

    Ok(actions)
}

#[derive(Debug)]
pub(in crate::state) struct DuplicateAliasEditTarget {
    alias: String,
    range: Range,
    removal_range: Option<Range>,
}

pub(in crate::state) fn duplicate_alias_edit_target(
    snapshot: &StateSnapshot,
    note: &Note,
    diagnostic: &Diagnostic,
) -> Result<Option<DuplicateAliasEditTarget>, StateError> {
    let text = snapshot.text_for_path(&note.path)?;
    let alias = diagnostic_backtick_value(diagnostic)
        .or_else(|| alias_at_range(note, &text, diagnostic.range))
        .map(|alias| alias.to_lowercase());
    let Some(alias) = alias else {
        return Ok(None);
    };

    for alias_range in frontmatter_alias_ranges(&text) {
        if alias_range.value.to_lowercase() == alias {
            return Ok(Some(DuplicateAliasEditTarget {
                alias: alias_range.value,
                range: alias_range.range,
                removal_range: block_list_item_removal_range(&text, alias_range.range),
            }));
        }
    }

    if let Some(title) = note.title.as_deref()
        && title.to_lowercase() == alias
        && let Some(range) = find_title_or_heading_range(&text, title)
    {
        return Ok(Some(DuplicateAliasEditTarget {
            alias: title.to_string(),
            range,
            removal_range: None,
        }));
    }

    Ok(None)
}

pub(in crate::state) fn alias_at_range(note: &Note, text: &str, range: Range) -> Option<String> {
    for alias_range in frontmatter_alias_ranges(text) {
        if ranges_intersect(alias_range.range, range) {
            return Some(alias_range.value);
        }
    }

    note.title.as_deref().and_then(|title| {
        find_title_or_heading_range(text, title)
            .filter(|title_range| ranges_intersect(*title_range, range))
            .map(|_| title.to_string())
    })
}

pub(in crate::state) fn block_list_item_removal_range(text: &str, value_range: Range) -> Option<Range> {
    let line_index = value_range.start.line as usize;
    let line = text.lines().nth(line_index)?;
    if !line.trim_start().starts_with('-') {
        return None;
    }

    let line_count = text.lines().count();
    let end = if line_index + 1 < line_count || text.ends_with('\n') {
        Position::new(value_range.start.line + 1, 0)
    } else {
        Position::new(value_range.start.line, line.chars().count() as u32)
    };
    Some(Range::new(Position::new(value_range.start.line, 0), end))
}

pub(in crate::state) fn create_note_code_action(
    source_path: &Path,
    located_link: &LocatedLink,
    vault_path: &Path,
    diagnostics: &[Diagnostic],
) -> Result<Option<CodeAction>, StateError> {
    let Some(new_path) = compute_new_note_path(source_path, vault_path, &located_link.link) else {
        return Ok(None);
    };

    if new_path.exists() {
        return Ok(None);
    }

    let stem = new_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("note")
        .to_string();
    let note_title = new_note_title_from_link(&located_link.link);
    let new_note_text = new_note_content(&stem, note_title.as_deref());

    let new_path_str = new_path.to_string_lossy().into_owned();
    let new_uri = path_to_uri(&new_path)?;
    let title = format!("Create note '{stem}'");
    let mut arguments = vec![json!(new_path_str)];
    if let Some(note_title) = note_title {
        arguments.push(json!(note_title));
    }
    let link_range = location_to_range(&located_link.location);
    let action_diagnostics = matching_diagnostics(diagnostics, "broken-link", link_range);

    // `edit` is a TextDocumentEdit (no CreateFile) so preview plugins can show the diff
    // without creating any file on disk. `command` does the actual work when the user applies.
    Ok(Some(CodeAction {
        title: title.clone(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: (!action_diagnostics.is_empty()).then_some(action_diagnostics),
        edit: Some(WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(vec![DocumentChangeOperation::Edit(
                TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: new_uri,
                        version: None,
                    },
                    edits: vec![OneOf::Left(TextEdit {
                        range: document_start_range(),
                        new_text: new_note_text,
                    })],
                },
            )])),
            ..Default::default()
        }),
        command: Some(Command {
            title,
            command: "obsidian.createNote".to_string(),
            arguments: Some(arguments),
        }),
        ..Default::default()
    }))
}

pub(in crate::state) fn convert_link_code_action(
    snapshot: &StateSnapshot,
    source_path: &Path,
    located_link: &LocatedLink,
    target: &Note,
) -> Result<Option<CodeAction>, StateError> {
    let (title, new_text) = match &located_link.link {
        Link::Wiki { .. } => (
            "Convert wiki link to markdown".to_string(),
            markdown_link_text(source_path, &located_link.link, target),
        ),
        Link::Markdown { .. } => (
            "Convert markdown link to wiki".to_string(),
            wiki_link_text(snapshot, &located_link.link, target)?,
        ),
        Link::Embed { .. } => return Ok(None),
    };

    let mut edits_by_path = HashMap::new();
    edits_by_path.insert(
        source_path.to_path_buf(),
        vec![TextEdit {
            range: location_to_range(&located_link.location),
            new_text,
        }],
    );

    Ok(Some(CodeAction {
        title,
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(workspace_edit_from_text_edits(snapshot, edits_by_path)?),
        ..Default::default()
    }))
}

pub(in crate::state) fn add_missing_heading_code_action(
    snapshot: &StateSnapshot,
    located_link: &LocatedLink,
    target: &Note,
) -> Result<Option<CodeAction>, StateError> {
    let Link::Wiki {
        target: wiki_target,
        heading: Some(heading),
        ..
    } = &located_link.link
    else {
        return Ok(None);
    };
    if wiki_target.is_empty() || heading.is_empty() || heading.contains('#') {
        return Ok(None);
    }

    let text = snapshot.text_for_path(&target.path)?;
    if find_heading_range(&text, heading).is_some() {
        return Ok(None);
    }

    let mut edits_by_path = HashMap::new();
    edits_by_path.insert(
        target.path.clone(),
        vec![TextEdit {
            range: document_end_range(&text),
            new_text: append_heading_text(&text, heading),
        }],
    );

    Ok(Some(CodeAction {
        title: format!(
            "Add heading '{}' to {}",
            heading,
            relative_display(snapshot.vault_path.as_path(), &target.path)
        ),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(workspace_edit_from_text_edits(snapshot, edits_by_path)?),
        ..Default::default()
    }))
}

pub(in crate::state) fn new_note_title_from_link(link: &Link) -> Option<String> {
    match link {
        Link::Wiki { alias, .. } => alias.as_deref(),
        Link::Markdown { text, .. } => Some(text.as_str()),
        Link::Embed { .. } => None,
    }
    .map(str::trim)
    .filter(|title| !title.is_empty())
    .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub(crate) fn new_note_content(id: &str, title: Option<&str>) -> String {
    let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) else {
        return format!("---\nid: {id}\n---\n");
    };
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");

    format!(
        "---\nid: {id}\naliases:\n- {}\n---\n\n# {}\n",
        yaml_scalar(&title),
        title
    )
}

pub(in crate::state) fn yaml_scalar(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let starts_with_yaml_indicator = value.chars().next().is_some_and(|c| {
        matches!(
            c,
            '-' | '?' | ':' | '!' | '&' | '*' | '#' | '[' | ']' | '{' | '}' | ',' | '|' | '>' | '@' | '`' | '"' | '\''
        )
    });
    if !value.is_empty()
        && !matches!(lower.as_str(), "null" | "true" | "false" | "~")
        && !starts_with_yaml_indicator
        && !value.ends_with(':')
        && !value.contains(": ")
        && !value.contains(" #")
        && !value.chars().any(|c| matches!(c, '\n' | '\r' | '\t'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

pub(in crate::state) fn assign_note_id_workspace_edit(
    snapshot: &StateSnapshot,
    note: &Note,
    new_id: &str,
) -> Result<WorkspaceEdit, StateError> {
    let text = snapshot.text_for_path(&note.path)?;
    let edit = note_id_text_edit(&text, note, new_id);
    let mut edits_by_path = HashMap::new();
    edits_by_path.insert(note.path.clone(), vec![edit]);
    workspace_edit_from_text_edits(snapshot, edits_by_path)
}

pub(in crate::state) fn note_id_text_edit(text: &str, note: &Note, new_id: &str) -> TextEdit {
    if let Some(range) = find_frontmatter_key_value_range(text, "id", &note.id) {
        return TextEdit {
            range,
            new_text: new_id.to_string(),
        };
    }

    if text.lines().next() == Some("---") {
        return TextEdit {
            range: Range::new(Position::new(1, 0), Position::new(1, 0)),
            new_text: format!("id: {}\n", yaml_scalar(new_id)),
        };
    }

    TextEdit {
        range: document_start_range(),
        new_text: format!("---\nid: {}\n---\n\n", yaml_scalar(new_id)),
    }
}

pub(in crate::state) fn unique_note_id(note: &Note, notes: &[Note]) -> String {
    let used = notes.iter().map(|note| note.id.clone()).collect::<HashSet<_>>();
    unique_suffixed_name(&note_file_stem(note), |candidate| used.contains(candidate))
}

pub(in crate::state) fn unique_note_alias(alias: &str, notes: &[Note]) -> String {
    let used = notes
        .iter()
        .flat_map(|note| note.aliases.iter())
        .map(|alias| alias.to_lowercase())
        .collect::<HashSet<_>>();
    unique_suffixed_name(alias, |candidate| used.contains(&candidate.to_lowercase()))
}

pub(in crate::state) fn unique_suffixed_name(base: &str, is_used: impl Fn(&str) -> bool) -> String {
    let base = if base.trim().is_empty() { "note" } else { base.trim() };
    let mut suffix = 2;
    let mut candidate = base.to_string();
    while is_used(&candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    candidate
}

pub(in crate::state) fn append_heading_text(text: &str, heading: &str) -> String {
    let prefix = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{prefix}## {heading}\n")
}

pub(in crate::state) fn compute_new_note_path(source_path: &Path, vault_path: &Path, link: &Link) -> Option<PathBuf> {
    match link {
        Link::Wiki { target, .. } => {
            if target.is_empty() {
                return None;
            }
            let source_dir = source_path.parent().unwrap_or(source_path);
            normalize_new_note_path(vault_path, source_dir.join(format!("{target}.md")))
        }
        Link::Markdown { url, .. } => {
            let url_path = markdown_url_path(url)?;
            if !url_path.ends_with(".md") {
                return None;
            }
            normalize_new_note_path(vault_path, vault_path.join(&url_path))
        }
        Link::Embed { .. } => None,
    }
}
