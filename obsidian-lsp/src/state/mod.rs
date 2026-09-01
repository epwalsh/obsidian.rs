use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use obsidian_core::{
    BrokenLink, DuplicateAlias, DuplicateId, ExtractEdits, ExtractSelection, InlineLocation, Link, LocatedLink,
    Location as CoreLocation, Note, NoteError, TextSpan, Vault, VaultError, default_note_id_for_path,
};
use serde_json::{Value, json};
use thiserror::Error;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, Command, CompletionItem, CompletionItemKind, CompletionTextEdit, Diagnostic,
    DiagnosticSeverity, DocumentChangeOperation, DocumentChanges, DocumentLink, DocumentSymbol, DocumentSymbolResponse,
    GotoDefinitionResponse, Hover, HoverContents, Location, MarkupContent, MarkupKind, NumberOrString, OneOf,
    OptionalVersionedTextDocumentIdentifier, Position, PrepareRenameResponse, Range, RenameFile, RenameFileOptions,
    ResourceOp, SymbolInformation, SymbolKind, TextDocumentContentChangeEvent, TextDocumentEdit, TextEdit, Url,
    WorkspaceEdit,
};

use crate::uri::{UriError, path_to_uri, uri_to_path, vault_relative_path};

mod code_action_request;
mod completion_request;
mod diagnostic_helpers;
mod diagnostics_request;
mod document_link_data;
mod document_links_request;
mod document_symbols_request;
mod edits;
mod extract_note_request;
mod formatting_request;
mod frontmatter;
mod headings;
mod links;
mod navigation_request;
mod prepare_rename_request;
mod ranges;
mod rename_request;
mod resolve_document_link_request;
mod tags;
mod workspace_symbols_request;

pub use self::code_action_request::CodeActionRequest;
pub(crate) use self::code_action_request::new_note_content;
pub use self::completion_request::CompletionRequest;
#[cfg(test)]
pub use self::diagnostics_request::DiagnosticUpdate;
pub use self::diagnostics_request::DiagnosticsRequest;
pub use self::document_links_request::DocumentLinksRequest;
pub use self::document_symbols_request::DocumentSymbolsRequest;
pub use self::extract_note_request::{ExtractNoteRequest, ExtractNoteSelection};
pub use self::formatting_request::FormattingRequest;
pub(crate) use self::links::normalize_new_note_path;
pub use self::navigation_request::NavigationRequest;
pub use self::prepare_rename_request::PrepareRenameRequest;
pub use self::rename_request::RenameRequest;
pub use self::resolve_document_link_request::ResolveDocumentLinkRequest;
pub use self::workspace_symbols_request::WorkspaceSymbolsRequest;

#[cfg(test)]
pub(in crate::state) use self::completion_request::{LinkContext, detect_link_context};
pub(in crate::state) use self::diagnostic_helpers::*;
#[cfg(test)]
pub(in crate::state) use self::diagnostics_request::DiagnosticsBatch;
pub(in crate::state) use self::diagnostics_request::{PrimaryDocument, duplicate_id_range};
pub(in crate::state) use self::document_link_data::*;
pub(in crate::state) use self::document_symbols_request::symbol_tag_ranges;
pub(in crate::state) use self::edits::*;
pub(in crate::state) use self::frontmatter::*;
pub(in crate::state) use self::headings::*;
pub(in crate::state) use self::links::*;
pub(in crate::state) use self::ranges::*;
pub(in crate::state) use self::rename_request::rename_target;
pub(in crate::state) use self::tags::*;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub vault_path_override: Option<PathBuf>,
    pub diagnostics_ignore: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenDocument {
    pub uri: Url,
    pub path: PathBuf,
    pub version: i32,
    pub text: String,
    pub shadows_disk: bool,
}

