use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use obsidian_core::search;
use obsidian_core::{
    BrokenLink, DuplicateAlias, DuplicateId, InlineLocation, Link, Note, NoteError, Vault, VaultError,
};
use thiserror::Error;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticSeverity, Hover, HoverContents, MarkupContent, MarkupKind, NumberOrString, Position, Range,
    TextDocumentContentChangeEvent, Url,
};

use crate::uri::{UriError, path_to_uri, uri_to_path, vault_relative_path};

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
}

#[derive(Debug)]
pub struct DiagnosticsRequest {
    snapshot: StateSnapshot,
    previously_published: HashMap<PathBuf, Url>,
    primary_document: PrimaryDocument,
    revision: u64,
}

#[derive(Debug)]
pub struct HoverRequest {
    snapshot: StateSnapshot,
    path: PathBuf,
    position: Position,
}

#[derive(Debug)]
struct PrimaryDocument {
    path: PathBuf,
    uri: Url,
    version: Option<i32>,
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
}

pub struct BackendState {
    vault: Vault,
    open_documents: HashMap<PathBuf, OpenDocument>,
    published_diagnostics: HashMap<PathBuf, Url>,
    diagnostics_revision: u64,
}

impl BackendState {
    pub fn new(vault: Vault) -> Self {
        Self {
            vault,
            open_documents: HashMap::new(),
            published_diagnostics: HashMap::new(),
            diagnostics_revision: 0,
        }
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

    pub fn hover_request(&self, uri: Url, position: Position) -> Result<HoverRequest, StateError> {
        let path = self.path_from_uri(&uri)?;

        Ok(HoverRequest {
            snapshot: self.snapshot(),
            path,
            position,
        })
    }

    fn sync_document(&mut self, uri: Url, version: i32, text: String) -> Result<DiagnosticsRequest, StateError> {
        let path = self.path_from_uri(&uri)?;

        self.vault.load_note(Note::parse(&path, &text));
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
        let report = vault.check(|_| true);
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

impl HoverRequest {
    pub fn compute(self) -> Result<Option<Hover>, StateError> {
        let vault = self.snapshot.build_vault()?;
        let source_note = self.snapshot.note_for_path(&self.path)?;
        let Some(hovered_link) = source_note
            .links
            .iter()
            .find(|link| position_in_location(self.position, &link.location))
        else {
            return Ok(None);
        };

        let notes: Vec<Note> = vault
            .notes_filtered(|_| true)
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        let matching_notes: Vec<&Note> = notes
            .iter()
            .filter(|note| {
                search::find_matching_links(&source_note, note, vault.path())
                    .iter()
                    .any(|candidate| same_location(&candidate.location, &hovered_link.location))
            })
            .collect();

        if matching_notes.is_empty() {
            return Ok(None);
        }

        let contents = if matching_notes.len() == 1 {
            render_note_hover(matching_notes[0], self.snapshot.vault_path.as_path())
        } else {
            render_ambiguous_hover(&matching_notes, self.snapshot.vault_path.as_path())
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: contents,
            }),
            range: Some(location_to_range(&hovered_link.location)),
        }))
    }
}

impl StateSnapshot {
    fn build_vault(&self) -> Result<Vault, StateError> {
        let mut vault = Vault::open(&self.vault_path)?;
        for document in self.open_documents.values() {
            vault.load_note(Note::parse(&document.path, &document.text));
        }
        Ok(vault)
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

fn same_location(left: &InlineLocation, right: &InlineLocation) -> bool {
    left.line == right.line && left.col_start == right.col_start && left.col_end == right.col_end
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

    #[test]
    fn hover_request_returns_metadata_for_wiki_links() {
        let vault_dir = tempfile::tempdir().unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        let target_path = vault_dir.path().join("target.md");
        fs::write(
            &target_path,
            "---\nid: target-id\ntitle: Target Note\naliases: [target-alias]\ntags: [rust]\n---\n\nBody.\n",
        )
        .unwrap();

        let source_path = vault_dir.path().join("source.md");
        fs::write(&source_path, "placeholder").unwrap();
        let source_path = source_path.canonicalize().unwrap();
        let source_uri = path_to_uri(&source_path).unwrap();
        let source_text = "See [[target-id]].";

        let vault = Vault::open(vault_dir.path()).unwrap();
        let mut state = BackendState::new(vault);
        state
            .open_document(source_uri.clone(), 1, source_text.to_string())
            .unwrap();

        let hover = state
            .hover_request(source_uri, position_for_substring(source_text, "[[target-id]]"))
            .unwrap()
            .compute()
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
    fn hover_request_resolves_relative_markdown_links() {
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
            .hover_request(source_uri, position_for_substring(source_text, "../notes/target.md"))
            .unwrap()
            .compute()
            .unwrap()
            .unwrap();

        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected markdown hover contents");
        };
        assert!(contents.value.contains("Target Note"));
        assert!(contents.value.contains("notes/target.md"));
    }
}
