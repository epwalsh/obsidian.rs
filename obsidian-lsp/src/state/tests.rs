use super::*;
use crate::uri::path_to_uri;
use std::fs;

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
fn open_document_adds_note_to_lsp_overlay() {
    let (_vault_dir, mut state, note_path, uri) = open_state();

    let request = state.open_document(uri.clone(), 1, "buffer body".to_string()).unwrap();
    let batch = request.compute().unwrap();
    let update = update_for_uri(&batch, &uri);

    assert_eq!(update.uri, uri);
    assert_eq!(update.version, Some(1));
    assert!(update.diagnostics.is_empty());
    assert_eq!(state.open_documents.get(&note_path).unwrap().text, "buffer body");
    assert_eq!(
        state.snapshot().note_for_path(&note_path).unwrap().body.as_deref(),
        Some("buffer body")
    );
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
    assert_eq!(
        state.snapshot().note_for_path(&note_path).unwrap().body.as_deref(),
        Some("changed body")
    );
    assert_eq!(update_for_uri(&batch, &note_uri).version, Some(2));
}

#[test]
fn file_change_create_clears_broken_link_without_open_documents() {
    let vault_dir = tempfile::tempdir().unwrap();
    fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();
    let source_path = vault_dir.path().join("source.md");
    fs::write(&source_path, "See [[target]].").unwrap();
    let source_path = source_path.canonicalize().unwrap();
    let source_uri = path_to_uri(&source_path).unwrap();

    let mut state = BackendState::new(Vault::open(vault_dir.path()).unwrap());
    let batch = state.global_diagnostics_request().compute().unwrap();
    assert_eq!(codes(update_for_uri(&batch, &source_uri)), vec!["broken-link"]);
    state.set_published_diagnostics(batch.published_diagnostics);

    let target_path = vault_dir.path().join("target.md");
    fs::write(&target_path, "---\nid: target\n---\n").unwrap();
    let request = state
        .apply_file_changes(vec![FileChange {
            path: target_path,
            kind: FileChangeKind::Created,
        }])
        .unwrap()
        .expect("created note should trigger diagnostics");
    let batch = request.compute().unwrap();

    assert!(codes(update_for_uri(&batch, &source_uri)).is_empty());
}

#[test]
fn file_change_delete_creates_broken_link_diagnostic() {
    let vault_dir = tempfile::tempdir().unwrap();
    fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();
    let source_path = vault_dir.path().join("source.md");
    fs::write(&source_path, "See [[target]].").unwrap();
    let source_path = source_path.canonicalize().unwrap();
    let source_uri = path_to_uri(&source_path).unwrap();
    let target_path = vault_dir.path().join("target.md");
    fs::write(&target_path, "---\nid: target\n---\n").unwrap();
    let target_path = target_path.canonicalize().unwrap();

    let mut state = BackendState::new(Vault::open(vault_dir.path()).unwrap());
    let batch = state.global_diagnostics_request().compute().unwrap();
    state.set_published_diagnostics(batch.published_diagnostics);

    fs::remove_file(&target_path).unwrap();
    let request = state
        .apply_file_changes(vec![FileChange {
            path: target_path,
            kind: FileChangeKind::Deleted,
        }])
        .unwrap()
        .expect("deleted note should trigger diagnostics");
    let batch = request.compute().unwrap();

    assert_eq!(codes(update_for_uri(&batch, &source_uri)), vec!["broken-link"]);
}

#[test]
fn open_documents_shadow_external_file_changes_until_close() {
    let (_vault_dir, mut state, note_path, uri) = open_state();
    state
        .open_document(uri.clone(), 1, "---\nid: open-id\n---\n".to_string())
        .unwrap();
    fs::write(&note_path, "---\nid: disk-id\n---\n").unwrap();

    state
        .apply_file_changes(vec![FileChange {
            path: note_path.clone(),
            kind: FileChangeKind::Changed,
        }])
        .unwrap()
        .expect("changed note should trigger diagnostics");
    let snapshot = state.snapshot();
    assert_eq!(snapshot.note_for_path(&note_path).unwrap().id, "open-id");

    state.close_document(uri).unwrap();
    let snapshot = state.snapshot();
    assert_eq!(snapshot.note_for_path(&note_path).unwrap().id, "disk-id");
}