#[derive(Clone, Debug)]
pub(in crate::state) struct StateSnapshot {
    pub(in crate::state) vault_path: PathBuf,
    pub(in crate::state) vault: Arc<Vault>,
    pub(in crate::state) open_documents: HashMap<PathBuf, OpenDocument>,
    pub(in crate::state) diagnostics_ignore: Vec<String>,
}
#[derive(Debug, Error)]
pub enum StateError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Note(#[from] NoteError),
    #[error(transparent)]
    Uri(#[from] UriError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error("didChange for '{uri}' did not include full document content")]
    MissingDocumentContent { uri: Url },
    #[error("invalid document link data: {0}")]
    InvalidDocumentLinkData(String),
    #[error("invalid rename target '{new_name}' for note '{path}'")]
    InvalidRenameTarget { path: PathBuf, new_name: String },
    #[error("invalid extract target '{new_path}' for note '{path}'")]
    InvalidExtractTarget { path: PathBuf, new_path: String },
    #[error("invalid tag rename target '{0}'")]
    InvalidTagRenameTarget(String),
    #[error("vault index is not ready yet")]
    NotReady,
}

pub struct BackendState {
    vault: Arc<Vault>,
    open_documents: HashMap<PathBuf, OpenDocument>,
    published_diagnostics: HashMap<PathBuf, Url>,
    diagnostics_revision: u64,
    config: Config,
    is_indexed: bool,
}

#[derive(Clone, Debug)]
pub enum FileChangeKind {
    Created,
    Changed,
    Deleted,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: FileChangeKind,
}

impl BackendState {
    #[cfg(test)]
    pub fn new(vault: Vault) -> Self {
        let vault = Vault::open_cached(vault.path()).expect("opened vault path should be cacheable");
        Self::with_index_state(vault, true)
    }

    pub(crate) fn new_unindexed(vault: Vault) -> Self {
        Self::with_index_state(vault, false)
    }

    fn with_index_state(vault: Vault, is_indexed: bool) -> Self {
        Self {
            vault: Arc::new(vault),
            open_documents: HashMap::new(),
            published_diagnostics: HashMap::new(),
            diagnostics_revision: 0,
            config: Config::default(),
            is_indexed,
        }
    }

    pub fn apply_config(&mut self, config: Config) -> Result<(), StateError> {
        if let Some(new_path) = &config.vault_path_override
            && new_path != self.vault.path()
        {
            self.vault = Arc::new(Vault::open(new_path)?);
            self.open_documents.clear();
            self.published_diagnostics.clear();
            self.diagnostics_revision += 1;
            self.is_indexed = false;
        }
        self.config = config;
        Ok(())
    }

    pub fn install_indexed_vault(&mut self, vault: Vault) -> Option<DiagnosticsRequest> {
        if vault.path() != self.vault.path() {
            return None;
        }

        self.vault = Arc::new(vault);
        self.is_indexed = true;
        Some(self.prepare_diagnostics_request(None))
    }

    pub fn is_indexed(&self) -> bool {
        self.is_indexed
    }

    pub fn global_diagnostics_request(&mut self) -> DiagnosticsRequest {
        self.prepare_diagnostics_request(None)
    }

    pub fn vault_path(&self) -> &Path {
        self.vault.path()
    }

    pub fn diagnostics_revision(&self) -> u64 {
        self.diagnostics_revision
    }

    pub fn set_published_diagnostics(&mut self, published_diagnostics: HashMap<PathBuf, Url>) {
        self.published_diagnostics = published_diagnostics;
    }

    pub fn open_document(&mut self, uri: Url, version: i32, text: String) -> Result<DiagnosticsRequest, StateError> {
        self.sync_document(uri, version, text)
    }

    pub fn change_document(
        &mut self,
        uri: Url,
        version: i32,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Result<DiagnosticsRequest, StateError> {
        let text = changes
            .last()
            .map(|change| change.text.clone())
            .ok_or_else(|| StateError::MissingDocumentContent { uri: uri.clone() })?;

        self.sync_document(uri, version, text)
    }

    pub fn close_document(&mut self, uri: Url) -> Result<DiagnosticsRequest, StateError> {
        let path = self.path_from_uri(&uri)?;
        self.open_documents.remove(&path);
        if !self.is_indexed {
            return Err(StateError::NotReady);
        }
        Arc::make_mut(&mut self.vault).refresh_cached_note(&path)?;

        Ok(self.prepare_diagnostics_request(Some(PrimaryDocument {
            path,
            uri,
            version: None,
        })))
    }

    pub fn apply_file_changes(&mut self, changes: Vec<FileChange>) -> Result<Option<DiagnosticsRequest>, StateError> {
        let mut changed = false;

        for change in changes {
            let path = self.vault.normalize_path(&change.path);
            vault_relative_path(self.vault.path(), &path)?;
            if !Vault::is_note_path(&path) {
                continue;
            }

            match change.kind {
                FileChangeKind::Created | FileChangeKind::Changed => {
                    if let Some(document) = self.open_documents.get_mut(&path) {
                        if !document.shadows_disk && path.exists() {
                            document.shadows_disk = true;
                        }
                        changed = true;
                    } else {
                        Arc::make_mut(&mut self.vault).refresh_cached_note(&path)?;
                        changed = true;
                    }
                }
                FileChangeKind::Deleted => {
                    Arc::make_mut(&mut self.vault).remove_cached_note(&path);
                    changed = true;
                }
            }
        }

        if !self.is_indexed {
            return Ok(None);
        }

        Ok(changed.then(|| self.prepare_diagnostics_request(None)))
    }

    pub fn document_links_request(&self, uri: Url) -> Result<DocumentLinksRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;

        Ok(DocumentLinksRequest {
            snapshot: self.snapshot(),
            path,
            uri,
        })
    }

    pub fn document_symbols_request(&self, uri: Url) -> Result<DocumentSymbolsRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;

        Ok(DocumentSymbolsRequest {
            snapshot: self.snapshot(),
            path,
        })
    }

    pub fn formatting_request(&self, uri: Url) -> Result<FormattingRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;

        Ok(FormattingRequest {
            snapshot: self.snapshot(),
            path,
        })
    }

    pub fn workspace_symbols_request(&self, query: String) -> Result<WorkspaceSymbolsRequest, StateError> {
        self.ensure_indexed()?;
        Ok(WorkspaceSymbolsRequest {
            snapshot: self.snapshot(),
            query,
        })
    }

    pub fn resolve_document_link_request(
        &self,
        document_link: DocumentLink,
    ) -> Result<ResolveDocumentLinkRequest, StateError> {
        self.ensure_indexed()?;
        let data = parse_document_link_data(
            document_link
                .data
                .as_ref()
                .ok_or_else(|| StateError::InvalidDocumentLinkData("missing documentLink.data".to_string()))?,
        )?;
        let source_uri = Url::parse(&data.source_uri).map_err(|error| {
            StateError::InvalidDocumentLinkData(format!("invalid source URI '{}': {error}", data.source_uri))
        })?;
        let source_path = self.path_from_uri(&source_uri)?;

        Ok(ResolveDocumentLinkRequest {
            snapshot: self.snapshot(),
            source_path,
            document_link,
            raw_link: data.raw_link,
        })
    }

    pub fn navigation_request(&self, uri: Url, position: Position) -> Result<NavigationRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;

        Ok(NavigationRequest {
            snapshot: self.snapshot(),
            path,
            position,
        })
    }

    pub fn completion_request(&self, uri: Url, position: Position) -> Result<CompletionRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;

        Ok(CompletionRequest {
            snapshot: self.snapshot(),
            path,
            position,
        })
    }

    pub fn code_action_request(
        &self,
        uri: Url,
        range: Range,
        diagnostics: Vec<Diagnostic>,
    ) -> Result<CodeActionRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;

        Ok(CodeActionRequest {
            snapshot: self.snapshot(),
            path,
            position: range.start,
            range,
            diagnostics,
        })
    }

    pub fn prepare_rename_request(&self, uri: Url, position: Position) -> Result<PrepareRenameRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;

        Ok(PrepareRenameRequest {
            snapshot: self.snapshot(),
            path,
            position,
        })
    }

    pub fn rename_request(&self, uri: Url, position: Position, new_name: String) -> Result<RenameRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;

        Ok(RenameRequest {
            snapshot: self.snapshot(),
            path,
            position,
            new_name,
        })
    }

    pub fn extract_note_request(
        &self,
        uri: Url,
        selection: ExtractNoteSelection,
        new_path: String,
        new_id: Option<String>,
        replace_with: Option<String>,
    ) -> Result<ExtractNoteRequest, StateError> {
        self.ensure_indexed()?;
        let path = self.path_from_uri(&uri)?;
        let normalized_new_path =
            normalize_new_note_path(self.vault.path(), &new_path).ok_or_else(|| StateError::InvalidExtractTarget {
                path: path.clone(),
                new_path: new_path.clone(),
            })?;

        Ok(ExtractNoteRequest {
            snapshot: self.snapshot(),
            path,
            selection,
            new_path: normalized_new_path,
            new_id,
            replace_with,
        })
    }

    fn sync_document(&mut self, uri: Url, version: i32, text: String) -> Result<DiagnosticsRequest, StateError> {
        let path = self.path_from_uri(&uri)?;
        let shadows_disk = self
            .open_documents
            .get(&path)
            .map(|document| document.shadows_disk)
            .unwrap_or_else(|| path.exists() || self.vault.has_cached_note(&path));

        // Only shadow the on-disk note when the file actually exists. If the file doesn't exist
        // yet, a preview plugin may have opened a temporary buffer (e.g. to show a diff for a
        // "Create note" code action) without the user intending to create the file. Loading a
        // non-existent note into vault memory would prematurely resolve broken-link diagnostics.
        self.open_documents.insert(
            path.clone(),
            OpenDocument {
                uri: uri.clone(),
                path: path.clone(),
                version,
                text,
                shadows_disk,
            },
        );

        if !self.is_indexed {
            return Err(StateError::NotReady);
        }

        Ok(self.prepare_diagnostics_request(Some(PrimaryDocument {
            path,
            uri,
            version: Some(version),
        })))
    }

    fn prepare_diagnostics_request(&mut self, primary_document: Option<PrimaryDocument>) -> DiagnosticsRequest {
        self.diagnostics_revision += 1;

        DiagnosticsRequest {
            snapshot: self.snapshot(),
            previously_published: self.published_diagnostics.clone(),
            primary_document,
            revision: self.diagnostics_revision,
        }
    }

    fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            vault_path: self.vault.path().to_path_buf(),
            vault: Arc::clone(&self.vault),
            open_documents: self.open_documents.clone(),
            diagnostics_ignore: self.config.diagnostics_ignore.clone(),
        }
    }

    fn path_from_uri(&self, uri: &Url) -> Result<PathBuf, StateError> {
        let path = uri_to_path(uri)?;
        vault_relative_path(self.vault.path(), &path)?;
        Ok(path)
    }

    fn ensure_indexed(&self) -> Result<(), StateError> {
        if self.is_indexed {
            Ok(())
        } else {
            Err(StateError::NotReady)
        }
    }
}

