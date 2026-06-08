use super::*;

#[derive(Debug)]
pub struct DocumentLinksRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) uri: Url,
}

impl DocumentLinksRequest {
    pub fn compute(self) -> Result<Vec<DocumentLink>, StateError> {
        let source_note = self.snapshot.note_for_path(&self.path)?;

        Ok(source_note
            .links
            .iter()
            .filter_map(|link| build_document_link(&self.uri, link))
            .collect())
    }
}

pub(in crate::state) fn build_document_link(source_uri: &Url, link: &LocatedLink) -> Option<DocumentLink> {
    Some(DocumentLink {
        range: location_to_range(&link.location),
        target: None,
        tooltip: None,
        data: Some(json!({
            DOCUMENT_LINK_SOURCE_URI_KEY: source_uri.as_str(),
            DOCUMENT_LINK_RAW_LINK_KEY: render_link_text(&link.link)?,
        })),
    })
}

pub(in crate::state) fn render_link_text(link: &Link) -> Option<String> {
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
            Some(text)
        }
        Link::Markdown { text, url } => Some(format!("[{text}]({url})")),
        Link::Embed { .. } => None,
    }
}