#[test]
fn close_document_unloads_the_in_memory_override() {
    let (_vault_dir, mut state, note_path, uri) = open_state();
    state.open_document(uri.clone(), 1, "buffer body".to_string()).unwrap();

    let request = state.close_document(uri.clone()).unwrap();
    let batch = request.compute().unwrap();
    let update = update_for_uri(&batch, &uri);

    assert!(update.diagnostics.is_empty());
    assert!(!state.open_documents.contains_key(&note_path));
    assert_eq!(
        state.snapshot().note_for_path(&note_path).unwrap().body.as_deref(),
        None
    );
    assert_eq!(state.snapshot().text_for_path(&note_path).unwrap(), "disk body");
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

    let markdown_position = position_for_substring(source_text, "[Existing](notes/target%20note.md#existing-heading)");
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
    let id_line_actions_without_client_diagnostics = state
        .code_action_request(
            source_uri.clone(),
            Range::new(
                Position::new(duplicate_id.range.start.line, 0),
                Position::new(duplicate_id.range.start.line, 0),
            ),
            Vec::new(),
        )
        .unwrap()
        .compute()
        .unwrap()
        .unwrap();
    action_by_title(
        &id_line_actions_without_client_diagnostics,
        "Assign unique note ID 'source'",
    );

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
    let alias_line_actions_without_client_diagnostics = state
        .code_action_request(
            source_uri.clone(),
            Range::new(
                Position::new(duplicate_alias.range.start.line, 0),
                Position::new(duplicate_alias.range.start.line, 0),
            ),
            Vec::new(),
        )
        .unwrap()
        .compute()
        .unwrap()
        .unwrap();
    action_by_title(
        &alias_line_actions_without_client_diagnostics,
        "Change duplicate alias 'shared' to 'shared-2'",
    );
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

fn completion_text_edit(item: &CompletionItem) -> &TextEdit {
    match item.text_edit.as_ref().expect("completion item should have text edit") {
        CompletionTextEdit::Edit(edit) => edit,
        _ => panic!("expected a plain TextEdit"),
    }
}

fn completion_text_edit_for_label<'a>(items: &'a [CompletionItem], label: &str) -> &'a TextEdit {
    let item = items
        .iter()
        .find(|item| item.label == label)
        .expect("completion item should exist");
    completion_text_edit(item)
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
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
fn completion_request_uses_lsp_ranges_for_wiki_completion_after_non_ascii_character() {
    let (_vault_dir, mut state, _target_uri, _other_uri) = completion_state();
    let source_path = _vault_dir.path().join("source.md");
    let text = "Intro — [[tar";
    fs::write(&source_path, text).unwrap();
    let source_path = source_path.canonicalize().unwrap();
    let source_uri = path_to_uri(&source_path).unwrap();
    state.open_document(source_uri.clone(), 1, text.to_string()).unwrap();

    let items = state
        .completion_request(source_uri, Position::new(0, utf16_len(text)))
        .unwrap()
        .compute()
        .unwrap()
        .unwrap();

    let edit = completion_text_edit_for_label(&items, "[[target-id]]");
    assert_eq!(edit.range.start.character, utf16_len("Intro — "));
    assert_eq!(edit.range.end.character, utf16_len(text));
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
fn completion_request_uses_lsp_ranges_for_markdown_completion_after_non_ascii_character() {
    let (_vault_dir, mut state, _target_uri, _other_uri) = completion_state();
    let source_path = _vault_dir.path().join("source.md");
    let text = "Intro — [tar";
    fs::write(&source_path, text).unwrap();
    let source_path = source_path.canonicalize().unwrap();
    let source_uri = path_to_uri(&source_path).unwrap();
    state.open_document(source_uri.clone(), 1, text.to_string()).unwrap();

    let items = state
        .completion_request(source_uri, Position::new(0, utf16_len(text)))
        .unwrap()
        .compute()
        .unwrap()
        .unwrap();

    let item = items
        .iter()
        .find(|item| item.label.starts_with("[target-id]("))
        .expect("completion item should exist");
    let edit = completion_text_edit(item);
    assert_eq!(edit.range.start.character, utf16_len("Intro — "));
    assert_eq!(edit.range.end.character, utf16_len(text));
}

#[test]
fn completion_request_uses_lsp_ranges_for_tag_completion_after_non_ascii_character() {
    let vault_dir = tempfile::tempdir().unwrap();
    fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

    let tagged_path = vault_dir.path().join("tagged.md");
    fs::write(&tagged_path, "---\nid: tagged\ntags: [project, work]\n---\n").unwrap();

    let source_path = vault_dir.path().join("source.md");
    let text = "Intro — #pro";
    fs::write(&source_path, text).unwrap();
    let source_path = source_path.canonicalize().unwrap();
    let source_uri = path_to_uri(&source_path).unwrap();

    let vault = Vault::open(vault_dir.path()).unwrap();
    let mut state = BackendState::new(vault);
    state.open_document(source_uri.clone(), 1, text.to_string()).unwrap();

    let items = state
        .completion_request(source_uri, Position::new(0, utf16_len(text)))
        .unwrap()
        .compute()
        .unwrap()
        .unwrap();

    let labels = completion_labels(&items);
    assert!(labels.contains(&"#project"), "missing #project");
    let edit = completion_text_edit_for_label(&items, "#project");
    assert_eq!(edit.range.start.character, utf16_len("Intro — "));
    assert_eq!(edit.range.end.character, utf16_len(text));
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