impl StateSnapshot {
    fn build_vault(&self) -> Result<Vault, StateError> {
        let mut vault = self.vault.as_ref().clone();
        for document in self.open_documents.values() {
            // Only buffers that correspond to real indexed/disk notes shadow the index. Preview
            // buffers for not-yet-created notes stay out of vault-wide diagnostics until a file
            // creation event marks them as disk-backed.
            if document.shadows_disk {
                vault.load_note(Note::parse(&document.path, &document.text));
            }
        }
        Ok(vault)
    }

    fn notes(&self) -> Vec<Note> {
        let mut notes = self
            .vault
            .notes()
            .into_iter()
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        let overlay_paths = self
            .open_documents
            .values()
            .filter(|document| document.shadows_disk)
            .map(|document| document.path.clone())
            .collect::<HashSet<_>>();
        notes.retain(|note| !overlay_paths.contains(&note.path));
        notes.extend(
            self.open_documents
                .values()
                .filter(|document| document.shadows_disk)
                .map(|document| Note::parse(&document.path, &document.text)),
        );
        notes.sort_by(|left, right| left.path.cmp(&right.path));
        notes
    }

    fn note_for_path(&self, path: &Path) -> Result<Note, StateError> {
        if let Some(document) = self.open_documents.get(path) {
            Ok(Note::parse(path, &document.text))
        } else {
            Ok(self.vault.note_for_path(path)?)
        }
    }

    fn text_for_path(&self, path: &Path) -> Result<String, StateError> {
        if let Some(document) = self.open_documents.get(path) {
            Ok(document.text.clone())
        } else {
            Ok(self.vault.text_for_path(path)?)
        }
    }

    fn list_tags(&self) -> Vec<String> {
        let mut tags = BTreeSet::new();
        for note in self.notes() {
            tags.extend(note.tags.into_iter().map(|tag| tag.tag.to_lowercase()));
        }
        tags.into_iter().collect()
    }

    fn uri_for_path(&self, path: &Path) -> Result<Url, StateError> {
        if let Some(document) = self.open_documents.get(path) {
            Ok(document.uri.clone())
        } else {
            Ok(path_to_uri(path)?)
        }
    }

    fn version_for_path(&self, path: &Path) -> Option<i32> {
        self.open_documents.get(path).map(|document| document.version)
    }
}

#[cfg(test)]
mod tests;
