use super::*;

#[derive(Debug)]
pub struct ResolveDocumentLinkRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) source_path: PathBuf,
    pub(in crate::state) document_link: DocumentLink,
    pub(in crate::state) raw_link: String,
}

impl ResolveDocumentLinkRequest {
    pub fn compute(mut self) -> Result<DocumentLink, StateError> {
        let source_note = synthetic_note_for_raw_link(&self.source_path, &self.raw_link)?;
        let link = source_note.links.first().ok_or_else(|| {
            StateError::InvalidDocumentLinkData("raw link did not parse into a supported link".to_string())
        })?;

        if let Some(target) = direct_document_link_target(&self.source_path, link, self.snapshot.vault_path.as_path())?
        {
            self.document_link.target = Some(target);
            self.document_link.tooltip = Some(render_document_link_tooltip(
                Some("Resolved external or file link"),
                None,
            ));
            return Ok(self.document_link);
        }

        let notes = self.snapshot.notes();
        let matching_notes = resolve_link_targets(
            &source_note.path,
            &link.link,
            &notes,
            self.snapshot.vault_path.as_path(),
        );

        match matching_notes.as_slice() {
            [] => {
                self.document_link.tooltip = Some(render_document_link_tooltip(Some("Broken note link"), None));
            }
            [note] => {
                self.document_link.target = Some(note_target_uri(note, &link.link)?);
                self.document_link.tooltip = Some(render_document_link_tooltip(
                    Some("Resolved note link"),
                    Some(render_note_link_label(note, self.snapshot.vault_path.as_path())),
                ));
            }
            notes => {
                self.document_link.tooltip = Some(render_ambiguous_document_link_tooltip(
                    notes,
                    self.snapshot.vault_path.as_path(),
                ));
            }
        }

        Ok(self.document_link)
    }
}

pub(in crate::state) fn synthetic_note_for_raw_link(source_path: &Path, raw_link: &str) -> Result<Note, StateError> {
    let note = Note::parse(source_path, raw_link);
    if note.links.is_empty() {
        return Err(StateError::InvalidDocumentLinkData(
            "raw link did not parse into a wiki or markdown link".to_string(),
        ));
    }

    Ok(note)
}

pub(in crate::state) fn direct_document_link_target(
    source_path: &Path,
    link: &LocatedLink,
    vault_path: &Path,
) -> Result<Option<Url>, StateError> {
    match &link.link {
        Link::Markdown { url, .. } => {
            if url.contains("://") {
                return Ok(Url::parse(url).ok());
            }

            let Some(path) = resolve_local_file_target_path(source_path, url, vault_path) else {
                return Ok(None);
            };

            Ok(Some(path_to_uri(&path)?))
        }
        Link::Wiki { .. } | Link::Embed { .. } => Ok(None),
    }
}

pub(in crate::state) fn resolve_local_file_target_path(
    source_path: &Path,
    url: &str,
    vault_path: &Path,
) -> Option<PathBuf> {
    let path = markdown_url_path(url)?;
    if path.ends_with(".md") {
        return None;
    }

    local_markdown_candidates(source_path, &path, vault_path)
        .into_iter()
        .find(|candidate| candidate.exists())
}

pub(in crate::state) fn render_document_link_tooltip(prefix: Option<&str>, detail: Option<String>) -> String {
    match (prefix, detail) {
        (Some(prefix), Some(detail)) => format!("{prefix}: {detail}"),
        (Some(prefix), None) => prefix.to_string(),
        (None, Some(detail)) => detail,
        (None, None) => "Obsidian link".to_string(),
    }
}

pub(in crate::state) fn render_ambiguous_document_link_tooltip(notes: &[&Note], vault_path: &Path) -> String {
    format!(
        "Ambiguous note link: {}",
        notes
            .iter()
            .map(|note| relative_display(vault_path, &note.path))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(in crate::state) fn render_note_link_label(note: &Note, vault_path: &Path) -> String {
    let title = note.title.as_deref().unwrap_or(note.id.as_str());
    format!("{title} ({})", relative_display(vault_path, &note.path))
}
