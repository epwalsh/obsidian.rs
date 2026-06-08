use super::*;

#[derive(Debug)]
pub struct RenameRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) position: Position,
    pub(in crate::state) new_name: String,
}

pub(in crate::state) struct RenameTarget {
    pub(in crate::state) note: Note,
    pub(in crate::state) range: Range,
    pub(in crate::state) placeholder: String,
}

impl RenameRequest {
    pub fn compute(self) -> Result<Option<WorkspaceEdit>, StateError> {
        let source_note = self.snapshot.note_for_path(&self.path)?;
        if find_link_at_position(&source_note, self.position).is_none()
            && let Some(selected_tag) = find_tag_at_position(&self.snapshot, &source_note, self.position)?
        {
            return Ok(Some(tag_rename_workspace_edit(
                &self.snapshot,
                &selected_tag.tag,
                &self.new_name,
            )?));
        }

        let Some(target) = rename_target(&self.snapshot, &self.path, self.position)? else {
            return Ok(None);
        };

        Ok(Some(rename_workspace_edit(
            &self.snapshot,
            &target.note,
            &self.new_name,
        )?))
    }
}

pub(in crate::state) fn rename_target(
    snapshot: &StateSnapshot,
    path: &Path,
    position: Position,
) -> Result<Option<RenameTarget>, StateError> {
    let source_note = snapshot.note_for_path(path)?;

    if let Some(selected_link) = find_link_at_position(&source_note, position) {
        let notes = snapshot.notes();
        let matching_notes = resolve_link_targets(
            &source_note.path,
            &selected_link.link,
            &notes,
            snapshot.vault_path.as_path(),
        );
        if matching_notes.len() != 1 {
            return Ok(None);
        }

        let note = matching_notes[0].clone();
        if !note.path.exists() {
            return Ok(None);
        }
        let placeholder = note_file_stem(&note);
        let range =
            rename_link_target_range(selected_link).unwrap_or_else(|| location_to_range(&selected_link.location));
        return Ok(Some(RenameTarget {
            note,
            range,
            placeholder,
        }));
    }

    if !source_note.path.exists() {
        return Ok(None);
    }
    let placeholder = note_file_stem(&source_note);
    Ok(Some(RenameTarget {
        note: source_note,
        range: Range::new(position, position),
        placeholder,
    }))
}

pub(in crate::state) fn rename_workspace_edit(
    snapshot: &StateSnapshot,
    note: &Note,
    new_name: &str,
) -> Result<WorkspaceEdit, StateError> {
    let new_path = rename_target_path(snapshot.vault_path.as_path(), &note.path, new_name)?;
    if new_path == note.path {
        return Ok(WorkspaceEdit::default());
    }

    let vault = snapshot.build_vault()?;
    let rename_edits = vault.rename_edits(note, &new_path)?;
    let mut edits_by_path: HashMap<PathBuf, Vec<TextEdit>> = HashMap::new();

    if rename_edits.id_will_update {
        let text = snapshot.text_for_path(&note.path)?;
        if let Some(range) = find_frontmatter_key_value_range(&text, "id", &note.id) {
            edits_by_path.entry(note.path.clone()).or_default().push(TextEdit {
                range,
                new_text: rename_edits.new_stem.clone(),
            });
        }
    }

    for (path, replacements) in &rename_edits.backlink_edits {
        let edits = edits_by_path.entry(path.clone()).or_default();
        edits.extend(replacements.iter().map(|(link, new_text)| TextEdit {
            range: location_to_range(&link.location),
            new_text: new_text.clone(),
        }));
    }

    let mut operations = Vec::new();
    let mut paths = edits_by_path.keys().cloned().collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let mut edits = edits_by_path.remove(&path).unwrap_or_default();
        edits.sort_by(|left, right| {
            left.range
                .start
                .line
                .cmp(&right.range.start.line)
                .then(left.range.start.character.cmp(&right.range.start.character))
        });
        operations.push(DocumentChangeOperation::Edit(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: snapshot.uri_for_path(&path)?,
                version: if path == note.path {
                    None
                } else {
                    snapshot.version_for_path(&path)
                },
            },
            edits: edits.into_iter().map(OneOf::Left).collect(),
        }));
    }

    operations.push(DocumentChangeOperation::Op(ResourceOp::Rename(RenameFile {
        old_uri: snapshot.uri_for_path(&note.path)?,
        new_uri: path_to_uri(&rename_edits.new_path)?,
        options: Some(RenameFileOptions {
            overwrite: Some(false),
            ignore_if_exists: Some(false),
        }),
        annotation_id: None,
    })));

    Ok(WorkspaceEdit {
        document_changes: Some(DocumentChanges::Operations(operations)),
        ..Default::default()
    })
}

pub(in crate::state) fn tag_rename_workspace_edit(
    snapshot: &StateSnapshot,
    old_tag: &str,
    new_name: &str,
) -> Result<WorkspaceEdit, StateError> {
    let new_tag = normalize_tag_rename_target(new_name)?;
    if old_tag.eq_ignore_ascii_case(&new_tag) {
        return Ok(WorkspaceEdit::default());
    }

    let mut edits_by_path: HashMap<PathBuf, Vec<TextEdit>> = HashMap::new();
    for occurrence in tag_occurrences(snapshot, old_tag)? {
        edits_by_path.entry(occurrence.path).or_default().push(TextEdit {
            range: occurrence.range,
            new_text: if occurrence.inline {
                format!("#{new_tag}")
            } else {
                new_tag.clone()
            },
        });
    }

    workspace_edit_from_text_edits(snapshot, edits_by_path)
}

pub(in crate::state) fn normalize_tag_rename_target(new_name: &str) -> Result<String, StateError> {
    let tag = new_name.trim().trim_start_matches('#');
    if tag.is_empty() {
        return Err(StateError::InvalidTagRenameTarget(new_name.to_string()));
    }

    let mut chars = tag.chars();
    if !chars.next().is_some_and(|ch| ch.is_ascii_alphabetic()) {
        return Err(StateError::InvalidTagRenameTarget(new_name.to_string()));
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '/') {
        return Err(StateError::InvalidTagRenameTarget(new_name.to_string()));
    }

    Ok(tag.to_string())
}

pub(in crate::state) fn rename_target_path(
    vault_path: &Path,
    note_path: &Path,
    new_name: &str,
) -> Result<PathBuf, StateError> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(StateError::InvalidRenameTarget {
            path: note_path.to_path_buf(),
            new_name: new_name.to_string(),
        });
    }

    let raw = PathBuf::from(new_name);
    let raw = match raw.extension().and_then(|ext| ext.to_str()) {
        Some("md") => raw,
        Some(_) => {
            return Err(StateError::InvalidRenameTarget {
                path: note_path.to_path_buf(),
                new_name: new_name.to_string(),
            });
        }
        None => raw.with_extension("md"),
    };
    let has_parent_component = raw.components().count() > 1;
    let candidate = if raw.is_absolute() {
        raw
    } else if has_parent_component {
        vault_path.join(raw)
    } else {
        note_path.parent().unwrap_or(vault_path).join(raw)
    };

    normalize_new_note_path(vault_path, candidate).ok_or_else(|| StateError::InvalidRenameTarget {
        path: note_path.to_path_buf(),
        new_name: new_name.to_string(),
    })
}
