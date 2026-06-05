use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use obsidian_core::{
    BrokenLink, DuplicateAlias, DuplicateId, InlineLocation, Link, LocatedLink, Location as CoreLocation, Note,
    NoteError, Vault, VaultError,
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
}

#[derive(Clone, Debug)]
pub struct DiagnosticUpdate {
    pub uri: Url,
    pub version: Option<i32>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub struct DiagnosticsBatch {
    pub revision: u64,
    pub updates: Vec<DiagnosticUpdate>,
    pub published_diagnostics: HashMap<PathBuf, Url>,
}

#[derive(Clone, Debug)]
struct StateSnapshot {
    vault_path: PathBuf,
    open_documents: HashMap<PathBuf, OpenDocument>,
    diagnostics_ignore: Vec<String>,
}

#[derive(Debug)]
pub struct DiagnosticsRequest {
    snapshot: StateSnapshot,
    previously_published: HashMap<PathBuf, Url>,
    primary_document: PrimaryDocument,
    revision: u64,
}

#[derive(Debug)]
pub struct DocumentLinksRequest {
    snapshot: StateSnapshot,
    path: PathBuf,
    uri: Url,
}

#[derive(Debug)]
pub struct DocumentSymbolsRequest {
    snapshot: StateSnapshot,
    path: PathBuf,
}

#[derive(Debug)]
pub struct WorkspaceSymbolsRequest {
    snapshot: StateSnapshot,
    query: String,
}

#[derive(Debug)]
pub struct ResolveDocumentLinkRequest {
    snapshot: StateSnapshot,
    source_path: PathBuf,
    document_link: DocumentLink,
    raw_link: String,
}

#[derive(Debug)]
pub struct NavigationRequest {
    snapshot: StateSnapshot,
    path: PathBuf,
    position: Position,
}

#[derive(Debug)]
pub struct CompletionRequest {
    snapshot: StateSnapshot,
    path: PathBuf,
    position: Position,
}

#[derive(Debug)]
pub struct CodeActionRequest {
    snapshot: StateSnapshot,
    path: PathBuf,
    range: Range,
    position: Position,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub struct PrepareRenameRequest {
    snapshot: StateSnapshot,
    path: PathBuf,
    position: Position,
}

#[derive(Debug)]
pub struct RenameRequest {
    snapshot: StateSnapshot,
    path: PathBuf,
    position: Position,
    new_name: String,
}

enum LinkContext {
    Wiki {
        note_query: String,
        heading_query: Option<String>,
        link_start_char: usize,
    },
    Markdown {
        query: String,
        link_start_char: usize,
    },
    Tag {
        query: String,
        tag_start_char: usize,
    },
}

struct NavigationContext {
    snapshot: StateSnapshot,
    vault: Vault,
    notes: Vec<Note>,
    source_note: Note,
    selected_link: Option<LocatedLink>,
    selected_tag: Option<TagSelection>,
}

struct RenameTarget {
    note: Note,
    range: Range,
    placeholder: String,
}

#[derive(Clone, Debug)]
struct TagSelection {
    tag: String,
    range: Range,
    rename_range: Range,
    placeholder: String,
}

#[derive(Clone, Debug)]
struct TagOccurrence {
    path: PathBuf,
    range: Range,
    inline: bool,
}

#[derive(Clone, Debug)]
struct FrontmatterTagRange {
    tag: String,
    range: Range,
}

#[derive(Clone, Debug)]
struct FrontmatterKeyRange {
    key: String,
    range: Range,
}

#[derive(Clone, Debug)]
struct FrontmatterValueRange {
    value: String,
    range: Range,
}

#[derive(Clone, Debug)]
struct HeadingSymbol {
    name: String,
    range: Range,
}

#[derive(Debug)]
struct PrimaryDocument {
    path: PathBuf,
    uri: Url,
    version: Option<i32>,
}

struct DocumentLinkData {
    source_uri: String,
    raw_link: String,
}

const DOCUMENT_LINK_SOURCE_URI_KEY: &str = "sourceUri";
const DOCUMENT_LINK_RAW_LINK_KEY: &str = "rawLink";
const WORKSPACE_SYMBOL_LIMIT: usize = 500;

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
    #[error("invalid tag rename target '{0}'")]
    InvalidTagRenameTarget(String),
}

pub struct BackendState {
    vault: Vault,
    open_documents: HashMap<PathBuf, OpenDocument>,
    published_diagnostics: HashMap<PathBuf, Url>,
    diagnostics_revision: u64,
    config: Config,
}

impl BackendState {
    pub fn new(vault: Vault) -> Self {
        Self {
            vault,
            open_documents: HashMap::new(),
            published_diagnostics: HashMap::new(),
            diagnostics_revision: 0,
            config: Config::default(),
        }
    }

    pub fn apply_config(&mut self, config: Config) -> Result<(), StateError> {
        if let Some(new_path) = &config.vault_path_override
            && new_path != self.vault.path()
        {
            self.vault = Vault::open(new_path)?;
            self.open_documents.clear();
            self.published_diagnostics.clear();
            self.diagnostics_revision += 1;
        }
        self.config = config;
        Ok(())
    }

    pub fn global_diagnostics_request(&mut self) -> Option<DiagnosticsRequest> {
        let doc = self.open_documents.values().next()?;
        let (path, uri, version) = (doc.path.clone(), doc.uri.clone(), Some(doc.version));
        Some(self.prepare_diagnostics_request(path, uri, version))
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
        self.vault.unload_note(&path);

        Ok(self.prepare_diagnostics_request(path, uri, None))
    }

    pub fn document_links_request(&self, uri: Url) -> Result<DocumentLinksRequest, StateError> {
        let path = self.path_from_uri(&uri)?;

        Ok(DocumentLinksRequest {
            snapshot: self.snapshot(),
            path,
            uri,
        })
    }

    pub fn document_symbols_request(&self, uri: Url) -> Result<DocumentSymbolsRequest, StateError> {
        let path = self.path_from_uri(&uri)?;

        Ok(DocumentSymbolsRequest {
            snapshot: self.snapshot(),
            path,
        })
    }

    pub fn workspace_symbols_request(&self, query: String) -> WorkspaceSymbolsRequest {
        WorkspaceSymbolsRequest {
            snapshot: self.snapshot(),
            query,
        }
    }

    pub fn resolve_document_link_request(
        &self,
        document_link: DocumentLink,
    ) -> Result<ResolveDocumentLinkRequest, StateError> {
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
        let path = self.path_from_uri(&uri)?;

        Ok(NavigationRequest {
            snapshot: self.snapshot(),
            path,
            position,
        })
    }

    pub fn completion_request(&self, uri: Url, position: Position) -> Result<CompletionRequest, StateError> {
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
        let path = self.path_from_uri(&uri)?;

        Ok(PrepareRenameRequest {
            snapshot: self.snapshot(),
            path,
            position,
        })
    }

    pub fn rename_request(&self, uri: Url, position: Position, new_name: String) -> Result<RenameRequest, StateError> {
        let path = self.path_from_uri(&uri)?;

        Ok(RenameRequest {
            snapshot: self.snapshot(),
            path,
            position,
            new_name,
        })
    }

    fn sync_document(&mut self, uri: Url, version: i32, text: String) -> Result<DiagnosticsRequest, StateError> {
        let path = self.path_from_uri(&uri)?;

        // Only shadow the on-disk note when the file actually exists. If the file doesn't exist
        // yet, a preview plugin may have opened a temporary buffer (e.g. to show a diff for a
        // "Create note" code action) without the user intending to create the file. Loading a
        // non-existent note into vault memory would prematurely resolve broken-link diagnostics.
        if path.exists() {
            self.vault.load_note(Note::parse(&path, &text));
        }
        self.open_documents.insert(
            path.clone(),
            OpenDocument {
                uri: uri.clone(),
                path: path.clone(),
                version,
                text,
            },
        );

        Ok(self.prepare_diagnostics_request(path, uri, Some(version)))
    }

    fn prepare_diagnostics_request(&mut self, path: PathBuf, uri: Url, version: Option<i32>) -> DiagnosticsRequest {
        self.diagnostics_revision += 1;

        DiagnosticsRequest {
            snapshot: self.snapshot(),
            previously_published: self.published_diagnostics.clone(),
            primary_document: PrimaryDocument { path, uri, version },
            revision: self.diagnostics_revision,
        }
    }

    fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            vault_path: self.vault.path().to_path_buf(),
            open_documents: self.open_documents.clone(),
            diagnostics_ignore: self.config.diagnostics_ignore.clone(),
        }
    }

    fn path_from_uri(&self, uri: &Url) -> Result<PathBuf, StateError> {
        let path = uri_to_path(uri)?;
        vault_relative_path(self.vault.path(), &path)?;
        Ok(path)
    }
}

