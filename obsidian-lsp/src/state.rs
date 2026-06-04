use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use obsidian_core::{
    BrokenLink, DuplicateAlias, DuplicateId, InlineLocation, Link, LocatedLink, Note, NoteError, Vault, VaultError,
};
use serde_json::{Value, json};
use thiserror::Error;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, Command, CompletionItem, CompletionItemKind, CompletionTextEdit, Diagnostic,
    DiagnosticSeverity, DocumentChangeOperation, DocumentChanges, DocumentLink, GotoDefinitionResponse, Hover,
    HoverContents, Location, MarkupContent, MarkupKind, NumberOrString, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, Range, TextDocumentContentChangeEvent, TextDocumentEdit, TextEdit, Url, WorkspaceEdit,
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
    position: Position,
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
}

struct NavigationContext {
    snapshot: StateSnapshot,
    vault: Vault,
    notes: Vec<Note>,
    source_note: Note,
    selected_link: Option<LocatedLink>,
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

    pub fn code_action_request(&self, uri: Url, position: Position) -> Result<CodeActionRequest, StateError> {
        let path = self.path_from_uri(&uri)?;

        Ok(CodeActionRequest {
            snapshot: self.snapshot(),
            path,
            position,
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

        Ok(NavigationContext {
            snapshot: self.snapshot,
            vault,
            notes,
            source_note,
            selected_link,
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

        let (note_query, heading_query, link_start_char) = match &context {
            LinkContext::Wiki {
                note_query,
                heading_query,
                link_start_char,
            } => (note_query.as_str(), heading_query.as_deref(), *link_start_char),
            LinkContext::Markdown { query, link_start_char } => (query.as_str(), None, *link_start_char),
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
        let source_note = self.snapshot.note_for_path(&self.path)?;
        let Some(located_link) = find_link_at_position(&source_note, self.position) else {
            return Ok(None);
        };

        let vault = self.snapshot.build_vault()?;
        let notes = self.snapshot.notes(&vault);
        let targets = resolve_link_targets(&self.path, &located_link.link, &notes, vault.path());
        if !targets.is_empty() {
            return Ok(None);
        }

        let Some(new_path) = compute_new_note_path(&self.path, vault.path(), &located_link.link) else {
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

        let new_path_str = new_path.to_string_lossy().into_owned();
        let new_uri = path_to_uri(&new_path)?;
        let title = format!("Create note '{stem}'");

        // `edit` is a TextDocumentEdit (no CreateFile) so preview plugins can show the diff
        // without creating any file on disk. `command` does the actual work when the user applies.
        let action = CodeAction {
            title: title.clone(),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                document_changes: Some(DocumentChanges::Operations(vec![DocumentChangeOperation::Edit(
                    TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier {
                            uri: new_uri,
                            version: None,
                        },
                        edits: vec![OneOf::Left(TextEdit {
                            range: Range {
                                start: Position { line: 0, character: 0 },
                                end: Position { line: 0, character: 0 },
                            },
                            new_text: format!("---\nid: {stem}\n---\n"),
                        })],
                    },
                )])),
                ..Default::default()
            }),
            command: Some(Command {
                title,
                command: "obsidian.createNote".to_string(),
                arguments: Some(vec![json!(new_path_str)]),
            }),
            ..Default::default()
        };

        Ok(Some(vec![action]))
    }
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
    }
}

fn parse_headings(text: &str) -> Vec<String> {
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
        if let Some((_, _, heading_text)) = heading_line_parts(line) {
            headings.push(heading_text.to_string());
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
                position_for_substring(source_text, "[[../outside]]"),
            )
            .unwrap()
            .compute()
            .unwrap();
        assert!(wiki_actions.is_none());

        let markdown_actions = state
            .code_action_request(
                source_uri,
                position_for_substring(source_text, "[Outside](../outside.md)"),
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
