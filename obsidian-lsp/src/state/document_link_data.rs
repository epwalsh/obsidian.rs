use super::*;

pub(in crate::state) struct DocumentLinkData {
    pub(in crate::state) source_uri: String,
    pub(in crate::state) raw_link: String,
}
pub(in crate::state) const DOCUMENT_LINK_SOURCE_URI_KEY: &str = "sourceUri";
pub(in crate::state) const DOCUMENT_LINK_RAW_LINK_KEY: &str = "rawLink";

pub(in crate::state) fn parse_document_link_data(value: &Value) -> Result<DocumentLinkData, StateError> {
    let object = value
        .as_object()
        .ok_or_else(|| StateError::InvalidDocumentLinkData("documentLink.data was not a JSON object".to_string()))?;
    let source_uri = object
        .get(DOCUMENT_LINK_SOURCE_URI_KEY)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            StateError::InvalidDocumentLinkData(format!(
                "documentLink.data did not include a string '{DOCUMENT_LINK_SOURCE_URI_KEY}'"
            ))
        })?;
    let raw_link = object
        .get(DOCUMENT_LINK_RAW_LINK_KEY)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            StateError::InvalidDocumentLinkData(format!(
                "documentLink.data did not include a string '{DOCUMENT_LINK_RAW_LINK_KEY}'"
            ))
        })?;

    Ok(DocumentLinkData {
        source_uri: source_uri.to_string(),
        raw_link: raw_link.to_string(),
    })
}