impl DiagnosticsRequest {
    pub fn compute(self) -> Result<DiagnosticsBatch, StateError> {
        let vault = self.snapshot.build_vault()?;
        let ignore_set = build_ignore_set(&self.snapshot.diagnostics_ignore);
        let report = vault.check(|path| {
            let rel = path.strip_prefix(vault.path()).unwrap_or(path);
            !ignore_set.is_match(rel)
        });
        let mut diagnostics_by_path = build_diagnostics_by_path(&self.snapshot, &report)?;

        let mut paths_to_publish = BTreeSet::new();
        paths_to_publish.extend(diagnostics_by_path.keys().cloned());
        paths_to_publish.extend(self.previously_published.keys().cloned());
        paths_to_publish.insert(self.primary_document.path.clone());

        let mut published_diagnostics = HashMap::new();
        let mut updates = Vec::with_capacity(paths_to_publish.len());

        for path in paths_to_publish {
            let uri = if path == self.primary_document.path {
                self.primary_document.uri.clone()
            } else {
                self.snapshot.uri_for_path(&path)?
            };
            let version = if path == self.primary_document.path {
                self.primary_document.version
            } else {
                self.snapshot.version_for_path(&path)
            };
            let diagnostics = diagnostics_by_path.remove(&path).unwrap_or_default();

            if !diagnostics.is_empty() {
                published_diagnostics.insert(path.clone(), uri.clone());
            }

            updates.push(DiagnosticUpdate {
                uri,
                version,
                diagnostics,
            });
        }

        Ok(DiagnosticsBatch {
            revision: self.revision,
            updates,
            published_diagnostics,
        })
    }
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

impl DocumentSymbolsRequest {
    pub fn compute(self) -> Result<DocumentSymbolResponse, StateError> {
        let note = self.snapshot.note_for_path(&self.path)?;
        let text = self.snapshot.text_for_path(&self.path)?;
        Ok(DocumentSymbolResponse::Nested(document_symbols_for_note(&note, &text)))
    }
}

impl WorkspaceSymbolsRequest {
    pub fn compute(self) -> Result<Vec<SymbolInformation>, StateError> {
        let vault = self.snapshot.build_vault()?;
        let notes = self.snapshot.notes_with_content(&vault);
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

        let vault = self.snapshot.build_vault()?;
        let notes = self.snapshot.notes(&vault);
        let matching_notes = resolve_link_targets(&source_note.path, &link.link, &notes, vault.path());

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

impl NavigationRequest {
    pub fn compute_hover(self) -> Result<Option<Hover>, StateError> {
        let context = self.build_context()?;
        if let Some(selected_tag) = context.selected_tag.as_ref() {
            let occurrences = tag_occurrences(&context.snapshot, &selected_tag.tag)?;
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: render_tag_hover(&selected_tag.tag, occurrences.len()),
                }),
                range: Some(selected_tag.range),
            }));
        }

        let Some(selected_link) = context.selected_link.as_ref() else {
            return Ok(None);
        };
        let matching_notes = context.resolve_selected_link_targets();

        if matching_notes.is_empty() {
            return Ok(None);
        }

        let contents = if matching_notes.len() == 1 {
            render_note_hover(matching_notes[0], context.snapshot.vault_path.as_path())
        } else {
            render_ambiguous_hover(&matching_notes, context.snapshot.vault_path.as_path())
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: contents,
            }),
            range: Some(location_to_range(&selected_link.location)),
        }))
    }

    pub fn compute_references(self) -> Result<Option<Vec<Location>>, StateError> {
        let context = self.build_context()?;
        if let Some(selected_tag) = context.selected_tag.as_ref() {
            return Ok(Some(tag_locations(&context.snapshot, &selected_tag.tag)?));
        }

        let target_notes = if context.selected_link.is_some() {
            let matching = context.resolve_selected_link_targets();
            if matching.len() != 1 {
                return Ok(Some(Vec::new()));
            }
            vec![matching[0]]
        } else {
            // Obsidian-specific behavior: when the cursor is not on a link, treat
            // references as backlinks to the current note.
            vec![&context.source_note]
        };

        let mut locations = Vec::new();
        for target in target_notes {
            for (source_note, links) in context.vault.backlinks_from(&context.notes, target) {
                let uri = context.snapshot.uri_for_path(&source_note.path)?;
                locations.extend(links.into_iter().map(|link| Location {
                    uri: uri.clone(),
                    range: location_to_range(&link.location),
                }));
            }
        }

        locations.sort_by(|left, right| {
            left.uri
                .cmp(&right.uri)
                .then(left.range.start.line.cmp(&right.range.start.line))
                .then(left.range.start.character.cmp(&right.range.start.character))
        });
        locations.dedup_by(|left, right| left.uri == right.uri && left.range == right.range);

        Ok(Some(locations))
    }

    pub fn compute_definition(self) -> Result<Option<GotoDefinitionResponse>, StateError> {
        let context = self.build_context()?;
        if let Some(selected_tag) = context.selected_tag.as_ref() {
            let mut locations = tag_locations(&context.snapshot, &selected_tag.tag)?;
            return Ok(if locations.is_empty() {
                None
            } else if locations.len() == 1 {
                Some(GotoDefinitionResponse::Scalar(locations.remove(0)))
            } else {
                Some(GotoDefinitionResponse::Array(locations))
            });
        }

        let matching_notes = context.resolve_selected_link_targets();

        if matching_notes.is_empty() {
            // Anchor-only wiki link like [[#Heading]] — navigate within the current document.
            if let Some(LocatedLink {
                link:
                    Link::Wiki {
                        target,
                        heading: Some(_),
                        ..
                    },
                ..
            }) = &context.selected_link
                && target.is_empty()
            {
                let fragment = selected_link_fragment(context.selected_link.as_ref());
                let location = note_location(&context.snapshot, &context.source_note, fragment)?;
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }
            return Ok(None);
        }

        let mut locations = matching_notes
            .into_iter()
            .map(|note| {
                note_location(
                    &context.snapshot,
                    note,
                    selected_link_fragment(context.selected_link.as_ref()),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        locations.sort_by(|left, right| {
            left.uri
                .cmp(&right.uri)
                .then(left.range.start.line.cmp(&right.range.start.line))
                .then(left.range.start.character.cmp(&right.range.start.character))
        });

        Ok(Some(if locations.len() == 1 {
            GotoDefinitionResponse::Scalar(locations.remove(0))
        } else {
            GotoDefinitionResponse::Array(locations)
        }))
    }

    fn build_context(self) -> Result<NavigationContext, StateError> {
        let vault = self.snapshot.build_vault()?;
        let notes = self.snapshot.notes(&vault);
        let source_note = self.snapshot.note_for_path(&self.path)?;
        let selected_link = find_link_at_position(&source_note, self.position).cloned();
        let selected_tag = if selected_link.is_none() {
            find_tag_at_position(&self.snapshot, &source_note, self.position)?
        } else {
            None
        };

        Ok(NavigationContext {
            snapshot: self.snapshot,
            vault,
            notes,
            source_note,
            selected_link,
            selected_tag,
        })
    }
}

impl NavigationContext {
    fn resolve_selected_link_targets(&self) -> Vec<&Note> {
        self.selected_link
            .as_ref()
            .map(|link| resolve_link_targets(&self.source_note.path, &link.link, &self.notes, self.vault.path()))
            .unwrap_or_default()
    }
}

fn find_tag_at_position(
    snapshot: &StateSnapshot,
    note: &Note,
    position: Position,
) -> Result<Option<TagSelection>, StateError> {
    for tag in &note.tags {
        if let CoreLocation::Inline(location) = &tag.location
            && position_in_location(position, location)
        {
            let range = location_to_range(location);
            return Ok(Some(TagSelection {
                tag: tag.tag.clone(),
                rename_range: range,
                range,
                placeholder: format!("#{}", tag.tag),
            }));
        }
    }

    let frontmatter_tags = note
        .tags
        .iter()
        .filter_map(|tag| match tag.location {
            CoreLocation::Frontmatter => Some(tag.tag.as_str()),
            CoreLocation::Inline(_) => None,
        })
        .collect::<Vec<_>>();
    if frontmatter_tags.is_empty() {
        return Ok(None);
    }

    let text = snapshot.text_for_path(&note.path)?;
    for tag_range in frontmatter_tag_ranges(&text) {
        if !frontmatter_tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(&tag_range.tag))
        {
            continue;
        }
        if position_in_range(position, &tag_range.range) {
            return Ok(Some(TagSelection {
                tag: tag_range.tag.clone(),
                range: tag_range.range,
                rename_range: tag_range.range,
                placeholder: tag_range.tag,
            }));
        }
    }

    Ok(None)
}

fn tag_locations(snapshot: &StateSnapshot, tag: &str) -> Result<Vec<Location>, StateError> {
    let mut locations = tag_occurrences(snapshot, tag)?
        .into_iter()
        .map(|occurrence| {
            Ok(Location {
                uri: snapshot.uri_for_path(&occurrence.path)?,
                range: occurrence.range,
            })
        })
        .collect::<Result<Vec<_>, StateError>>()?;

    locations.sort_by(|left, right| {
        left.uri
            .cmp(&right.uri)
            .then(left.range.start.line.cmp(&right.range.start.line))
            .then(left.range.start.character.cmp(&right.range.start.character))
    });
    locations.dedup_by(|left, right| left.uri == right.uri && left.range == right.range);
    Ok(locations)
}

fn tag_occurrences(snapshot: &StateSnapshot, tag: &str) -> Result<Vec<TagOccurrence>, StateError> {
    let vault = snapshot.build_vault()?;
    let results = vault.find_tags(&[tag.to_string()])?;
    let mut occurrences = Vec::new();
    let mut frontmatter_cache: HashMap<PathBuf, Vec<FrontmatterTagRange>> = HashMap::new();
    let mut used_frontmatter_ranges: HashMap<PathBuf, Vec<Range>> = HashMap::new();

    for (note, tags) in results {
        for tag in tags {
            match tag.location {
                CoreLocation::Inline(location) => occurrences.push(TagOccurrence {
                    path: note.path.clone(),
                    range: location_to_range(&location),
                    inline: true,
                }),
                CoreLocation::Frontmatter => {
                    let ranges = match frontmatter_cache.get(&note.path) {
                        Some(ranges) => ranges,
                        None => {
                            let text = snapshot.text_for_path(&note.path)?;
                            frontmatter_cache.insert(note.path.clone(), frontmatter_tag_ranges(&text));
                            frontmatter_cache.get(&note.path).unwrap()
                        }
                    };
                    let used = used_frontmatter_ranges.entry(note.path.clone()).or_default();
                    if let Some(tag_range) = ranges.iter().find(|tag_range| {
                        tag_range.tag.eq_ignore_ascii_case(&tag.tag) && !used.contains(&tag_range.range)
                    }) {
                        used.push(tag_range.range);
                        occurrences.push(TagOccurrence {
                            path: note.path.clone(),
                            range: tag_range.range,
                            inline: false,
                        });
                    }
                }
            }
        }
    }

    occurrences.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.range.start.line.cmp(&right.range.start.line))
            .then(left.range.start.character.cmp(&right.range.start.character))
    });
    Ok(occurrences)
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
        let char_pos = (position.character as usize).min(line.len());
        let line_prefix = &line[..char_pos];
        let text_after_cursor = &line[char_pos..];

        let context = match detect_link_context(line_prefix) {
            Some(ctx) => ctx,
            None => return Ok(None),
        };

        if let LinkContext::Tag { query, tag_start_char } = &context {
            let prefix_range = Range::new(
                Position::new(position.line, *tag_start_char as u32),
                Position::new(position.line, char_pos as u32),
            );
            let vault = snapshot.build_vault()?;
            let all_tags = vault.list_tags().map_err(StateError::Vault)?;
            return Ok(Some(tag_completions(&all_tags, query, prefix_range)));
        }

        let (note_query, heading_query, link_start_char) = match &context {
            LinkContext::Wiki {
                note_query,
                heading_query,
                link_start_char,
            } => (note_query.as_str(), heading_query.as_deref(), *link_start_char),
            LinkContext::Markdown { query, link_start_char } => (query.as_str(), None, *link_start_char),
            LinkContext::Tag { .. } => unreachable!(),
        };

        let close_len = closing_bracket_len(text_after_cursor, &context);
        let prefix_range = Range::new(
            Position::new(position.line, link_start_char as u32),
            Position::new(position.line, (char_pos + close_len) as u32),
        );

        let vault = snapshot.build_vault()?;
        let notes = snapshot.notes(&vault);

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
                    LinkContext::Markdown { .. } => markdown_completions_for_note(note, vault.path(), prefix_range),
                    LinkContext::Tag { .. } => unreachable!(),
                })
                .collect()
        };

        Ok(Some(items))
    }
}

impl StateSnapshot {
    fn build_vault(&self) -> Result<Vault, StateError> {
        let mut vault = Vault::open(&self.vault_path)?;
        for document in self.open_documents.values() {
            // Only shadow on-disk notes. If the file doesn't exist yet the buffer was likely
            // opened by a preview plugin for a "Create note" action; loading it would
            // prematurely resolve broken-link diagnostics before the file is actually created.
            if document.path.exists() {
                vault.load_note(Note::parse(&document.path, &document.text));
            }
        }
        Ok(vault)
    }

    fn notes(&self, vault: &Vault) -> Vec<Note> {
        vault
            .notes_filtered(|_| true)
            .into_iter()
            .filter_map(Result::ok)
            .collect()
    }

    fn notes_with_content(&self, vault: &Vault) -> Vec<Note> {
        vault
            .notes_filtered_with_content(|_| true)
            .into_iter()
            .filter_map(Result::ok)
            .collect()
    }

    fn note_for_path(&self, path: &Path) -> Result<Note, StateError> {
        if let Some(document) = self.open_documents.get(path) {
            Ok(Note::parse(path, &document.text))
        } else {
            Ok(Note::from_path(path)?)
        }
    }

    fn text_for_path(&self, path: &Path) -> Result<String, StateError> {
        if let Some(document) = self.open_documents.get(path) {
            Ok(document.text.clone())
        } else {
            Ok(fs::read_to_string(path)?)
        }
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

fn build_diagnostics_by_path(
    snapshot: &StateSnapshot,
    report: &obsidian_core::VaultHealthReport,
) -> Result<HashMap<PathBuf, Vec<Diagnostic>>, StateError> {
    let mut diagnostics_by_path: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();

    for duplicate in &report.duplicate_ids {
        add_duplicate_id_diagnostics(snapshot, &mut diagnostics_by_path, duplicate)?;
    }

    for duplicate in &report.duplicate_aliases {
        add_duplicate_alias_diagnostics(snapshot, &mut diagnostics_by_path, duplicate)?;
    }

    add_broken_link_diagnostics(snapshot, &mut diagnostics_by_path, &report.broken_links)?;

    for diagnostics in diagnostics_by_path.values_mut() {
        diagnostics.sort_by(|left, right| {
            left.range
                .start
                .line
                .cmp(&right.range.start.line)
                .then(left.range.start.character.cmp(&right.range.start.character))
                .then(left.message.cmp(&right.message))
        });
    }

    Ok(diagnostics_by_path)
}

fn add_duplicate_id_diagnostics(
    snapshot: &StateSnapshot,
    diagnostics_by_path: &mut HashMap<PathBuf, Vec<Diagnostic>>,
    duplicate: &DuplicateId,
) -> Result<(), StateError> {
    for note in &duplicate.notes {
        let other_paths = duplicate
            .notes
            .iter()
            .filter(|candidate| candidate.path != note.path)
            .map(|candidate| relative_display(snapshot.vault_path.as_path(), &candidate.path))
            .collect::<Vec<_>>();

        diagnostics_by_path
            .entry(note.path.clone())
            .or_default()
            .push(make_diagnostic(
                duplicate_id_range(snapshot, &note.path)?,
                "duplicate-id",
                format!(
                    "Duplicate note ID `{}` also used by {}.",
                    duplicate.id,
                    other_paths.join(", ")
                ),
            ));
    }

    Ok(())
}

fn add_duplicate_alias_diagnostics(
    snapshot: &StateSnapshot,
    diagnostics_by_path: &mut HashMap<PathBuf, Vec<Diagnostic>>,
    duplicate: &DuplicateAlias,
) -> Result<(), StateError> {
    for note in &duplicate.notes {
        let other_paths = duplicate
            .notes
            .iter()
            .filter(|candidate| candidate.path != note.path)
            .map(|candidate| relative_display(snapshot.vault_path.as_path(), &candidate.path))
            .collect::<Vec<_>>();

        diagnostics_by_path
            .entry(note.path.clone())
            .or_default()
            .push(make_diagnostic(
                duplicate_alias_range(snapshot, &note.path, &duplicate.alias)?,
                "duplicate-alias",
                format!(
                    "Duplicate alias `{}` also used by {}.",
                    duplicate.alias,
                    other_paths.join(", ")
                ),
            ));
    }

    Ok(())
}

fn add_broken_link_diagnostics(
    snapshot: &StateSnapshot,
    diagnostics_by_path: &mut HashMap<PathBuf, Vec<Diagnostic>>,
    broken_links: &[BrokenLink],
) -> Result<(), StateError> {
    let mut note_cache: HashMap<PathBuf, Note> = HashMap::new();
    let mut used_ranges: HashMap<PathBuf, Vec<Range>> = HashMap::new();

    for broken in broken_links {
        let note = match note_cache.get(&broken.source_path) {
            Some(note) => note,
            None => {
                let note = snapshot.note_for_path(&broken.source_path)?;
                note_cache.insert(broken.source_path.clone(), note);
                note_cache.get(&broken.source_path).unwrap()
            }
        };
        let range = find_broken_link_range(note, broken, used_ranges.entry(broken.source_path.clone()).or_default())
            .unwrap_or_else(|| line_start_range(broken.line));

        diagnostics_by_path
            .entry(broken.source_path.clone())
            .or_default()
            .push(make_diagnostic(
                range,
                "broken-link",
                format!("Broken link {}.", broken.text),
            ));
    }

    Ok(())
}

fn find_broken_link_range(note: &Note, broken: &BrokenLink, used_ranges: &mut Vec<Range>) -> Option<Range> {
    for link in &note.links {
        let range = location_to_range(&link.location);
        if used_ranges.contains(&range) {
            continue;
        }
        if link.location.line != broken.line || !link_matches_broken(link, broken) {
            continue;
        }

        used_ranges.push(range);
        return Some(range);
    }

    None
}

fn link_matches_broken(link: &obsidian_core::LocatedLink, broken: &BrokenLink) -> bool {
    match &link.link {
        Link::Wiki { target, .. } => broken.text == format!("[[{}]]", target),
        Link::Markdown { url, .. } => broken.text == format!("[...]({})", url),
        Link::Embed { .. } => false,
    }
}

fn duplicate_id_range(snapshot: &StateSnapshot, path: &Path) -> Result<Range, StateError> {
    let text = snapshot.text_for_path(path)?;
    Ok(find_frontmatter_key_range(&text, "id").unwrap_or_else(document_start_range))
}

fn duplicate_alias_range(snapshot: &StateSnapshot, path: &Path, alias: &str) -> Result<Range, StateError> {
    let text = snapshot.text_for_path(path)?;
    if let Some(range) = frontmatter_alias_ranges(&text)
        .into_iter()
        .find(|alias_range| alias_range.value.eq_ignore_ascii_case(alias))
        .map(|alias_range| alias_range.range)
    {
        return Ok(range);
    }

    let note = snapshot.note_for_path(path)?;
    if let Some(title) = note.title.as_deref()
        && title.eq_ignore_ascii_case(alias)
        && let Some(range) = find_title_or_heading_range(&text, title)
    {
        return Ok(range);
    }

    Ok(find_alias_range(&text, alias).unwrap_or_else(document_start_range))
}

fn find_frontmatter_key_range(text: &str, key: &str) -> Option<Range> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }

    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" {
            break;
        }
        if let Some(col_start) = line.find(&format!("{key}:")) {
            return Some(range_for_span(line_index, col_start, key.len()));
        }
    }

    None
}

