use super::*;

pub(in crate::state) fn workspace_edit_from_text_edits(
    snapshot: &StateSnapshot,
    mut edits_by_path: HashMap<PathBuf, Vec<TextEdit>>,
) -> Result<WorkspaceEdit, StateError> {
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
        edits.dedup_by(|left, right| left.range == right.range);
        if edits.is_empty() {
            continue;
        }

        operations.push(DocumentChangeOperation::Edit(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: snapshot.uri_for_path(&path)?,
                version: snapshot.version_for_path(&path),
            },
            edits: edits.into_iter().map(OneOf::Left).collect(),
        }));
    }

    Ok(WorkspaceEdit {
        document_changes: Some(DocumentChanges::Operations(operations)),
        ..Default::default()
    })
}