fn find_frontmatter_key_value_range(text: &str, key: &str, expected_value: &str) -> Option<Range> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }

    let key_prefix = format!("{key}:");
    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" || line == "..." {
            break;
        }

        let trimmed = line.trim_start();
        if !trimmed.starts_with(&key_prefix) {
            continue;
        }

        let leading_width = line.len() - trimmed.len();
        let after_key = &trimmed[key_prefix.len()..];
        let value_start = after_key.find(expected_value)?;
        let col_start = leading_width + key_prefix.len() + after_key[..value_start].chars().count();
        return Some(range_for_span(line_index, col_start, expected_value.chars().count()));
    }

    None
}

fn frontmatter_key_ranges(text: &str) -> Vec<FrontmatterKeyRange> {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" || line == "..." {
            break;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with('-') || !is_frontmatter_key_line(trimmed) {
            continue;
        }
        let Some((key, _)) = trimmed.split_once(':') else {
            continue;
        };
        let leading_bytes = line.len() - trimmed.len();
        ranges.push(FrontmatterKeyRange {
            key: key.to_string(),
            range: range_for_span(line_index, line[..leading_bytes].chars().count(), key.chars().count()),
        });
    }

    ranges
}

fn frontmatter_tag_ranges(text: &str) -> Vec<FrontmatterTagRange> {
    frontmatter_sequence_value_ranges(text, "tags", true)
        .into_iter()
        .map(|value| FrontmatterTagRange {
            tag: value.value,
            range: value.range,
        })
        .collect()
}

fn frontmatter_alias_ranges(text: &str) -> Vec<FrontmatterValueRange> {
    frontmatter_sequence_value_ranges(text, "aliases", false)
}

fn frontmatter_sequence_value_ranges(text: &str, key: &str, trim_leading_hash: bool) -> Vec<FrontmatterValueRange> {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let mut in_value_block = false;
    let key_prefix = format!("{key}:");

    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" || line == "..." {
            break;
        }

        let trimmed = line.trim_start();
        let leading_bytes = line.len() - trimmed.len();

        if in_value_block {
            if trimmed.is_empty() {
                continue;
            }
            if is_frontmatter_key_line(trimmed) && !trimmed.starts_with('-') {
                in_value_block = false;
            } else if let Some(after_dash) = trimmed.strip_prefix('-') {
                let segment_start = leading_bytes + 1;
                ranges.extend(scan_frontmatter_value_tokens(
                    line,
                    line_index,
                    segment_start,
                    after_dash,
                    trim_leading_hash,
                ));
                continue;
            } else {
                continue;
            }
        }

        let Some(after_key) = trimmed.strip_prefix(&key_prefix) else {
            continue;
        };
        let segment_start = leading_bytes + key_prefix.len();
        let after_key_trimmed = after_key.trim_start();
        if after_key_trimmed.is_empty() {
            in_value_block = true;
        } else if after_key_trimmed.starts_with('[') {
            ranges.extend(scan_frontmatter_value_tokens(
                line,
                line_index,
                segment_start,
                after_key,
                trim_leading_hash,
            ));
        }
    }

    ranges
}

fn scan_frontmatter_value_tokens(
    line: &str,
    line_index: usize,
    segment_start: usize,
    segment: &str,
    trim_leading_hash: bool,
) -> Vec<FrontmatterValueRange> {
    let mut ranges = Vec::new();
    let mut index = 0;

    while index < segment.len() {
        let Some((offset, ch)) = segment[index..]
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace() && !matches!(ch, '[' | ']' | ','))
        else {
            break;
        };
        index += offset;

        let token_start = index;
        let (value_start, value_end, token_end) = if ch == '\'' || ch == '"' {
            let quote = ch;
            let content_start = token_start + quote.len_utf8();
            let mut end = content_start;
            let mut close_end = content_start;
            for (relative, candidate) in segment[content_start..].char_indices() {
                if candidate == quote {
                    end = content_start + relative;
                    close_end = end + quote.len_utf8();
                    break;
                }
            }
            if close_end == content_start {
                break;
            }
            (content_start, end, close_end)
        } else {
            let token_end = segment[token_start..]
                .char_indices()
                .find_map(|(relative, candidate)| matches!(candidate, ',' | ']').then_some(token_start + relative))
                .unwrap_or(segment.len());
            let (value_start, value_end) = trim_span(segment, token_start, token_end);
            (value_start, value_end, token_end)
        };

        if value_start < value_end {
            let value = if trim_leading_hash {
                segment[value_start..value_end].trim_start_matches('#').to_string()
            } else {
                segment[value_start..value_end].to_string()
            };
            if !value.is_empty() {
                let col_start = line[..segment_start + value_start].chars().count();
                ranges.push(FrontmatterValueRange {
                    value,
                    range: range_for_span(line_index, col_start, segment[value_start..value_end].chars().count()),
                });
            }
        }

        index = token_end;
    }

    ranges
}

fn trim_span(text: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end {
        let Some(ch) = text[start..end].chars().next() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        start += ch.len_utf8();
    }
    while start < end {
        let Some(ch) = text[start..end].chars().next_back() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        end -= ch.len_utf8();
    }
    (start, end)
}

fn is_frontmatter_key_line(trimmed_line: &str) -> bool {
    let Some((key, _)) = trimmed_line.split_once(':') else {
        return false;
    };
    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn find_alias_range(text: &str, alias: &str) -> Option<Range> {
    find_frontmatter_value_range(text, alias).or_else(|| find_title_or_heading_range(text, alias))
}

fn find_frontmatter_value_range(text: &str, value: &str) -> Option<Range> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }

    for (line_index, line) in text.lines().enumerate().skip(1) {
        if line == "---" {
            break;
        }
        if let Some(col_start) = line.find(value) {
            return Some(range_for_span(line_index, col_start, value.chars().count()));
        }
    }

    None
}

fn find_title_or_heading_range(text: &str, value: &str) -> Option<Range> {
    for (line_index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("title:") && !trimmed.starts_with("# ") {
            continue;
        }
        if let Some(col_start) = line.find(value) {
            return Some(range_for_span(line_index, col_start, value.chars().count()));
        }
    }

    None
}

fn make_diagnostic(range: Range, code: &str, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some("obsidian-lsp".to_string()),
        message,
        ..Default::default()
    }
}

fn build_document_link(source_uri: &Url, link: &LocatedLink) -> Option<DocumentLink> {
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

fn document_symbols_for_note(note: &Note, text: &str) -> Vec<DocumentSymbol> {
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

fn workspace_symbols_for_note(
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

fn push_workspace_symbol(symbols: &mut Vec<SymbolInformation>, query: &str, symbol: SymbolInformation) {
    if symbol_matches_query(&symbol.name, query) && symbols.len() < WORKSPACE_SYMBOL_LIMIT {
        symbols.push(symbol);
    }
}

fn symbol_matches_query(name: &str, query: &str) -> bool {
    query.is_empty() || name.to_lowercase().contains(query)
}

fn symbol_tag_ranges(note: &Note, text: &str) -> Vec<FrontmatterValueRange> {
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

fn symbol_link_name(link: &Link) -> String {
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

fn symbol_range_order(left: &DocumentSymbol, right: &DocumentSymbol) -> std::cmp::Ordering {
    left.range
        .start
        .line
        .cmp(&right.range.start.line)
        .then(left.range.start.character.cmp(&right.range.start.character))
        .then(left.name.cmp(&right.name))
}

#[allow(deprecated)]
fn document_symbol(name: String, detail: Option<String>, kind: SymbolKind, range: Range) -> DocumentSymbol {
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

#[allow(deprecated)]
fn workspace_symbol(
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

fn parse_document_link_data(value: &Value) -> Result<DocumentLinkData, StateError> {
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

fn synthetic_note_for_raw_link(source_path: &Path, raw_link: &str) -> Result<Note, StateError> {
    let note = Note::parse(source_path, raw_link);
    if note.links.is_empty() {
        return Err(StateError::InvalidDocumentLinkData(
            "raw link did not parse into a wiki or markdown link".to_string(),
        ));
    }

    Ok(note)
}

fn render_link_text(link: &Link) -> Option<String> {
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

fn direct_document_link_target(
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

fn resolve_local_file_target_path(source_path: &Path, url: &str, vault_path: &Path) -> Option<PathBuf> {
    let path = markdown_url_path(url)?;
    if path.ends_with(".md") {
        return None;
    }

    local_markdown_candidates(source_path, &path, vault_path)
        .into_iter()
        .find(|candidate| candidate.exists())
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
        let vault = snapshot.build_vault()?;
        let notes = snapshot.notes(&vault);
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
            let targets = resolve_link_targets(&path, &located_link.link, &notes, vault.path());
            match targets.as_slice() {
                [] => {
                    if let Some(action) = create_note_code_action(&path, located_link, vault.path(), &diagnostics)? {
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

fn duplicate_diagnostic_actions(
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

fn duplicate_diagnostics_for_request(
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

fn duplicate_alias_ranges_for_note(snapshot: &StateSnapshot, note: &Note) -> Result<Vec<(String, Range)>, StateError> {
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

fn push_unique_diagnostic(diagnostics: &mut Vec<Diagnostic>, diagnostic: Diagnostic) {
    if diagnostics
        .iter()
        .any(|existing| existing.code == diagnostic.code && existing.range == diagnostic.range)
    {
        return;
    }
    diagnostics.push(diagnostic);
}

fn diagnostics_path_is_ignored(snapshot: &StateSnapshot, ignore_set: &globset::GlobSet, path: &Path) -> bool {
    let rel = path.strip_prefix(snapshot.vault_path.as_path()).unwrap_or(path);
    ignore_set.is_match(rel)
}

fn assign_unique_note_id_code_action(
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

fn change_duplicate_alias_code_actions(
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
struct DuplicateAliasEditTarget {
    alias: String,
    range: Range,
    removal_range: Option<Range>,
}

fn duplicate_alias_edit_target(
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

fn alias_at_range(note: &Note, text: &str, range: Range) -> Option<String> {
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

fn block_list_item_removal_range(text: &str, value_range: Range) -> Option<Range> {
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

fn create_note_code_action(
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

fn convert_link_code_action(
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

fn add_missing_heading_code_action(
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

impl PrepareRenameRequest {
    pub fn compute(self) -> Result<Option<PrepareRenameResponse>, StateError> {
        let source_note = self.snapshot.note_for_path(&self.path)?;
        if find_link_at_position(&source_note, self.position).is_none()
            && let Some(selected_tag) = find_tag_at_position(&self.snapshot, &source_note, self.position)?
        {
            return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                range: selected_tag.rename_range,
                placeholder: selected_tag.placeholder,
            }));
        }

        let Some(target) = rename_target(&self.snapshot, &self.path, self.position)? else {
            return Ok(None);
        };

        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: target.range,
            placeholder: target.placeholder,
        }))
    }
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

fn rename_target(
    snapshot: &StateSnapshot,
    path: &Path,
    position: Position,
) -> Result<Option<RenameTarget>, StateError> {
    let source_note = snapshot.note_for_path(path)?;

    if let Some(selected_link) = find_link_at_position(&source_note, position) {
        let vault = snapshot.build_vault()?;
        let notes = snapshot.notes(&vault);
        let matching_notes = resolve_link_targets(&source_note.path, &selected_link.link, &notes, vault.path());
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

fn rename_workspace_edit(snapshot: &StateSnapshot, note: &Note, new_name: &str) -> Result<WorkspaceEdit, StateError> {
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

fn tag_rename_workspace_edit(
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

fn workspace_edit_from_text_edits(
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

fn normalize_tag_rename_target(new_name: &str) -> Result<String, StateError> {
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

fn new_note_title_from_link(link: &Link) -> Option<String> {
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

fn yaml_scalar(value: &str) -> String {
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

fn assign_note_id_workspace_edit(
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

fn note_id_text_edit(text: &str, note: &Note, new_id: &str) -> TextEdit {
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

fn unique_note_id(note: &Note, notes: &[Note]) -> String {
    let used = notes.iter().map(|note| note.id.clone()).collect::<HashSet<_>>();
    unique_suffixed_name(&note_file_stem(note), |candidate| used.contains(candidate))
}

fn unique_note_alias(alias: &str, notes: &[Note]) -> String {
    let used = notes
        .iter()
        .flat_map(|note| note.aliases.iter())
        .map(|alias| alias.to_lowercase())
        .collect::<HashSet<_>>();
    unique_suffixed_name(alias, |candidate| used.contains(&candidate.to_lowercase()))
}

fn unique_suffixed_name(base: &str, is_used: impl Fn(&str) -> bool) -> String {
    let base = if base.trim().is_empty() { "note" } else { base.trim() };
    let mut suffix = 2;
    let mut candidate = base.to_string();
    while is_used(&candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    candidate
}

fn diagnostic_code_is(diagnostic: &Diagnostic, expected: &str) -> bool {
    matches!(
        diagnostic.code.as_ref(),
        Some(NumberOrString::String(code)) if code == expected
    )
}

fn diagnostic_backtick_value(diagnostic: &Diagnostic) -> Option<String> {
    let (_, after_open) = diagnostic.message.split_once('`')?;
    let (value, _) = after_open.split_once('`')?;
    Some(value.to_string())
}

fn matching_diagnostics(diagnostics: &[Diagnostic], code: &str, range: Range) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic_code_is(diagnostic, code))
        .filter(|diagnostic| ranges_intersect(diagnostic.range, range))
        .cloned()
        .collect()
}

fn diagnostic_applies_to_request(diagnostic: &Diagnostic, range: Range, position: Position) -> bool {
    ranges_intersect(diagnostic.range, range) || position_in_or_at_range(position, &diagnostic.range)
}

fn diagnostic_applies_to_request_range(diagnostic_range: Range, range: Range, position: Position) -> bool {
    ranges_intersect(diagnostic_range, range) || position_in_or_at_range(position, &diagnostic_range)
}

fn markdown_link_text(source_path: &Path, link: &Link, target: &Note) -> String {
    let Link::Wiki { heading, alias, .. } = link else {
        unreachable!("markdown_link_text should only be called for wiki links");
    };
    let display_text = alias
        .as_deref()
        .or(target.title.as_deref())
        .unwrap_or(target.id.as_str());
    let mut url = markdown_url_for_target(source_path, &target.path);
    if let Some(heading) = heading.as_deref().filter(|heading| !heading.is_empty()) {
        url.push('#');
        url.push_str(&percent_encode_url_component(heading));
    }
    format!("[{display_text}]({url})")
}

fn wiki_link_text(snapshot: &StateSnapshot, link: &Link, target: &Note) -> Result<String, StateError> {
    let Link::Markdown { text, .. } = link else {
        unreachable!("wiki_link_text should only be called for markdown links");
    };
    let target_name = note_file_stem(target);
    let mut wiki = format!("[[{target_name}");
    if let Some(fragment) = link_fragment(link) {
        let target_text = snapshot.text_for_path(&target.path)?;
        let heading = resolve_heading_fragment_text(&target_text, &fragment).unwrap_or(fragment);
        if !heading.is_empty() {
            wiki.push('#');
            wiki.push_str(&heading);
        }
    }
    let alias = text.trim();
    if !alias.is_empty() && alias != target_name {
        wiki.push('|');
        wiki.push_str(alias);
    }
    wiki.push_str("]]");
    Ok(wiki)
}

fn markdown_url_for_target(source_path: &Path, target_path: &Path) -> String {
    let source_dir = source_path.parent().unwrap_or(source_path);
    let relative = relative_path_from(source_dir, target_path);
    percent_encode_url_path(&path_to_slash(&relative))
}

fn relative_path_from(from_dir: &Path, target_path: &Path) -> PathBuf {
    if let Ok(stripped) = target_path.strip_prefix(from_dir)
        && !stripped.as_os_str().is_empty()
    {
        return stripped.to_path_buf();
    }

    let from_components = from_dir.components().collect::<Vec<_>>();
    let target_components = target_path.components().collect::<Vec<_>>();
    let mut common_len = 0;
    while common_len < from_components.len()
        && common_len < target_components.len()
        && from_components[common_len] == target_components[common_len]
    {
        common_len += 1;
    }

    let mut relative = PathBuf::new();
    for component in &from_components[common_len..] {
        if matches!(component, std::path::Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in &target_components[common_len..] {
        relative.push(component.as_os_str());
    }
    relative
}

fn path_to_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_url_path(path: &str) -> String {
    percent_encode_with(path, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_' | b'~')
    })
}

fn percent_encode_url_component(component: &str) -> String {
    percent_encode_with(component, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~')
    })
}

fn percent_encode_with(input: &str, allow: impl Fn(u8) -> bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in input.as_bytes() {
        if allow(*byte) {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

fn resolve_heading_fragment_text(text: &str, fragment: &str) -> Option<String> {
    let expected_segments = parse_heading_fragment_segments(fragment);
    if expected_segments.is_empty() {
        return None;
    }

    let mut seen_anchors = HashMap::new();
    let mut current_path = Vec::new();

    for line in text.lines() {
        let Some((level, _, heading_text)) = heading_line_parts(line) else {
            continue;
        };
        let Some(resolved_anchor) = resolve_heading_anchor(heading_text, &mut seen_anchors) else {
            continue;
        };

        current_path.truncate(level.saturating_sub(1));
        current_path.push(HeadingPathSegment {
            text: heading_text,
            normalized_anchor: normalize_heading_anchor(heading_text),
            resolved_anchor,
        });

        if heading_path_matches(&current_path, &expected_segments) {
            return Some(
                current_path[current_path.len() - expected_segments.len()..]
                    .iter()
                    .map(|segment| segment.text)
                    .collect::<Vec<_>>()
                    .join("#"),
            );
        }
    }

    None
}

fn append_heading_text(text: &str, heading: &str) -> String {
    let prefix = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{prefix}## {heading}\n")
}

fn compute_new_note_path(source_path: &Path, vault_path: &Path, link: &Link) -> Option<PathBuf> {
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

fn rename_target_path(vault_path: &Path, note_path: &Path, new_name: &str) -> Result<PathBuf, StateError> {
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

pub(crate) fn normalize_new_note_path(vault_path: &Path, candidate: impl AsRef<Path>) -> Option<PathBuf> {
    let vault_path = obsidian_core::common::normalize_path(vault_path, None);
    let path = obsidian_core::common::normalize_path(candidate, Some(&vault_path));
    if !path.starts_with(&vault_path) || path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return None;
    }
    Some(path)
}

fn build_ignore_set(patterns: &[String]) -> globset::GlobSet {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        if let Ok(glob) = globset::GlobBuilder::new(pattern).case_insensitive(false).build() {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_default()
}

fn resolve_link_targets<'a>(source_path: &Path, link: &Link, notes: &'a [Note], vault_path: &Path) -> Vec<&'a Note> {
    match link {
        Link::Wiki { target, .. } => {
            if target.is_empty() {
                return Vec::new();
            }

            notes
                .iter()
                .filter(|note| note_matches_wiki_target(note, target))
                .collect()
        }
        Link::Markdown { url, .. } => {
            let Some(url_path) = markdown_url_path(url) else {
                return Vec::new();
            };
            if !url_path.ends_with(".md") {
                return Vec::new();
            }

            let candidates = local_markdown_candidates(source_path, &url_path, vault_path);
            notes.iter().filter(|note| candidates.contains(&note.path)).collect()
        }
        Link::Embed { .. } => Vec::new(),
    }
}

fn note_matches_wiki_target(note: &Note, target: &str) -> bool {
    note.id == target
        || note
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem == target)
        || note.aliases.iter().any(|alias| alias == target)
}

fn local_markdown_candidates(source_path: &Path, url_path: &str, vault_path: &Path) -> Vec<PathBuf> {
    let source_dir = source_path.parent().unwrap_or(source_path);
    let mut candidates = vec![
        obsidian_core::common::normalize_path(source_dir.join(url_path), Some(vault_path)),
        obsidian_core::common::normalize_path(url_path, Some(vault_path)),
    ];
    candidates.dedup();
    candidates
}

fn markdown_url_path(url: &str) -> Option<String> {
    if url.contains("://") || url.starts_with('/') {
        return None;
    }

    let url_path_raw = match url.find('#') {
        Some(index) => &url[..index],
        None => url,
    };

    Some(percent_decode(url_path_raw))
}

fn link_fragment(link: &Link) -> Option<String> {
    match link {
        Link::Wiki { heading, .. } => heading.clone(),
        Link::Markdown { url, .. } => url
            .split_once('#')
            .map(|(_, fragment)| percent_decode(fragment))
            .filter(|fragment| !fragment.is_empty()),
        Link::Embed { .. } => None,
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            output.push((high * 16 + low) as u8);
            index += 3;
            continue;
        }

        output.push(bytes[index]);
        index += 1;
    }

    String::from_utf8(output).unwrap_or_else(|_| input.to_string())
}

fn selected_link_fragment(link: Option<&LocatedLink>) -> Option<String> {
    link.and_then(|link| link_fragment(&link.link))
}

fn note_target_uri(note: &Note, link: &Link) -> Result<Url, StateError> {
    let mut uri = path_to_uri(&note.path)?;
    if let Some(fragment) = link_fragment(link) {
        uri.set_fragment(Some(&fragment));
    }
    Ok(uri)
}

fn note_location(snapshot: &StateSnapshot, note: &Note, fragment: Option<String>) -> Result<Location, StateError> {
    let mut uri = path_to_uri(&note.path)?;
    if let Some(fragment) = fragment.as_deref() {
        uri.set_fragment(Some(fragment));
    }

    Ok(Location {
        uri,
        range: note_definition_range(snapshot, note, fragment.as_deref())?,
    })
}

fn note_definition_range(snapshot: &StateSnapshot, note: &Note, fragment: Option<&str>) -> Result<Range, StateError> {
    let text = snapshot.text_for_path(&note.path)?;

    if let Some(fragment) = fragment
        && let Some(range) = find_heading_range(&text, fragment)
    {
        return Ok(range);
    }

    if let Some(title) = note.title.as_deref()
        && let Some(range) = find_title_or_heading_range(&text, title)
    {
        return Ok(range);
    }

    if let Some(range) = find_frontmatter_key_range(&text, "id") {
        return Ok(range);
    }

    Ok(document_start_range())
}

struct HeadingFragmentSegment<'a> {
    raw: &'a str,
    normalized: String,
}

struct HeadingPathSegment<'a> {
    text: &'a str,
    normalized_anchor: String,
    resolved_anchor: String,
}

fn find_heading_range(text: &str, heading: &str) -> Option<Range> {
    let expected_segments = parse_heading_fragment_segments(heading);
    if expected_segments.is_empty() {
        return None;
    }

    let mut seen_anchors = HashMap::new();
    let mut current_path = Vec::new();

    for (line_index, line) in text.lines().enumerate() {
        let Some((level, col_start, heading_text)) = heading_line_parts(line) else {
            continue;
        };
        let Some(resolved_anchor) = resolve_heading_anchor(heading_text, &mut seen_anchors) else {
            continue;
        };

        current_path.truncate(level.saturating_sub(1));
        current_path.push(HeadingPathSegment {
            text: heading_text,
            normalized_anchor: normalize_heading_anchor(heading_text),
            resolved_anchor,
        });

        if heading_path_matches(&current_path, &expected_segments) {
            return Some(range_for_span(line_index, col_start, heading_text.chars().count()));
        }
    }

    None
}

fn parse_heading_fragment_segments(heading: &str) -> Vec<HeadingFragmentSegment<'_>> {
    heading
        .split('#')
        .filter(|segment| !segment.is_empty())
        .map(|segment| HeadingFragmentSegment {
            raw: segment,
            normalized: normalize_heading_anchor(segment),
        })
        .collect()
}

fn heading_path_matches(path: &[HeadingPathSegment<'_>], expected: &[HeadingFragmentSegment<'_>]) -> bool {
    if expected.len() > path.len() {
        return false;
    }

    path[path.len() - expected.len()..]
        .iter()
        .zip(expected.iter())
        .all(|(candidate, expected_segment)| heading_segment_matches(candidate, expected_segment))
}

fn heading_segment_matches(candidate: &HeadingPathSegment<'_>, expected: &HeadingFragmentSegment<'_>) -> bool {
    candidate.text == expected.raw
        || candidate.normalized_anchor == expected.normalized
        || candidate.resolved_anchor == expected.normalized
}

fn heading_line_parts(line: &str) -> Option<(usize, usize, &str)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }

    let marker_bytes = trimmed
        .char_indices()
        .take_while(|(_, ch)| *ch == '#')
        .last()
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let level = trimmed[..marker_bytes].chars().count();
    let after_markers = &trimmed[marker_bytes..];
    let content = after_markers.trim_start();
    if content.is_empty() {
        return None;
    }

    let heading_text = strip_optional_heading_closing_hashes(content);
    if heading_text.is_empty() {
        return None;
    }

    let leading_bytes = line.len() - trimmed.len();
    let whitespace_bytes = after_markers.len() - content.len();
    let heading_start = leading_bytes + marker_bytes + whitespace_bytes;

    Some((level, line[..heading_start].chars().count(), heading_text))
}

fn strip_optional_heading_closing_hashes(text: &str) -> &str {
    let trimmed = text.trim_end();
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() == trimmed.len() || !without_hashes.chars().last().is_some_and(char::is_whitespace) {
        return trimmed;
    }

    without_hashes.trim_end()
}

fn resolve_heading_anchor(heading_text: &str, seen_anchors: &mut HashMap<String, usize>) -> Option<String> {
    let base_anchor = normalize_heading_anchor(heading_text);
    if base_anchor.is_empty() {
        return None;
    }

    let seen_count = seen_anchors.entry(base_anchor.clone()).or_default();
    let anchor = if *seen_count == 0 {
        base_anchor
    } else {
        format!("{base_anchor}-{seen_count}")
    };
    *seen_count += 1;

    Some(anchor)
}

fn normalize_heading_anchor(text: &str) -> String {
    let mut anchor = String::new();
    let mut last_was_separator = true;

    for ch in text.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '_' {
            anchor.push(ch);
            last_was_separator = false;
        } else if (ch.is_whitespace() || ch == '-') && !last_was_separator && !anchor.is_empty() {
            anchor.push('-');
            last_was_separator = true;
        }
    }

    while anchor.ends_with('-') {
        anchor.pop();
    }

    anchor
}

fn find_link_at_position(note: &Note, position: Position) -> Option<&LocatedLink> {
    note.links
        .iter()
        .filter(|link| !matches!(link.link, Link::Embed { .. }))
        .find(|link| position_in_location(position, &link.location))
}

fn note_file_stem(note: &Note) -> String {
    note.path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(note.id.as_str())
        .to_string()
}

fn rename_link_target_range(link: &LocatedLink) -> Option<Range> {
    let line = (link.location.line.saturating_sub(1)) as u32;
    match &link.link {
        Link::Wiki { target, .. } => {
            if target.is_empty() {
                return None;
            }
            let start = link.location.col_start + 2;
            Some(Range::new(
                Position::new(line, start as u32),
                Position::new(line, (start + target.chars().count()) as u32),
            ))
        }
        Link::Markdown { text, url } => {
            let url_start = link.location.col_start + 1 + text.chars().count() + 2;
            let path_end = url.find('#').unwrap_or(url.len());
            let raw_path = &url[..path_end];
            if raw_path.is_empty() {
                return None;
            }

            let stem_start = match (raw_path.rfind('/'), raw_path.rfind('\\')) {
                (Some(left), Some(right)) => left.max(right) + 1,
                (Some(index), None) | (None, Some(index)) => index + 1,
                (None, None) => 0,
            };
            let stem_end = raw_path
                .strip_suffix(".md")
                .map(|without_ext| without_ext.len())
                .unwrap_or(raw_path.len());
            if stem_start >= stem_end {
                return None;
            }

            let start = url_start + raw_path[..stem_start].chars().count();
            let end = url_start + raw_path[..stem_end].chars().count();
            Some(Range::new(
                Position::new(line, start as u32),
                Position::new(line, end as u32),
            ))
        }
        Link::Embed { .. } => None,
    }
}

fn render_document_link_tooltip(prefix: Option<&str>, detail: Option<String>) -> String {
    match (prefix, detail) {
        (Some(prefix), Some(detail)) => format!("{prefix}: {detail}"),
        (Some(prefix), None) => prefix.to_string(),
        (None, Some(detail)) => detail,
        (None, None) => "Obsidian link".to_string(),
    }
}

fn render_ambiguous_document_link_tooltip(notes: &[&Note], vault_path: &Path) -> String {
    format!(
        "Ambiguous note link: {}",
        notes
            .iter()
            .map(|note| relative_display(vault_path, &note.path))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn render_note_link_label(note: &Note, vault_path: &Path) -> String {
    let title = note.title.as_deref().unwrap_or(note.id.as_str());
    format!("{title} ({})", relative_display(vault_path, &note.path))
}

fn render_note_hover(note: &Note, vault_path: &Path) -> String {
    let mut lines = Vec::new();
    let heading = note.title.as_deref().unwrap_or(note.id.as_str());
    lines.push(format!("**{}**", heading));
    lines.push(String::new());
    lines.push(format!("- Path: `{}`", relative_display(vault_path, &note.path)));
    lines.push(format!("- ID: `{}`", note.id));

    if !note.aliases.is_empty() {
        lines.push(format!("- Aliases: {}", markdown_list(note.aliases.iter().cloned())));
    }

    let tags = note
        .tags
        .iter()
        .map(|tag| tag.tag.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !tags.is_empty() {
        lines.push(format!("- Tags: {}", markdown_list(tags)));
    }

    lines.join("\n")
}

fn render_tag_hover(tag: &str, occurrence_count: usize) -> String {
    format!("**#{}**\n\n- Occurrences: {}", tag, occurrence_count)
}

fn render_ambiguous_hover(notes: &[&Note], vault_path: &Path) -> String {
    let mut lines = vec![
        "**Ambiguous note link**".to_string(),
        String::new(),
        "This link matches more than one note:".to_string(),
        String::new(),
    ];

    for note in notes {
        lines.push(format!("- `{}`", relative_display(vault_path, &note.path)));
    }

    lines.join("\n")
}

fn markdown_list(items: impl IntoIterator<Item = String>) -> String {
    items
        .into_iter()
        .map(|item| format!("`{}`", item))
        .collect::<Vec<_>>()
        .join(", ")
}

fn relative_display(vault_path: &Path, path: &Path) -> String {
    path.strip_prefix(vault_path).unwrap_or(path).display().to_string()
}

fn position_in_location(position: Position, location: &InlineLocation) -> bool {
    let line = position.line + 1;
    let character = position.character;

    line == location.line as u32 && character >= location.col_start as u32 && character < location.col_end as u32
}

fn position_in_range(position: Position, range: &Range) -> bool {
    (position.line > range.start.line
        || (position.line == range.start.line && position.character >= range.start.character))
        && (position.line < range.end.line
            || (position.line == range.end.line && position.character < range.end.character))
}

fn position_in_or_at_range(position: Position, range: &Range) -> bool {
    if range.start == range.end {
        position == range.start
    } else {
        position_in_range(position, range)
    }
}

fn ranges_intersect(left: Range, right: Range) -> bool {
    if left.start == left.end {
        return position_in_or_at_range(left.start, &right);
    }
    if right.start == right.end {
        return position_in_or_at_range(right.start, &left);
    }

    position_before(left.start, right.end) && position_before(right.start, left.end)
}

fn position_before(left: Position, right: Position) -> bool {
    left.line < right.line || (left.line == right.line && left.character < right.character)
}

fn location_to_range(location: &InlineLocation) -> Range {
    Range::new(
        Position::new((location.line.saturating_sub(1)) as u32, location.col_start as u32),
        Position::new((location.line.saturating_sub(1)) as u32, location.col_end as u32),
    )
}

fn line_start_range(line: usize) -> Range {
    Range::new(
        Position::new((line.saturating_sub(1)) as u32, 0),
        Position::new((line.saturating_sub(1)) as u32, 0),
    )
}

fn document_start_range() -> Range {
    Range::new(Position::new(0, 0), Position::new(0, 0))
}

fn document_end_range(text: &str) -> Range {
    let mut line = 0;
    let mut character = 0;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }
    let position = Position::new(line, character);
    Range::new(position, position)
}

fn range_for_span(line: usize, col_start: usize, width: usize) -> Range {
    Range::new(
        Position::new(line as u32, col_start as u32),
        Position::new(line as u32, (col_start + width) as u32),
    )
}

fn detect_link_context(line_prefix: &str) -> Option<LinkContext> {
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
                link_start_char: start,
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
                link_start_char: i,
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
                tag_start_char: i,
            });
        }
        break;
    }

    None
}

fn note_matches_query(note: &Note, query: &str) -> bool {
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

fn wiki_completions_for_note(note: &Note, prefix_range: Range) -> Vec<CompletionItem> {
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

fn markdown_completions_for_note(note: &Note, vault_path: &Path, prefix_range: Range) -> Vec<CompletionItem> {
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

fn anchor_completions(text: &str, heading_query: &str, prefix_range: Range) -> Vec<CompletionItem> {
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

fn tag_completions(tags: &[String], query: &str, prefix_range: Range) -> Vec<CompletionItem> {
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

fn closing_bracket_len(text_after_cursor: &str, context: &LinkContext) -> usize {
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

fn parse_headings(text: &str) -> Vec<String> {
    parse_heading_symbols(text)
        .into_iter()
        .map(|heading| heading.name)
        .collect()
}

fn parse_heading_symbols(text: &str) -> Vec<HeadingSymbol> {
    let mut in_frontmatter = false;
    let mut headings = Vec::new();

    for (i, line) in text.lines().enumerate() {
        if i == 0 && line == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if line == "---" || line == "..." {
                in_frontmatter = false;
            }
            continue;
        }
        if let Some((_, col_start, heading_text)) = heading_line_parts(line) {
            headings.push(HeadingSymbol {
                name: heading_text.to_string(),
                range: range_for_span(i, col_start, heading_text.chars().count()),
            });
        }
    }

    headings
}

fn heading_completions_for_note(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uri::path_to_uri;

    fn open_state() -> (tempfile::TempDir, BackendState, PathBuf, Url) {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let note_path = vault_dir.path().join("notes/today.md");
        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "disk body").unwrap();
        let note_path = note_path.canonicalize().unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let state = BackendState::new(vault);
        let uri = path_to_uri(&note_path).unwrap();

        (vault_dir, state, note_path, uri)
    }

    fn note_body(state: &BackendState, note_path: &Path) -> Option<String> {
        state
            .vault
            .notes_filtered_with_content(|candidate| candidate == note_path)
            .into_iter()
            .find_map(Result::ok)
            .and_then(|note| note.body)
    }

    fn update_for_uri<'a>(batch: &'a DiagnosticsBatch, uri: &Url) -> &'a DiagnosticUpdate {
        batch
            .updates
            .iter()
            .find(|update| update.uri == *uri)
            .expect("batch should contain a diagnostics update for the requested URI")
    }

    fn codes(update: &DiagnosticUpdate) -> Vec<&str> {
        update
            .diagnostics
            .iter()
            .filter_map(|diagnostic| diagnostic.code.as_ref())
            .filter_map(|code| match code {
                NumberOrString::String(value) => Some(value.as_str()),
                NumberOrString::Number(_) => None,
            })
            .collect()
    }

    fn position_for_substring(text: &str, needle: &str) -> Position {
        for (line_index, line) in text.lines().enumerate() {
            if let Some(col_start) = line.find(needle) {
                return Position::new(line_index as u32, col_start as u32 + 2);
            }
        }

        panic!("substring '{needle}' not found");
    }

    #[test]
    fn new_note_content_includes_primary_alias_and_heading_when_title_is_available() {
        assert_eq!(
            new_note_content("foo", Some("Foo")),
            "---\nid: foo\naliases:\n- Foo\n---\n\n# Foo\n"
        );
    }

    #[test]
    fn new_note_content_preserves_existing_minimal_template_without_title() {
        assert_eq!(new_note_content("foo", None), "---\nid: foo\n---\n");
    }

    #[test]
    fn open_document_loads_note_into_vault_state() {
        let (_vault_dir, mut state, note_path, uri) = open_state();

        let request = state.open_document(uri.clone(), 1, "buffer body".to_string()).unwrap();
        let batch = request.compute().unwrap();
        let update = update_for_uri(&batch, &uri);

        assert_eq!(update.uri, uri);
        assert_eq!(update.version, Some(1));
        assert!(update.diagnostics.is_empty());
        assert!(state.vault.note_is_loaded(&note_path));
        assert_eq!(state.open_documents.get(&note_path).unwrap().text, "buffer body");
        assert_eq!(note_body(&state, &note_path).as_deref(), Some("buffer body"));
    }

    #[test]
    fn change_document_updates_in_memory_note_body() {
        let (_vault_dir, mut state, note_path, uri) = open_state();
        state.open_document(uri.clone(), 1, "buffer body".to_string()).unwrap();
        let note_uri = uri.clone();

        let request = state
            .change_document(
                uri,
                2,
                &[TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "changed body".to_string(),
                }],
            )
            .unwrap();
        let batch = request.compute().unwrap();

        assert_eq!(state.open_documents.get(&note_path).unwrap().version, 2);
        assert_eq!(state.open_documents.get(&note_path).unwrap().text, "changed body");
        assert_eq!(note_body(&state, &note_path).as_deref(), Some("changed body"));
        assert_eq!(update_for_uri(&batch, &note_uri).version, Some(2));
    }

    #[test]
    fn close_document_unloads_the_in_memory_override() {
        let (_vault_dir, mut state, note_path, uri) = open_state();
        state.open_document(uri.clone(), 1, "buffer body".to_string()).unwrap();

        let request = state.close_document(uri.clone()).unwrap();
        let batch = request.compute().unwrap();
        let update = update_for_uri(&batch, &uri);

        assert!(update.diagnostics.is_empty());
        assert!(!state.vault.note_is_loaded(&note_path));
        assert!(!state.open_documents.contains_key(&note_path));
        assert_eq!(note_body(&state, &note_path).as_deref(), Some("disk body"));
    }

    #[test]
    fn open_document_rejects_paths_outside_the_vault() {
        let (_vault_dir, mut state, _note_path, _uri) = open_state();
        let external_path = tempfile::tempdir().unwrap().path().join("external.md");
        let external_uri = path_to_uri(&external_path).unwrap();

        let error = state
            .open_document(external_uri, 1, "buffer body".to_string())
            .unwrap_err();

        assert!(matches!(error, StateError::Uri(UriError::PathOutsideVault { .. })));
    }

    #[test]
    fn change_document_requires_full_document_content() {
        let (_vault_dir, mut state, _note_path, uri) = open_state();

        let error = state.change_document(uri.clone(), 2, &[]).unwrap_err();

        assert!(matches!(
            error,
            StateError::MissingDocumentContent { uri: actual } if actual == uri
        ));
    }

    #[test]
    fn open_document_reports_health_diagnostics_for_all_affected_notes() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let note_a_path = vault_dir.path().join("note-a.md");
        let note_a_text = "---\nid: shared-id\naliases: [shared-alias]\n---\n\nSee [[missing-note]].\n";
        fs::write(&note_a_path, note_a_text).unwrap();
        let note_a_path = note_a_path.canonicalize().unwrap();
        let note_a_uri = path_to_uri(&note_a_path).unwrap();

        let note_b_path = vault_dir.path().join("note-b.md");
        fs::write(
            &note_b_path,
            "---\nid: shared-id\naliases: [shared-alias]\n---\n\nBody.\n",
        )
        .unwrap();
        let note_b_path = note_b_path.canonicalize().unwrap();
        let note_b_uri = path_to_uri(&note_b_path).unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        let batch = state
            .open_document(note_a_uri.clone(), 1, note_a_text.to_string())
            .unwrap()
            .compute()
            .unwrap();

        let note_a_update = update_for_uri(&batch, &note_a_uri);
        assert_eq!(note_a_update.version, Some(1));
        assert_eq!(
            codes(note_a_update),
            vec!["duplicate-id", "duplicate-alias", "broken-link"]
        );
        assert!(
            note_a_update
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("Broken link [[missing-note]]"))
        );
        assert!(
            note_a_update
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.range.start.line == 5 && diagnostic.range.start.character == 4)
        );

        let note_b_update = update_for_uri(&batch, &note_b_uri);
        assert_eq!(note_b_update.version, None);
        assert_eq!(codes(note_b_update), vec!["duplicate-id", "duplicate-alias"]);
    }

    #[test]
    fn duplicate_alias_diagnostics_point_to_frontmatter_alias_tokens() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let note_a_path = vault_dir.path().join("note-a.md");
        let note_a_text = "---\nid: note-a\naliases: [Shared Alias, Other]\n---\n\nBody.\n";
        fs::write(&note_a_path, note_a_text).unwrap();
        let note_a_uri = path_to_uri(&note_a_path.canonicalize().unwrap()).unwrap();

        let note_b_path = vault_dir.path().join("note-b.md");
        fs::write(
            &note_b_path,
            "---\nid: note-b\naliases:\n- shared alias\n---\n\nBody.\n",
        )
        .unwrap();
        let note_b_uri = path_to_uri(&note_b_path.canonicalize().unwrap()).unwrap();

        let mut state = BackendState::new(Vault::open(vault_dir.path()).unwrap());
        let batch = state
            .open_document(note_a_uri.clone(), 1, note_a_text.to_string())
            .unwrap()
            .compute()
            .unwrap();

        let note_a_diagnostic = update_for_uri(&batch, &note_a_uri)
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic_code_is(diagnostic, "duplicate-alias"))
            .expect("note A should have duplicate alias diagnostic");
        assert_eq!(note_a_diagnostic.range.start.line, 2);
        assert_eq!(note_a_diagnostic.range.start.character, 10);

        let note_b_diagnostic = update_for_uri(&batch, &note_b_uri)
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic_code_is(diagnostic, "duplicate-alias"))
            .expect("note B should have duplicate alias diagnostic");
        assert_eq!(note_b_diagnostic.range.start.line, 3);
        assert_eq!(note_b_diagnostic.range.start.character, 2);
    }

    fn navigation_state() -> (tempfile::TempDir, BackendState, Url, Url, Url, String) {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("target.md");
        fs::write(
            &target_path,
            "---\nid: target-id\ntitle: Target Note\naliases: [target-alias]\ntags: [rust]\n---\n\nBody.\n",
        )
        .unwrap();
        let target_uri = path_to_uri(&target_path.canonicalize().unwrap()).unwrap();

        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "placeholder").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        let source_text = "See [[target-id]] and [Target Markdown](target.md).";

        let backlink_path = vault_dir.path().join("backlink.md");
        fs::write(&backlink_path, "Another reference to [[target-id]].").unwrap();
        let backlink_uri = path_to_uri(&backlink_path.canonicalize().unwrap()).unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        (
            vault_dir,
            state,
            source_uri,
            target_uri,
            backlink_uri,
            source_text.to_string(),
        )
    }

    fn heading_anchor_navigation_state() -> (tempfile::TempDir, BackendState, Url, Url, String) {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("target.md");
        fs::write(
            &target_path,
            "---\nid: target-id\ntitle: Target Note\n---\n\n# Overview\n\n## Linked Heading\nBody.\n",
        )
        .unwrap();
        let target_uri = path_to_uri(&target_path.canonicalize().unwrap()).unwrap();

        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "placeholder").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        let source_text = "See [[target-id#Linked Heading]] and [Target Markdown](target.md#linked-heading).";

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        (vault_dir, state, source_uri, target_uri, source_text.to_string())
    }

    fn nested_heading_anchor_navigation_state() -> (tempfile::TempDir, BackendState, Url, Url, String) {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("target.md");
        fs::write(
            &target_path,
            concat!(
                "---\n",
                "id: target-id\n",
                "title: Target Note\n",
                "---\n",
                "\n",
                "# Other Heading\n",
                "\n",
                "## Subheading B\n",
                "\n",
                "# Heading A\n",
                "\n",
                "## Subheading B\n",
                "Body.\n",
            ),
        )
        .unwrap();
        let target_uri = path_to_uri(&target_path.canonicalize().unwrap()).unwrap();

        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "placeholder").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        let source_text = concat!(
            "See [[target-id#Heading A#Subheading B]], ",
            "[[target-id#heading-a#subheading-b]], ",
            "and [Target Markdown](target.md#heading-a#subheading-b).\n"
        );

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        (vault_dir, state, source_uri, target_uri, source_text.to_string())
    }

    fn symbol_state() -> (tempfile::TempDir, BackendState, Url, Url) {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let source_path = vault_dir.path().join("source.md");
        let source_text = concat!(
            "---\n",
            "id: symbol-note\n",
            "title: Symbol Note\n",
            "aliases:\n",
            "- Work Alias\n",
            "tags: [rust, lsp]\n",
            "---\n",
            "\n",
            "# Overview\n",
            "See [[other-note]] and [Other](other.md). #inline/tag\n",
            "\n",
            "## Details\n",
        );
        fs::write(&source_path, source_text).unwrap();
        let source_uri = path_to_uri(&source_path.canonicalize().unwrap()).unwrap();

        let other_path = vault_dir.path().join("other.md");
        fs::write(
            &other_path,
            concat!(
                "---\n",
                "id: other-note\n",
                "aliases: [Other Alias]\n",
                "tags: [rust]\n",
                "---\n",
                "\n",
                "# Other Heading\n",
            ),
        )
        .unwrap();
        let other_uri = path_to_uri(&other_path.canonicalize().unwrap()).unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        (vault_dir, state, source_uri, other_uri)
    }

    fn document_link_for_raw_link(document_links: &[DocumentLink], raw_link: &str) -> DocumentLink {
        document_links
            .iter()
            .find(|document_link| {
                let data = parse_document_link_data(document_link.data.as_ref().unwrap()).unwrap();
                data.raw_link == raw_link
            })
            .cloned()
            .expect("document link should exist for the requested raw link")
    }

    fn nested_document_symbols(response: DocumentSymbolResponse) -> Vec<DocumentSymbol> {
        match response {
            DocumentSymbolResponse::Nested(symbols) => symbols,
            DocumentSymbolResponse::Flat(_) => panic!("document symbols should use nested response shape"),
        }
    }

    fn plain_text_edits_for_uri(edit: &WorkspaceEdit, uri: &Url) -> Vec<TextEdit> {
        let Some(DocumentChanges::Operations(operations)) = edit.document_changes.as_ref() else {
            panic!("workspace edit should use document change operations");
        };

        operations
            .iter()
            .filter_map(|operation| match operation {
                DocumentChangeOperation::Edit(document_edit) if document_edit.text_document.uri == *uri => {
                    Some(&document_edit.edits)
                }
                _ => None,
            })
            .flat_map(|edits| edits.iter())
            .map(|edit| match edit {
                OneOf::Left(edit) => edit.clone(),
                OneOf::Right(_) => panic!("rename edits should not use annotated text edits"),
            })
            .collect()
    }

    fn rename_file_operation(edit: &WorkspaceEdit) -> (Url, Url) {
        let Some(DocumentChanges::Operations(operations)) = edit.document_changes.as_ref() else {
            panic!("workspace edit should use document change operations");
        };

        operations
            .iter()
            .find_map(|operation| match operation {
                DocumentChangeOperation::Op(ResourceOp::Rename(rename)) => {
                    Some((rename.old_uri.clone(), rename.new_uri.clone()))
                }
                _ => None,
            })
            .expect("workspace edit should include a rename file operation")
    }

    fn action_by_title<'a>(actions: &'a [CodeAction], title: &str) -> &'a CodeAction {
        actions
            .iter()
            .find(|action| action.title == title)
            .unwrap_or_else(|| panic!("code action '{title}' should be present"))
    }

    #[test]
    fn document_symbols_include_note_structure_metadata_tags_and_links() {
        let (_vault_dir, state, source_uri, _other_uri) = symbol_state();

        let response = state.document_symbols_request(source_uri).unwrap().compute().unwrap();
        let symbols = nested_document_symbols(response);
        let names = symbols.iter().map(|symbol| symbol.name.as_str()).collect::<Vec<_>>();

        assert!(names.contains(&"id"));
        assert!(names.contains(&"title"));
        assert!(names.contains(&"aliases"));
        assert!(names.contains(&"tags"));
        assert!(names.contains(&"Work Alias"));
        assert!(names.contains(&"#rust"));
        assert!(names.contains(&"#inline/tag"));
        assert!(names.contains(&"Overview"));
        assert!(names.contains(&"Details"));
        assert!(names.contains(&"[[other-note]]"));
        assert!(names.contains(&"[Other](other.md)"));

        let overview = symbols
            .iter()
            .find(|symbol| symbol.name == "Overview")
            .expect("heading symbol should be present");
        assert_eq!(overview.kind, SymbolKind::STRING);
        assert_eq!(overview.selection_range.start.line, 8);

        let inline_tag = symbols
            .iter()
            .find(|symbol| symbol.name == "#inline/tag")
            .expect("inline tag symbol should be present");
        assert_eq!(inline_tag.kind, SymbolKind::ENUM_MEMBER);
        assert_eq!(inline_tag.selection_range.start.line, 9);
    }

    #[test]
    fn workspace_symbols_search_note_ids_aliases_tags_and_headings() {
        let (_vault_dir, state, _source_uri, other_uri) = symbol_state();

        let aliases = state.workspace_symbols_request("work".to_string()).compute().unwrap();
        assert!(aliases.iter().any(|symbol| symbol.name == "Work Alias"));

        let ids = state
            .workspace_symbols_request("other-note".to_string())
            .compute()
            .unwrap();
        assert!(ids.iter().any(|symbol| {
            symbol.name == "other-note" && symbol.kind == SymbolKind::FILE && symbol.location.uri == other_uri
        }));

        let tags = state.workspace_symbols_request("rust".to_string()).compute().unwrap();
        assert_eq!(tags.iter().filter(|symbol| symbol.name == "#rust").count(), 2);

        let headings = state
            .workspace_symbols_request("overview".to_string())
            .compute()
            .unwrap();
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].name, "Overview");
        assert_eq!(headings[0].location.range.start.line, 8);
    }

    #[test]
    fn code_action_request_converts_links_and_adds_missing_wiki_heading() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("notes/target note.md");
        fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        fs::write(
            &target_path,
            "---\nid: target-id\ntitle: Target Note\n---\n\n# Existing Heading\n",
        )
        .unwrap();
        let target_path = target_path.canonicalize().unwrap();
        let target_uri = path_to_uri(&target_path).unwrap();

        let source_path = vault_dir.path().join("source.md");
        let source_text = "See [[target-id#Missing Heading]] and [Existing](notes/target%20note.md#existing-heading).";
        fs::write(&source_path, source_text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();

        let state = BackendState::new(Vault::open(vault_dir.path()).unwrap());
        let wiki_position = position_for_substring(source_text, "[[target-id#Missing Heading]]");
        let wiki_actions = state
            .code_action_request(source_uri.clone(), Range::new(wiki_position, wiki_position), Vec::new())
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let convert = action_by_title(&wiki_actions, "Convert wiki link to markdown");
        assert_eq!(convert.kind, Some(CodeActionKind::REFACTOR_REWRITE));
        let source_edits = plain_text_edits_for_uri(convert.edit.as_ref().unwrap(), &source_uri);
        assert_eq!(
            source_edits[0].new_text,
            "[Target Note](notes/target%20note.md#Missing%20Heading)"
        );

        let add_heading = action_by_title(&wiki_actions, "Add heading 'Missing Heading' to notes/target note.md");
        let target_edits = plain_text_edits_for_uri(add_heading.edit.as_ref().unwrap(), &target_uri);
        assert_eq!(target_edits[0].new_text, "\n## Missing Heading\n");

        let markdown_position =
            position_for_substring(source_text, "[Existing](notes/target%20note.md#existing-heading)");
        let markdown_actions = state
            .code_action_request(
                source_uri.clone(),
                Range::new(markdown_position, markdown_position),
                Vec::new(),
            )
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let convert = action_by_title(&markdown_actions, "Convert markdown link to wiki");
        let source_edits = plain_text_edits_for_uri(convert.edit.as_ref().unwrap(), &source_uri);
        assert_eq!(source_edits[0].new_text, "[[target note#Existing Heading|Existing]]");
    }

    #[test]
    fn code_action_request_fixes_duplicate_id_and_alias_diagnostics() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let source_path = vault_dir.path().join("source.md");
        let source_text = "---\nid: duplicate\naliases:\n- shared\n---\n\nBody.\n";
        fs::write(&source_path, source_text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();

        let other_path = vault_dir.path().join("other.md");
        fs::write(&other_path, "---\nid: duplicate\naliases: [shared]\n---\n\nBody.\n").unwrap();

        let mut state = BackendState::new(Vault::open(vault_dir.path()).unwrap());
        let batch = state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap()
            .compute()
            .unwrap();
        let diagnostics = update_for_uri(&batch, &source_uri).diagnostics.clone();
        let duplicate_id = diagnostics
            .iter()
            .find(|diagnostic| diagnostic_code_is(diagnostic, "duplicate-id"))
            .expect("duplicate ID diagnostic should be present")
            .clone();
        let duplicate_alias = diagnostics
            .iter()
            .find(|diagnostic| diagnostic_code_is(diagnostic, "duplicate-alias"))
            .expect("duplicate alias diagnostic should be present")
            .clone();

        let id_actions = state
            .code_action_request(source_uri.clone(), duplicate_id.range, diagnostics.clone())
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let assign_id = action_by_title(&id_actions, "Assign unique note ID 'source'");
        let id_edits = plain_text_edits_for_uri(assign_id.edit.as_ref().unwrap(), &source_uri);
        assert_eq!(id_edits[0].new_text, "source");
        assert_eq!(assign_id.diagnostics.as_ref().unwrap().len(), 1);

        let id_actions_without_client_diagnostics = state
            .code_action_request(source_uri.clone(), duplicate_id.range, Vec::new())
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let assign_id = action_by_title(&id_actions_without_client_diagnostics, "Assign unique note ID 'source'");
        assert_eq!(assign_id.diagnostics.as_ref().unwrap().len(), 1);

        let alias_actions = state
            .code_action_request(source_uri.clone(), duplicate_alias.range, diagnostics)
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let change_alias = action_by_title(&alias_actions, "Change duplicate alias 'shared' to 'shared-2'");
        let alias_edits = plain_text_edits_for_uri(change_alias.edit.as_ref().unwrap(), &source_uri);
        assert_eq!(alias_edits[0].new_text, "shared-2");

        let remove_alias = action_by_title(&alias_actions, "Remove duplicate alias 'shared'");
        let alias_edits = plain_text_edits_for_uri(remove_alias.edit.as_ref().unwrap(), &source_uri);
        assert_eq!(alias_edits[0].new_text, "");
        assert_eq!(alias_edits[0].range.start.line, 3);
        assert_eq!(alias_edits[0].range.end.line, 4);

        let alias_actions_without_client_diagnostics = state
            .code_action_request(source_uri.clone(), duplicate_alias.range, Vec::new())
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let change_alias = action_by_title(
            &alias_actions_without_client_diagnostics,
            "Change duplicate alias 'shared' to 'shared-2'",
        );
        assert_eq!(change_alias.diagnostics.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn rename_request_renames_file_id_and_backlinks_when_id_matches_stem() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("old-note.md");
        fs::write(&target_path, "---\ntitle: old-note\nid: old-note\n---\n\nBody.\n").unwrap();
        let target_path = target_path.canonicalize().unwrap();
        let target_uri = path_to_uri(&target_path).unwrap();

        let source_path = vault_dir.path().join("source.md");
        let source_text = "See [[old-note]] and [Old](old-note.md).";
        fs::write(&source_path, source_text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();

        let state = BackendState::new(Vault::open(vault_dir.path()).unwrap());
        let position = position_for_substring(source_text, "[[old-note]]");
        let prepare = state
            .prepare_rename_request(source_uri.clone(), position)
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let PrepareRenameResponse::RangeWithPlaceholder { range, placeholder } = prepare else {
            panic!("prepare rename should return a placeholder range");
        };
        assert_eq!(placeholder, "old-note");
        assert_eq!(range.start.character, 6);
        assert_eq!(range.end.character, 14);

        let edit = state
            .rename_request(source_uri.clone(), position, "new-note".to_string())
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let new_uri = path_to_uri(&vault_dir.path().join("new-note.md")).unwrap();
        assert_eq!(rename_file_operation(&edit), (target_uri.clone(), new_uri));

        let target_edits = plain_text_edits_for_uri(&edit, &target_uri);
        assert_eq!(target_edits.len(), 1);
        assert_eq!(target_edits[0].new_text, "new-note");
        assert_eq!(target_edits[0].range.start.line, 2);
        assert_eq!(target_edits[0].range.start.character, 4);

        let source_edits = plain_text_edits_for_uri(&edit, &source_uri);
        let source_replacements = source_edits
            .iter()
            .map(|edit| edit.new_text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(source_replacements, vec!["[[new-note]]", "[Old](new-note.md)"]);
    }

    #[test]
    fn rename_request_keeps_custom_id_and_wiki_backlinks() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("my-note.md");
        fs::write(&target_path, "---\nid: custom-id\n---\n\nBody.\n").unwrap();
        let target_path = target_path.canonicalize().unwrap();
        let target_uri = path_to_uri(&target_path).unwrap();

        let source_path = vault_dir.path().join("source.md");
        let source_text = "See [[custom-id]] and [Custom](my-note.md).";
        fs::write(&source_path, source_text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();

        let state = BackendState::new(Vault::open(vault_dir.path()).unwrap());
        let edit = state
            .rename_request(
                source_uri.clone(),
                position_for_substring(source_text, "[[custom-id]]"),
                "renamed-note".to_string(),
            )
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        assert!(plain_text_edits_for_uri(&edit, &target_uri).is_empty());
        let source_edits = plain_text_edits_for_uri(&edit, &source_uri);
        assert_eq!(source_edits.len(), 1);
        assert_eq!(source_edits[0].new_text, "[Custom](renamed-note.md)");
    }

    #[test]
    fn frontmatter_tag_ranges_cover_core_supported_tag_lists() {
        let text = "---\ntags: [foo, bar, foo]\nother: value\n---\n";
        let ranges = frontmatter_tag_ranges(text);
        assert_eq!(
            ranges.iter().map(|range| range.tag.as_str()).collect::<Vec<_>>(),
            vec!["foo", "bar", "foo"]
        );
        assert_ne!(ranges[0].range, ranges[2].range);

        let block_text = "---\ntags:\n- foo\n- bar\n---\n";
        let block_ranges = frontmatter_tag_ranges(block_text);
        assert_eq!(
            block_ranges.iter().map(|range| range.tag.as_str()).collect::<Vec<_>>(),
            vec!["foo", "bar"]
        );

        assert!(frontmatter_tag_ranges("---\ntags: foo\n---\n").is_empty());
    }

    #[test]
    fn navigation_request_handles_tag_language_features() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let tagged_path = vault_dir.path().join("tagged.md");
        let tagged_text = "---\nid: tagged\ntags: [project, work]\n---\n\nBody #project and #project/task.\n";
        fs::write(&tagged_path, tagged_text).unwrap();
        let tagged_path = tagged_path.canonicalize().unwrap();
        let tagged_uri = path_to_uri(&tagged_path).unwrap();

        let other_path = vault_dir.path().join("other.md");
        fs::write(&other_path, "---\nid: other\ntags:\n- project\n---\n\nBody #project.\n").unwrap();
        let other_path = other_path.canonicalize().unwrap();
        let other_uri = path_to_uri(&other_path).unwrap();

        let state = BackendState::new(Vault::open(vault_dir.path()).unwrap());
        let position = position_for_substring(tagged_text, "#project and");

        let hover = state
            .navigation_request(tagged_uri.clone(), position)
            .unwrap()
            .compute_hover()
            .unwrap()
            .unwrap();
        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected markdown hover contents");
        };
        assert!(contents.value.contains("**#project**"));
        assert!(contents.value.contains("Occurrences: 5"));

        let references = state
            .navigation_request(tagged_uri.clone(), position)
            .unwrap()
            .compute_references()
            .unwrap()
            .unwrap();
        assert_eq!(references.len(), 5);
        assert_eq!(
            references.iter().filter(|location| location.uri == tagged_uri).count(),
            3
        );
        assert_eq!(
            references.iter().filter(|location| location.uri == other_uri).count(),
            2
        );

        let definition = state
            .navigation_request(tagged_uri.clone(), position)
            .unwrap()
            .compute_definition()
            .unwrap()
            .unwrap();
        let GotoDefinitionResponse::Array(definitions) = definition else {
            panic!("multiple tag occurrences should return an array definition response");
        };
        assert_eq!(definitions.len(), 5);

        let prepare = state
            .prepare_rename_request(tagged_uri.clone(), position)
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } = prepare else {
            panic!("tag prepareRename should return a placeholder range");
        };
        assert_eq!(placeholder, "#project");

        let edit = state
            .rename_request(tagged_uri.clone(), position, "#area".to_string())
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();
        let tagged_edits = plain_text_edits_for_uri(&edit, &tagged_uri);
        assert_eq!(
            tagged_edits
                .iter()
                .map(|edit| edit.new_text.as_str())
                .collect::<Vec<_>>(),
            vec!["area", "#area", "#area"]
        );
        let other_edits = plain_text_edits_for_uri(&edit, &other_uri);
        assert_eq!(
            other_edits
                .iter()
                .map(|edit| edit.new_text.as_str())
                .collect::<Vec<_>>(),
            vec!["area", "#area"]
        );
    }

    #[test]
    fn navigation_request_returns_metadata_for_wiki_links() {
        let (_vault_dir, state, source_uri, _target_uri, _backlink_uri, source_text) = navigation_state();

        let hover = state
            .navigation_request(source_uri, position_for_substring(&source_text, "[[target-id]]"))
            .unwrap()
            .compute_hover()
            .unwrap()
            .unwrap();

        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected markdown hover contents");
        };
        assert!(contents.value.contains("Target Note"));
        assert!(contents.value.contains("target-id"));
        assert!(contents.value.contains("target-alias"));
        assert!(contents.value.contains("rust"));
    }

    #[test]
    fn navigation_request_resolves_relative_markdown_links() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("notes/target.md");
        fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        fs::write(
            &target_path,
            "---\nid: target-id\ntitle: Target Note\naliases: [target-alias]\n---\n\nBody.\n",
        )
        .unwrap();

        let source_path = vault_dir.path().join("journal/today.md");
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, "placeholder").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        let source_text = "See [Target](../notes/target.md).";

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        let hover = state
            .navigation_request(source_uri, position_for_substring(source_text, "../notes/target.md"))
            .unwrap()
            .compute_hover()
            .unwrap()
            .unwrap();

        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected markdown hover contents");
        };
        assert!(contents.value.contains("Target Note"));
        assert!(contents.value.contains("notes/target.md"));
    }

    #[test]
    fn code_action_request_rejects_create_note_paths_outside_the_vault() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let source_path = vault_dir.path().join("source.md");
        let source_text = "See [[../outside]] and [Outside](../outside.md).";
        fs::write(&source_path, source_text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        let wiki_actions = state
            .code_action_request(
                source_uri.clone(),
                Range::new(
                    position_for_substring(source_text, "[[../outside]]"),
                    position_for_substring(source_text, "[[../outside]]"),
                ),
                Vec::new(),
            )
            .unwrap()
            .compute()
            .unwrap();
        assert!(wiki_actions.is_none());

        let markdown_actions = state
            .code_action_request(
                source_uri,
                Range::new(
                    position_for_substring(source_text, "[Outside](../outside.md)"),
                    position_for_substring(source_text, "[Outside](../outside.md)"),
                ),
                Vec::new(),
            )
            .unwrap()
            .compute()
            .unwrap();
        assert!(markdown_actions.is_none());
    }

    #[test]
    fn document_links_request_lists_wiki_and_markdown_links() {
        let (_vault_dir, state, source_uri, _target_uri, _backlink_uri, _source_text) = navigation_state();

        let document_links = state.document_links_request(source_uri).unwrap().compute().unwrap();

        assert_eq!(document_links.len(), 2);
        assert!(
            document_links
                .iter()
                .all(|document_link| document_link.target.is_none())
        );
        assert_eq!(
            parse_document_link_data(document_links[0].data.as_ref().unwrap())
                .unwrap()
                .raw_link,
            "[[target-id]]"
        );
        assert_eq!(
            parse_document_link_data(document_links[1].data.as_ref().unwrap())
                .unwrap()
                .raw_link,
            "[Target Markdown](target.md)"
        );
    }

    #[test]
    fn resolve_document_link_request_resolves_markdown_note_links() {
        let (_vault_dir, state, source_uri, target_uri, _backlink_uri, _source_text) = navigation_state();
        let document_links = state.document_links_request(source_uri).unwrap().compute().unwrap();
        let markdown_link = document_link_for_raw_link(&document_links, "[Target Markdown](target.md)");

        let resolved = state
            .resolve_document_link_request(markdown_link)
            .unwrap()
            .compute()
            .unwrap();

        assert_eq!(resolved.target.as_ref(), Some(&target_uri));
        assert!(resolved.tooltip.as_ref().unwrap().contains("Target Note"));
    }

    #[test]
    fn navigation_request_returns_backlinks_for_current_note_when_off_link() {
        let (_vault_dir, state, source_uri, target_uri, backlink_uri, _source_text) = navigation_state();

        let references = state
            .navigation_request(target_uri.clone(), Position::new(0, 0))
            .unwrap()
            .compute_references()
            .unwrap()
            .unwrap();

        assert_eq!(references.len(), 3);
        assert_eq!(
            references.iter().filter(|location| location.uri == source_uri).count(),
            2
        );
        assert!(references.iter().any(|location| location.uri == backlink_uri));
    }

    #[test]
    fn navigation_request_returns_backlinks_for_link_target_when_on_markdown_link() {
        let (_vault_dir, state, source_uri, _target_uri, backlink_uri, source_text) = navigation_state();

        let references = state
            .navigation_request(
                source_uri.clone(),
                position_for_substring(&source_text, "[Target Markdown](target.md)"),
            )
            .unwrap()
            .compute_references()
            .unwrap()
            .unwrap();

        assert_eq!(references.len(), 3);
        assert_eq!(
            references.iter().filter(|location| location.uri == source_uri).count(),
            2
        );
        assert!(references.iter().any(|location| location.uri == backlink_uri));
    }

    #[test]
    fn navigation_request_returns_definition_for_link_target() {
        let (_vault_dir, state, source_uri, target_uri, _backlink_uri, source_text) = navigation_state();

        let definition = state
            .navigation_request(source_uri, position_for_substring(&source_text, "[[target-id]]"))
            .unwrap()
            .compute_definition()
            .unwrap()
            .unwrap();

        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected a single definition location");
        };
        assert_eq!(location.uri, target_uri);
        assert_eq!(location.range.start.line, 2);
    }

    #[test]
    fn navigation_request_returns_definition_for_wiki_heading_anchor() {
        let (_vault_dir, state, source_uri, target_uri, source_text) = heading_anchor_navigation_state();

        let definition = state
            .navigation_request(
                source_uri,
                position_for_substring(&source_text, "[[target-id#Linked Heading]]"),
            )
            .unwrap()
            .compute_definition()
            .unwrap()
            .unwrap();

        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected a single definition location");
        };
        assert_eq!(location.uri.path(), target_uri.path());
        assert_eq!(location.uri.fragment(), Some("Linked%20Heading"));
        assert_eq!(location.range.start.line, 7);
        assert_eq!(location.range.start.character, 3);
    }

    #[test]
    fn navigation_request_returns_definition_for_markdown_heading_anchor() {
        let (_vault_dir, state, source_uri, target_uri, source_text) = heading_anchor_navigation_state();

        let definition = state
            .navigation_request(
                source_uri,
                position_for_substring(&source_text, "[Target Markdown](target.md#linked-heading)"),
            )
            .unwrap()
            .compute_definition()
            .unwrap()
            .unwrap();

        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected a single definition location");
        };
        assert_eq!(location.uri.path(), target_uri.path());
        assert_eq!(location.uri.fragment(), Some("linked-heading"));
        assert_eq!(location.range.start.line, 7);
        assert_eq!(location.range.start.character, 3);
    }

    #[test]
    fn navigation_request_returns_definition_for_nested_wiki_heading_anchor() {
        let (_vault_dir, state, source_uri, target_uri, source_text) = nested_heading_anchor_navigation_state();

        let definition = state
            .navigation_request(
                source_uri,
                position_for_substring(&source_text, "[[target-id#Heading A#Subheading B]]"),
            )
            .unwrap()
            .compute_definition()
            .unwrap()
            .unwrap();

        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected a single definition location");
        };
        assert_eq!(location.uri.path(), target_uri.path());
        assert_eq!(location.uri.fragment(), Some("Heading%20A#Subheading%20B"));
        assert_eq!(location.range.start.line, 11);
        assert_eq!(location.range.start.character, 3);
    }

    #[test]
    fn navigation_request_returns_definition_for_nested_slug_heading_anchor() {
        let (_vault_dir, state, source_uri, target_uri, source_text) = nested_heading_anchor_navigation_state();

        let definition = state
            .navigation_request(
                source_uri,
                position_for_substring(&source_text, "[[target-id#heading-a#subheading-b]]"),
            )
            .unwrap()
            .compute_definition()
            .unwrap()
            .unwrap();

        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected a single definition location");
        };
        assert_eq!(location.uri.path(), target_uri.path());
        assert_eq!(location.uri.fragment(), Some("heading-a#subheading-b"));
        assert_eq!(location.range.start.line, 11);
        assert_eq!(location.range.start.character, 3);
    }

    #[test]
    fn navigation_request_returns_definition_for_nested_markdown_heading_anchor() {
        let (_vault_dir, state, source_uri, target_uri, source_text) = nested_heading_anchor_navigation_state();

        let definition = state
            .navigation_request(
                source_uri,
                position_for_substring(&source_text, "[Target Markdown](target.md#heading-a#subheading-b)"),
            )
            .unwrap()
            .compute_definition()
            .unwrap()
            .unwrap();

        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected a single definition location");
        };
        assert_eq!(location.uri.path(), target_uri.path());
        assert_eq!(location.uri.fragment(), Some("heading-a#subheading-b"));
        assert_eq!(location.range.start.line, 11);
        assert_eq!(location.range.start.character, 3);
    }

    #[test]
    fn navigation_request_returns_definition_for_anchor_only_wiki_link() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "placeholder").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        let source_text = "# Overview\n\n## Getting Started\n\nSee [[#Getting Started]].\n";

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        let definition = state
            .navigation_request(
                source_uri.clone(),
                position_for_substring(source_text, "[[#Getting Started]]"),
            )
            .unwrap()
            .compute_definition()
            .unwrap()
            .unwrap();

        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected a single definition location");
        };
        assert_eq!(location.uri.path(), source_uri.path());
        assert_eq!(location.range.start.line, 2);
        assert_eq!(location.range.start.character, 3);
    }

    fn completion_state() -> (tempfile::TempDir, BackendState, Url, Url) {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("target.md");
        fs::write(
            &target_path,
            "---\nid: target-id\ntitle: Target Note\naliases: [Target Alias]\n---\n\nBody.\n",
        )
        .unwrap();
        let target_uri = path_to_uri(&target_path.canonicalize().unwrap()).unwrap();

        let other_path = vault_dir.path().join("other.md");
        fs::write(&other_path, "---\nid: other-note\n---\n\nBody.\n").unwrap();
        let other_uri = path_to_uri(&other_path.canonicalize().unwrap()).unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let state = BackendState::new(vault);
        (vault_dir, state, target_uri, other_uri)
    }

    fn completion_labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn detect_link_context_returns_wiki_for_open_double_bracket() {
        let ctx = detect_link_context("See [[tar").unwrap();
        assert!(matches!(ctx, LinkContext::Wiki { note_query, .. } if note_query == "tar"));
    }

    #[test]
    fn detect_link_context_returns_none_for_closed_wiki_link() {
        assert!(detect_link_context("See [[target]]").is_none());
    }

    #[test]
    fn detect_link_context_returns_none_for_wiki_link_with_pipe() {
        assert!(detect_link_context("See [[target|Al").is_none());
    }

    #[test]
    fn detect_link_context_returns_markdown_for_open_bracket() {
        let ctx = detect_link_context("See [tar").unwrap();
        assert!(matches!(ctx, LinkContext::Markdown { query, .. } if query == "tar"));
    }

    #[test]
    fn detect_link_context_prefers_wiki_over_markdown_when_both_open() {
        let ctx = detect_link_context("See [[tar").unwrap();
        assert!(matches!(ctx, LinkContext::Wiki { .. }));
    }

    #[test]
    fn detect_link_context_returns_none_for_plain_text() {
        assert!(detect_link_context("just some text").is_none());
    }

    #[test]
    fn detect_link_context_returns_wiki_with_empty_query_for_bare_open() {
        let ctx = detect_link_context("[[").unwrap();
        assert!(matches!(ctx, LinkContext::Wiki { note_query, .. } if note_query.is_empty()));
    }

    #[test]
    fn completion_request_returns_wiki_completions_for_partial_wiki_link() {
        let (_vault_dir, mut state, target_uri, _other_uri) = completion_state();
        let source_path = _vault_dir.path().join("source.md");
        fs::write(&source_path, "See [[tar").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        state
            .open_document(source_uri.clone(), 1, "See [[tar".to_string())
            .unwrap();

        let items = state
            .completion_request(source_uri, Position::new(0, 9))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let labels = completion_labels(&items);
        assert!(labels.contains(&"[[target-id]]"), "missing [[target-id]]");
        assert!(labels.contains(&"[[Target Note]]"), "missing [[Target Note]]");
        assert!(
            labels.contains(&"[[target-id|Target Alias]]"),
            "missing [[target-id|Target Alias]]"
        );
        assert!(labels.contains(&"[[Target Alias]]"), "missing [[Target Alias]]");
        assert!(!labels.contains(&"[[other-note]]"), "other-note should not match 'tar'");
        let _ = target_uri;
    }

    #[test]
    fn completion_request_returns_markdown_completions_for_partial_markdown_link() {
        let (_vault_dir, mut state, _target_uri, _other_uri) = completion_state();
        let source_path = _vault_dir.path().join("source.md");
        fs::write(&source_path, "See [tar").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        state
            .open_document(source_uri.clone(), 1, "See [tar".to_string())
            .unwrap();

        let items = state
            .completion_request(source_uri, Position::new(0, 8))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let labels = completion_labels(&items);
        assert!(labels.iter().any(|l| l.starts_with("[target-id](") && l.ends_with(')')));
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with("[Target Note](") && l.ends_with(')'))
        );
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with("[Target Alias](") && l.ends_with(')'))
        );
        assert!(!labels.iter().any(|l| l.starts_with("[other-note](")));
    }

    #[test]
    fn completion_request_returns_none_outside_link_context() {
        let (_vault_dir, mut state, _target_uri, _other_uri) = completion_state();
        let source_path = _vault_dir.path().join("source.md");
        fs::write(&source_path, "just plain text").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        state
            .open_document(source_uri.clone(), 1, "just plain text".to_string())
            .unwrap();

        let result = state
            .completion_request(source_uri, Position::new(0, 15))
            .unwrap()
            .compute()
            .unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn completion_request_returns_all_notes_for_empty_query() {
        let (_vault_dir, mut state, _target_uri, _other_uri) = completion_state();
        let source_path = _vault_dir.path().join("source.md");
        fs::write(&source_path, "[[").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        state.open_document(source_uri.clone(), 1, "[[".to_string()).unwrap();

        let items = state
            .completion_request(source_uri, Position::new(0, 2))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let labels = completion_labels(&items);
        assert!(labels.contains(&"[[target-id]]"));
        assert!(labels.contains(&"[[other-note]]"));
    }

    #[test]
    fn detect_link_context_returns_wiki_with_heading_query_after_hash() {
        let ctx = detect_link_context("See [[target-id#Head").unwrap();
        assert!(
            matches!(ctx, LinkContext::Wiki { note_query, heading_query: Some(hq), .. }
                if note_query == "target-id" && hq == "Head")
        );
    }

    #[test]
    fn detect_link_context_returns_wiki_with_empty_heading_query_at_hash() {
        let ctx = detect_link_context("See [[target-id#").unwrap();
        assert!(
            matches!(ctx, LinkContext::Wiki { note_query, heading_query: Some(hq), .. }
                if note_query == "target-id" && hq.is_empty())
        );
    }

    #[test]
    fn completion_request_returns_heading_completions_for_partial_heading() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("target.md");
        fs::write(
            &target_path,
            "---\nid: target-id\ntitle: Target Note\n---\n\n# Overview\n\n## Getting Started\n\nBody.\n",
        )
        .unwrap();
        let target_path = target_path.canonicalize().unwrap();

        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "[[target-id#Get").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, "[[target-id#Get".to_string())
            .unwrap();

        let items = state
            .completion_request(source_uri, Position::new(0, 15))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(
            labels.contains(&"[[target-id#Getting Started]]"),
            "missing [[target-id#Getting Started]]"
        );
        assert!(
            labels.contains(&"[[Target Note#Getting Started]]"),
            "missing [[Target Note#Getting Started]]"
        );
        assert!(
            !labels.iter().any(|l| l.contains("Overview")),
            "Overview heading should not match 'Get'"
        );
        let _ = target_path;
    }

    #[test]
    fn completion_request_extends_range_past_wiki_closing_brackets() {
        let (_vault_dir, mut state, _target_uri, _other_uri) = completion_state();
        let source_path = _vault_dir.path().join("source.md");
        let text = "[[tar]]";
        fs::write(&source_path, text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        state.open_document(source_uri.clone(), 1, text.to_string()).unwrap();

        // cursor at position 5 (after "[[tar", before "]]")
        let items = state
            .completion_request(source_uri, Position::new(0, 5))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let first = items.iter().find(|item| item.label == "[[target-id]]").unwrap();
        let edit = match first.text_edit.as_ref().unwrap() {
            CompletionTextEdit::Edit(e) => e,
            _ => panic!("expected a plain TextEdit"),
        };
        // range should extend past "]]" (chars 5-7)
        assert_eq!(edit.range.start.character, 0, "range should start at [[");
        assert_eq!(edit.range.end.character, 7, "range should extend past ]]");
    }

    #[test]
    fn completion_request_extends_range_past_markdown_closing_bracket() {
        let (_vault_dir, mut state, _target_uri, _other_uri) = completion_state();
        let source_path = _vault_dir.path().join("source.md");
        let text = "[tar]";
        fs::write(&source_path, text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        state.open_document(source_uri.clone(), 1, text.to_string()).unwrap();

        // cursor at position 4 (after "[tar", before "]")
        let items = state
            .completion_request(source_uri, Position::new(0, 4))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let first = items
            .iter()
            .find(|item| item.label.starts_with("[target-id]("))
            .unwrap();
        let edit = match first.text_edit.as_ref().unwrap() {
            CompletionTextEdit::Edit(e) => e,
            _ => panic!("expected a plain TextEdit"),
        };
        // range should extend past "]" (char 4)
        assert_eq!(edit.range.start.character, 0, "range should start at [");
        assert_eq!(edit.range.end.character, 5, "range should extend past ]");
    }

    #[test]
    fn detect_link_context_returns_wiki_with_empty_note_query_and_heading_query_for_anchor_link() {
        let ctx = detect_link_context("See [[#Get").unwrap();
        assert!(
            matches!(ctx, LinkContext::Wiki { note_query, heading_query: Some(hq), .. }
                if note_query.is_empty() && hq == "Get")
        );
    }

    #[test]
    fn completion_request_returns_anchor_completions_for_current_document() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        // Other note with the same headings — must NOT appear in anchor completions.
        let other_path = vault_dir.path().join("other.md");
        fs::write(&other_path, "# Getting Started\n\nBody.\n").unwrap();

        let source_path = vault_dir.path().join("source.md");
        let source_text = "# Overview\n\n## Getting Started\n\n[[#Get";
        fs::write(&source_path, source_text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        // cursor at end of "[[#Get" (line 4, char 6)
        let items = state
            .completion_request(source_uri, Position::new(4, 6))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"[[#Getting Started]]"), "missing [[#Getting Started]]");
        // "Overview" should not match "Get"
        assert!(!labels.contains(&"[[#Overview]]"), "Overview should not match 'Get'");
        // Should only have items with the [[#...]] form, not [[other-note#...]]
        assert!(
            labels.iter().all(|l| l.starts_with("[[#")),
            "all labels should be anchor-only form"
        );
    }

    #[test]
    fn completion_request_returns_all_headings_for_bare_anchor_trigger() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let source_path = vault_dir.path().join("source.md");
        let source_text = "# Overview\n\n## Details\n\n[[#";
        fs::write(&source_path, source_text).unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        // cursor at end of "[[#" (line 4, char 3)
        let items = state
            .completion_request(source_uri, Position::new(4, 3))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"[[#Overview]]"), "missing [[#Overview]]");
        assert!(labels.contains(&"[[#Details]]"), "missing [[#Details]]");
    }
}
