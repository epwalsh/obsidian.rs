use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tower_lsp::lsp_types::Url;

const MESSAGE_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

struct LspHarness {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<Value>,
    backlog: VecDeque<Value>,
    stderr: Arc<Mutex<String>>,
}

impl LspHarness {
    fn spawn(vault_path: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_obsidian-lsp"))
            .arg("--vault")
            .arg(vault_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("should spawn obsidian-lsp");

        let stdin = child.stdin.take().expect("child stdin should be piped");
        let stdout = child.stdout.take().expect("child stdout should be piped");
        let stderr = child.stderr.take().expect("child stderr should be piped");

        let (sender, receiver) = mpsc::channel();
        let stderr_output = Arc::new(Mutex::new(String::new()));

        spawn_stdout_reader(stdout, sender);
        spawn_stderr_reader(stderr, Arc::clone(&stderr_output));

        Self {
            child,
            stdin: Some(stdin),
            messages: receiver,
            backlog: VecDeque::new(),
            stderr: stderr_output,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("stdin should still be open");
        let bytes = serde_json::to_vec(&message).expect("message should serialize");
        write!(stdin, "Content-Length: {}\r\n\r\n", bytes.len()).expect("should write LSP header");
        stdin.write_all(&bytes).expect("should write LSP body");
        stdin.flush().expect("should flush message");
    }

    fn expect_message<F>(&mut self, description: &str, mut predicate: F) -> Value
    where
        F: FnMut(&Value) -> bool,
    {
        if let Some(index) = self.backlog.iter().position(&mut predicate) {
            return self
                .backlog
                .remove(index)
                .expect("backlog entry at reported index should exist");
        }

        let deadline = Instant::now() + MESSAGE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self.messages.recv_timeout(remaining).unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for {description}; stderr so far:\n{}",
                    self.stderr_snapshot()
                )
            });

            if predicate(&message) {
                return message;
            }

            self.backlog.push_back(message);
        }
    }

    fn wait_for_exit(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            match self.child.try_wait().expect("should query child status") {
                Some(status) => {
                    assert!(
                        status.success(),
                        "obsidian-lsp exited unsuccessfully ({status}); stderr:\n{}",
                        self.stderr_snapshot()
                    );
                    return;
                }
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!(
                        "timed out waiting for obsidian-lsp to exit; stderr:\n{}",
                        self.stderr_snapshot()
                    );
                }
            }
        }
    }

    fn stderr_snapshot(&self) -> String {
        self.stderr.lock().expect("stderr lock should not be poisoned").clone()
    }
}

impl Drop for LspHarness {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn spawn_stdout_reader(stdout: ChildStdout, sender: mpsc::Sender<Value>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(message) = read_message(&mut reader) {
            if sender.send(message).is_err() {
                return;
            }
        }
    });
}

fn spawn_stderr_reader(stderr: ChildStderr, output: Arc<Mutex<String>>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) => output
                    .lock()
                    .expect("stderr lock should not be poisoned")
                    .push_str(&line),
                Err(_) => return,
            }
        }
    });
}

fn read_message(reader: &mut BufReader<ChildStdout>) -> Option<Value> {
    let mut content_length = None;
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {
                if line == "\r\n" {
                    break;
                }

                let (name, value) = line
                    .split_once(':')
                    .expect("LSP headers should contain a ':' delimiter");
                if name.eq_ignore_ascii_case("Content-Length") {
                    content_length = Some(
                        value
                            .trim()
                            .parse::<usize>()
                            .expect("Content-Length header should contain a number"),
                    );
                }
            }
            Err(_) => return None,
        }
    }

    let mut content = vec![0; content_length.expect("LSP messages should include Content-Length header")];
    reader
        .read_exact(&mut content)
        .expect("should read the full JSON-RPC payload");

    Some(serde_json::from_slice(&content).expect("payload should be valid JSON"))
}

fn request(id: i64, method: &str, params: Option<Value>) -> Value {
    let mut message = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
    });

    if let Some(params) = params {
        message
            .as_object_mut()
            .expect("request message should be an object")
            .insert("params".to_string(), params);
    }

    message
}

fn notification(method: &str, params: Option<Value>) -> Value {
    let mut message = json!({
        "jsonrpc": "2.0",
        "method": method,
    });

    if let Some(params) = params {
        message
            .as_object_mut()
            .expect("notification message should be an object")
            .insert("params".to_string(), params);
    }

    message
}

fn initialize_session(harness: &mut LspHarness, vault_uri: &Url, vault_path: &Path) {
    harness.send(request(
        1,
        "initialize",
        Some(json!({
            "processId": null,
            "rootUri": vault_uri,
            "capabilities": {},
        })),
    ));

    let initialize = harness.expect_message("initialize response", |message| message["id"] == 1);
    assert_eq!(initialize["jsonrpc"], "2.0");
    assert_eq!(initialize["result"]["capabilities"]["textDocumentSync"], 1);
    assert_eq!(initialize["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(initialize["result"]["capabilities"]["referencesProvider"], true);
    assert_eq!(initialize["result"]["capabilities"]["definitionProvider"], true);
    assert_eq!(
        initialize["result"]["capabilities"]["documentLinkProvider"]["resolveProvider"],
        true
    );
    assert_eq!(initialize["result"]["serverInfo"]["name"], "obsidian-rs-lsp");
    assert_eq!(initialize["result"]["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));

    harness.send(notification("initialized", Some(json!({}))));

    let log_message = harness.expect_message("window/logMessage notification", |message| {
        message["method"] == "window/logMessage"
    });
    assert_eq!(log_message["params"]["type"], 3);
    assert!(
        log_message["params"]["message"]
            .as_str()
            .expect("log message should be a string")
            .contains(vault_path.to_string_lossy().as_ref())
    );
}

fn shutdown_session(harness: &mut LspHarness) {
    harness.send(request(99, "shutdown", None));

    let shutdown = harness.expect_message("shutdown response", |message| message["id"] == 99);
    assert_eq!(shutdown["jsonrpc"], "2.0");
    assert!(shutdown["result"].is_null());

    harness.send(notification("exit", None));
    harness.wait_for_exit();
}

fn expect_diagnostics(harness: &mut LspHarness, uri: &Url, version: Option<i32>) -> Value {
    harness.expect_message("publishDiagnostics notification", |message| {
        if message["method"] != "textDocument/publishDiagnostics" || message["params"]["uri"] != uri.as_str() {
            return false;
        }

        match version {
            Some(version) => message["params"]["version"] == json!(version),
            None => message["params"]
                .as_object()
                .expect("diagnostics params should be an object")
                .get("version")
                .is_none(),
        }
    })
}

fn diagnostic_codes(message: &Value) -> Vec<&str> {
    message["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics should be an array")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect()
}

fn raw_link(document_link: &Value) -> &str {
    document_link["data"]["rawLink"]
        .as_str()
        .expect("document link should include rawLink data")
}

fn position_for_substring(text: &str, needle: &str) -> (u32, u32) {
    for (line_index, line) in text.lines().enumerate() {
        if let Some(column) = line.find(needle) {
            return (line_index as u32, column as u32 + 2);
        }
    }

    panic!("substring '{needle}' not found");
}

fn create_test_vault() -> (tempfile::TempDir, PathBuf, Url) {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

    let note_path = vault_dir.path().join("notes/today.md");
    fs::create_dir_all(note_path.parent().expect("note should have a parent")).expect("should create note directory");
    fs::write(&note_path, "original body").expect("should write test note");

    let note_path = note_path.canonicalize().expect("note path should canonicalize");
    let note_uri = Url::from_file_path(&note_path).expect("note path should convert to file URI");

    (vault_dir, note_path, note_uri)
}

fn create_feature_vault() -> (tempfile::TempDir, Url, Url, Url, Url, String) {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

    let target_path = vault_dir.path().join("target.md");
    fs::write(
        &target_path,
        "---\nid: target-id\ntitle: Target Note\naliases: [target-alias]\ntags: [rust]\n---\n\nBody.\n",
    )
    .expect("should write target note");
    let target_uri = Url::from_file_path(target_path.canonicalize().expect("target path should canonicalize"))
        .expect("target path should convert to file URI");

    let duplicate_a_path = vault_dir.path().join("duplicate-a.md");
    let duplicate_a_text = "---\nid: shared-id\naliases: [shared-alias]\n---\n\nSee [[missing-note]], [[target-id]], and [Target Markdown](target.md).\n";
    fs::write(&duplicate_a_path, duplicate_a_text).expect("should write duplicate note A");
    let duplicate_a_uri = Url::from_file_path(
        duplicate_a_path
            .canonicalize()
            .expect("duplicate note A path should canonicalize"),
    )
    .expect("duplicate note A path should convert to file URI");

    let duplicate_b_path = vault_dir.path().join("duplicate-b.md");
    fs::write(
        &duplicate_b_path,
        "---\nid: shared-id\naliases: [shared-alias]\n---\n\nBody.\n",
    )
    .expect("should write duplicate note B");
    let duplicate_b_uri = Url::from_file_path(
        duplicate_b_path
            .canonicalize()
            .expect("duplicate note B path should canonicalize"),
    )
    .expect("duplicate note B path should convert to file URI");

    let backlink_path = vault_dir.path().join("backlink.md");
    fs::write(&backlink_path, "Another reference to [[target-id]].\n").expect("should write backlink note");
    let backlink_uri = Url::from_file_path(
        backlink_path
            .canonicalize()
            .expect("backlink note path should canonicalize"),
    )
    .expect("backlink note path should convert to file URI");

    (
        vault_dir,
        duplicate_a_uri,
        duplicate_b_uri,
        target_uri,
        backlink_uri,
        duplicate_a_text.to_string(),
    )
}

fn create_heading_anchor_vault() -> (tempfile::TempDir, Url, Url, String) {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

    let target_path = vault_dir.path().join("target.md");
    fs::write(
        &target_path,
        "---\nid: target-id\ntitle: Target Note\n---\n\n# Overview\n\n## Linked Heading\nBody.\n",
    )
    .expect("should write target note");
    let target_uri = Url::from_file_path(target_path.canonicalize().expect("target path should canonicalize"))
        .expect("target path should convert to file URI");

    let source_path = vault_dir.path().join("source.md");
    let source_text = "See [[target-id#Linked Heading]] and [Target Markdown](target.md#linked-heading).\n";
    fs::write(&source_path, source_text).expect("should write source note");
    let source_uri = Url::from_file_path(source_path.canonicalize().expect("source path should canonicalize"))
        .expect("source path should convert to file URI");

    (vault_dir, source_uri, target_uri, source_text.to_string())
}

fn create_nested_heading_anchor_vault() -> (tempfile::TempDir, Url, Url, String) {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

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
    .expect("should write target note");
    let target_uri = Url::from_file_path(target_path.canonicalize().expect("target path should canonicalize"))
        .expect("target path should convert to file URI");

    let source_path = vault_dir.path().join("source.md");
    let source_text = concat!(
        "See [[target-id#Heading A#Subheading B]], ",
        "[[target-id#heading-a#subheading-b]], ",
        "and [Target Markdown](target.md#heading-a#subheading-b).\n"
    );
    fs::write(&source_path, source_text).expect("should write source note");
    let source_uri = Url::from_file_path(source_path.canonicalize().expect("source path should canonicalize"))
        .expect("source path should convert to file URI");

    (vault_dir, source_uri, target_uri, source_text.to_string())
}

#[test]
fn stdio_session_handles_initialize_and_document_lifecycle() {
    let (vault_dir, note_path, note_uri) = create_test_vault();
    let vault_path = vault_dir.path().canonicalize().expect("vault path should canonicalize");
    let vault_uri = Url::from_file_path(&vault_path).expect("vault path should convert to file URI");

    let mut harness = LspHarness::spawn(vault_dir.path());
    initialize_session(&mut harness, &vault_uri, &vault_path);

    harness.send(notification(
        "textDocument/didOpen",
        Some(json!({
            "textDocument": {
                "uri": note_uri,
                "languageId": "markdown",
                "version": 1,
                "text": "opened body",
            }
        })),
    ));

    let open_diagnostics = harness.expect_message("didOpen diagnostics", |message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == note_uri.as_str()
            && message["params"]["version"] == 1
    });
    assert_eq!(open_diagnostics["params"]["diagnostics"], json!([]));

    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": {
                "uri": note_uri,
                "version": 2,
            },
            "contentChanges": [
                {
                    "text": "changed body",
                }
            ],
        })),
    ));

    let change_diagnostics = harness.expect_message("didChange diagnostics", |message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == note_uri.as_str()
            && message["params"]["version"] == 2
    });
    assert_eq!(change_diagnostics["params"]["diagnostics"], json!([]));

    harness.send(notification(
        "textDocument/didClose",
        Some(json!({
            "textDocument": {
                "uri": note_uri,
            }
        })),
    ));

    let close_diagnostics = harness.expect_message("didClose diagnostics", |message| {
        message["method"] == "textDocument/publishDiagnostics" && message["params"]["uri"] == note_uri.as_str()
    });
    assert_eq!(close_diagnostics["params"]["diagnostics"], json!([]));
    assert!(
        close_diagnostics["params"]
            .as_object()
            .expect("diagnostics params should be an object")
            .get("version")
            .is_none()
    );

    shutdown_session(&mut harness);

    let final_body = fs::read_to_string(note_path).expect("should be able to read note after LSP session");
    assert_eq!(final_body, "original body");
}

#[test]
fn stdio_session_reports_health_diagnostics_and_hover_metadata() {
    let (vault_dir, duplicate_a_uri, duplicate_b_uri, target_uri, backlink_uri, duplicate_a_text) =
        create_feature_vault();
    let vault_path = vault_dir.path().canonicalize().expect("vault path should canonicalize");
    let vault_uri = Url::from_file_path(&vault_path).expect("vault path should convert to file URI");
    let (hover_line, hover_character) = position_for_substring(&duplicate_a_text, "[[target-id]]");

    let mut harness = LspHarness::spawn(vault_dir.path());
    initialize_session(&mut harness, &vault_uri, &vault_path);

    harness.send(notification(
        "textDocument/didOpen",
        Some(json!({
            "textDocument": {
                "uri": duplicate_a_uri,
                "languageId": "markdown",
                "version": 1,
                "text": duplicate_a_text,
            }
        })),
    ));

    let duplicate_a_diagnostics = expect_diagnostics(&mut harness, &duplicate_a_uri, Some(1));
    assert_eq!(
        diagnostic_codes(&duplicate_a_diagnostics),
        vec!["duplicate-id", "duplicate-alias", "broken-link"]
    );
    assert!(
        duplicate_a_diagnostics["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics should be an array")
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .unwrap()
                .contains("Broken link [[missing-note]]"))
    );

    let duplicate_b_diagnostics = expect_diagnostics(&mut harness, &duplicate_b_uri, None);
    assert_eq!(
        diagnostic_codes(&duplicate_b_diagnostics),
        vec!["duplicate-id", "duplicate-alias"]
    );

    harness.send(request(
        2,
        "textDocument/hover",
        Some(json!({
            "textDocument": {
                "uri": duplicate_a_uri,
            },
            "position": {
                "line": hover_line,
                "character": hover_character,
            }
        })),
    ));

    let hover = harness.expect_message("hover response", |message| message["id"] == 2);
    let hover_text = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover response should contain markdown text");
    assert!(hover_text.contains("Target Note"));
    assert!(hover_text.contains("target-id"));
    assert!(hover_text.contains("target-alias"));
    assert!(hover_text.contains("rust"));
    assert!(hover_text.contains("target.md"));

    harness.send(request(
        3,
        "textDocument/documentLink",
        Some(json!({
            "textDocument": {
                "uri": duplicate_a_uri,
            }
        })),
    ));

    let document_links = harness.expect_message("documentLink response", |message| message["id"] == 3);
    let document_links = document_links["result"]
        .as_array()
        .expect("documentLink response should be an array");
    assert_eq!(document_links.len(), 3);
    assert!(
        document_links
            .iter()
            .all(|document_link| { document_link.get("target").is_none_or(|target| target.is_null()) })
    );

    let wiki_link = document_links
        .iter()
        .find(|document_link| raw_link(document_link) == "[[target-id]]")
        .cloned()
        .expect("documentLink response should include the wiki note link");
    let markdown_link = document_links
        .iter()
        .find(|document_link| raw_link(document_link) == "[Target Markdown](target.md)")
        .cloned()
        .expect("documentLink response should include the markdown note link");

    harness.send(request(4, "documentLink/resolve", Some(wiki_link)));
    let resolved_wiki_link = harness.expect_message("documentLink/resolve wiki response", |message| message["id"] == 4);
    assert_eq!(resolved_wiki_link["result"]["target"], target_uri.as_str());
    assert!(
        resolved_wiki_link["result"]["tooltip"]
            .as_str()
            .expect("resolved document link should include a tooltip")
            .contains("Target Note")
    );

    harness.send(request(5, "documentLink/resolve", Some(markdown_link)));
    let resolved_markdown_link =
        harness.expect_message("documentLink/resolve markdown response", |message| message["id"] == 5);
    assert_eq!(resolved_markdown_link["result"]["target"], target_uri.as_str());

    harness.send(request(
        6,
        "textDocument/references",
        Some(json!({
            "textDocument": {
                "uri": duplicate_a_uri,
            },
            "position": {
                "line": hover_line,
                "character": hover_character,
            },
            "context": {
                "includeDeclaration": true,
            }
        })),
    ));

    let link_references = harness.expect_message("references response", |message| message["id"] == 6);
    let link_references = link_references["result"]
        .as_array()
        .expect("references response should be an array");
    assert_eq!(link_references.len(), 3);
    assert_eq!(
        link_references
            .iter()
            .filter(|location| location["uri"] == duplicate_a_uri.as_str())
            .count(),
        2
    );
    assert!(
        link_references
            .iter()
            .any(|location| location["uri"] == backlink_uri.as_str())
    );

    harness.send(request(
        7,
        "textDocument/references",
        Some(json!({
            "textDocument": {
                "uri": target_uri,
            },
            "position": {
                "line": 0,
                "character": 0,
            },
            "context": {
                "includeDeclaration": true,
            }
        })),
    ));

    let note_references = harness.expect_message("off-link references response", |message| message["id"] == 7);
    let note_references = note_references["result"]
        .as_array()
        .expect("off-link references response should be an array");
    assert_eq!(note_references.len(), 3);

    harness.send(request(
        8,
        "textDocument/definition",
        Some(json!({
            "textDocument": {
                "uri": duplicate_a_uri,
            },
            "position": {
                "line": hover_line,
                "character": hover_character,
            }
        })),
    ));

    let definition = harness.expect_message("definition response", |message| message["id"] == 8);
    assert_eq!(definition["result"]["uri"], target_uri.as_str());
    assert_eq!(definition["result"]["range"]["start"]["line"], 2);

    shutdown_session(&mut harness);
}

#[test]
fn stdio_session_definition_jumps_to_heading_anchor() {
    let (vault_dir, source_uri, target_uri, source_text) = create_heading_anchor_vault();
    let vault_uri = Url::from_directory_path(vault_dir.path()).expect("vault path should convert to file URI");
    let mut harness = LspHarness::spawn(vault_dir.path());

    initialize_session(&mut harness, &vault_uri, vault_dir.path());

    harness.send(notification(
        "textDocument/didOpen",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
                "languageId": "markdown",
                "version": 1,
                "text": source_text,
            }
        })),
    ));

    harness.send(request(
        2,
        "textDocument/definition",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
            },
            "position": {
                "line": position_for_substring(&source_text, "[[target-id#Linked Heading]]").0,
                "character": position_for_substring(&source_text, "[[target-id#Linked Heading]]").1,
            }
        })),
    ));

    let wiki_definition = harness.expect_message("wiki heading definition response", |message| message["id"] == 2);
    let wiki_definition_uri = Url::parse(
        wiki_definition["result"]["uri"]
            .as_str()
            .expect("definition response should include a target URI"),
    )
    .expect("definition target should be a valid URI");
    assert_eq!(wiki_definition_uri.path(), target_uri.path());
    assert_eq!(wiki_definition_uri.fragment(), Some("Linked%20Heading"));
    assert_eq!(wiki_definition["result"]["range"]["start"]["line"], 7);
    assert_eq!(wiki_definition["result"]["range"]["start"]["character"], 3);

    harness.send(request(
        3,
        "textDocument/definition",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
            },
            "position": {
                "line": position_for_substring(&source_text, "[Target Markdown](target.md#linked-heading)").0,
                "character": position_for_substring(&source_text, "[Target Markdown](target.md#linked-heading)").1,
            }
        })),
    ));

    let markdown_definition =
        harness.expect_message("markdown heading definition response", |message| message["id"] == 3);
    let markdown_definition_uri = Url::parse(
        markdown_definition["result"]["uri"]
            .as_str()
            .expect("definition response should include a target URI"),
    )
    .expect("definition target should be a valid URI");
    assert_eq!(markdown_definition_uri.path(), target_uri.path());
    assert_eq!(markdown_definition_uri.fragment(), Some("linked-heading"));
    assert_eq!(markdown_definition["result"]["range"]["start"]["line"], 7);
    assert_eq!(markdown_definition["result"]["range"]["start"]["character"], 3);

    shutdown_session(&mut harness);
}

#[test]
fn stdio_session_definition_jumps_to_nested_heading_anchor() {
    let (vault_dir, source_uri, target_uri, source_text) = create_nested_heading_anchor_vault();
    let vault_uri = Url::from_directory_path(vault_dir.path()).expect("vault path should convert to file URI");
    let mut harness = LspHarness::spawn(vault_dir.path());

    initialize_session(&mut harness, &vault_uri, vault_dir.path());

    harness.send(notification(
        "textDocument/didOpen",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
                "languageId": "markdown",
                "version": 1,
                "text": source_text,
            }
        })),
    ));

    let (raw_wiki_line, raw_wiki_character) =
        position_for_substring(&source_text, "[[target-id#Heading A#Subheading B]]");
    harness.send(request(
        2,
        "textDocument/definition",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
            },
            "position": {
                "line": raw_wiki_line,
                "character": raw_wiki_character,
            }
        })),
    ));

    let raw_wiki_definition =
        harness.expect_message("nested wiki heading definition response", |message| message["id"] == 2);
    let raw_wiki_definition_uri = Url::parse(
        raw_wiki_definition["result"]["uri"]
            .as_str()
            .expect("definition response should include a target URI"),
    )
    .expect("definition target should be a valid URI");
    assert_eq!(raw_wiki_definition_uri.path(), target_uri.path());
    assert_eq!(raw_wiki_definition_uri.fragment(), Some("Heading%20A#Subheading%20B"));
    assert_eq!(raw_wiki_definition["result"]["range"]["start"]["line"], 11);
    assert_eq!(raw_wiki_definition["result"]["range"]["start"]["character"], 3);

    let (slug_wiki_line, slug_wiki_character) =
        position_for_substring(&source_text, "[[target-id#heading-a#subheading-b]]");
    harness.send(request(
        3,
        "textDocument/definition",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
            },
            "position": {
                "line": slug_wiki_line,
                "character": slug_wiki_character,
            }
        })),
    ));

    let slug_wiki_definition = harness.expect_message("nested slug wiki heading definition response", |message| {
        message["id"] == 3
    });
    let slug_wiki_definition_uri = Url::parse(
        slug_wiki_definition["result"]["uri"]
            .as_str()
            .expect("definition response should include a target URI"),
    )
    .expect("definition target should be a valid URI");
    assert_eq!(slug_wiki_definition_uri.path(), target_uri.path());
    assert_eq!(slug_wiki_definition_uri.fragment(), Some("heading-a#subheading-b"));
    assert_eq!(slug_wiki_definition["result"]["range"]["start"]["line"], 11);
    assert_eq!(slug_wiki_definition["result"]["range"]["start"]["character"], 3);

    let (markdown_line, markdown_character) =
        position_for_substring(&source_text, "[Target Markdown](target.md#heading-a#subheading-b)");
    harness.send(request(
        4,
        "textDocument/definition",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
            },
            "position": {
                "line": markdown_line,
                "character": markdown_character,
            }
        })),
    ));

    let markdown_definition = harness.expect_message("nested markdown heading definition response", |message| {
        message["id"] == 4
    });
    let markdown_definition_uri = Url::parse(
        markdown_definition["result"]["uri"]
            .as_str()
            .expect("definition response should include a target URI"),
    )
    .expect("definition target should be a valid URI");
    assert_eq!(markdown_definition_uri.path(), target_uri.path());
    assert_eq!(markdown_definition_uri.fragment(), Some("heading-a#subheading-b"));
    assert_eq!(markdown_definition["result"]["range"]["start"]["line"], 11);
    assert_eq!(markdown_definition["result"]["range"]["start"]["character"], 3);

    shutdown_session(&mut harness);
}

fn create_completion_vault() -> (tempfile::TempDir, Url, Url) {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

    let target_path = vault_dir.path().join("target.md");
    fs::write(
        &target_path,
        "---\nid: target-id\ntitle: Target Note\naliases: [Target Alias]\n---\n\n# Overview\n\n## Getting Started\n\nBody.\n",
    )
    .expect("should write target note");
    let target_uri = Url::from_file_path(target_path.canonicalize().expect("target path should canonicalize"))
        .expect("target path should convert to file URI");

    let other_path = vault_dir.path().join("other.md");
    fs::write(&other_path, "---\nid: other-note\n---\n\nBody.\n").expect("should write other note");
    let other_uri = Url::from_file_path(other_path.canonicalize().expect("other path should canonicalize"))
        .expect("other path should convert to file URI");

    (vault_dir, target_uri, other_uri)
}

fn completion_labels(response: &Value) -> Vec<&str> {
    response["result"]
        .as_array()
        .expect("completion result should be an array")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect()
}

#[test]
fn stdio_session_handles_completion_for_wiki_and_markdown_links() {
    let (vault_dir, _target_uri, _other_uri) = create_completion_vault();
    let vault_path = vault_dir.path().canonicalize().expect("vault path should canonicalize");
    let vault_uri = Url::from_file_path(&vault_path).expect("vault path should convert to file URI");

    let mut harness = LspHarness::spawn(vault_dir.path());
    initialize_session(&mut harness, &vault_uri, &vault_path);

    let source_path = vault_dir.path().join("source.md");
    let wiki_text = "See [[tar";
    fs::write(&source_path, wiki_text).expect("should write source note");
    let source_path = source_path.canonicalize().expect("source path should canonicalize");
    let source_uri = Url::from_file_path(&source_path).expect("source path should convert to file URI");

    // Open document with a partial wiki link "[[tar"
    harness.send(notification(
        "textDocument/didOpen",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
                "languageId": "markdown",
                "version": 1,
                "text": wiki_text,
            }
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(1));

    // Request completion at the end of "[[tar" (line 0, character 9)
    harness.send(request(
        2,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 0, "character": 9 },
        })),
    ));

    let wiki_completion = harness.expect_message("wiki completion response", |message| message["id"] == 2);
    let labels = completion_labels(&wiki_completion);
    assert!(labels.contains(&"[[target-id]]"), "missing [[target-id]]");
    assert!(labels.contains(&"[[Target Note]]"), "missing [[Target Note]]");
    assert!(
        labels.contains(&"[[target-id|Target Alias]]"),
        "missing [[target-id|Target Alias]]"
    );
    assert!(labels.contains(&"[[Target Alias]]"), "missing [[Target Alias]]");
    assert!(!labels.contains(&"[[other-note]]"), "other-note should not match 'tar'");

    // Verify text_edit replaces the correct range (from the `[[` at char 4 to cursor at char 9)
    let first_item = &wiki_completion["result"][0];
    assert_eq!(first_item["textEdit"]["range"]["start"]["character"], 4);
    assert_eq!(first_item["textEdit"]["range"]["end"]["character"], 9);

    // Change document to a partial markdown link "[tar"
    let markdown_text = "See [tar";
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 2 },
            "contentChanges": [{ "text": markdown_text }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(2));

    // Request completion at the end of "[tar" (line 0, character 8)
    harness.send(request(
        3,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 0, "character": 8 },
        })),
    ));

    let md_completion = harness.expect_message("markdown completion response", |message| message["id"] == 3);
    let labels = completion_labels(&md_completion);
    assert!(
        labels.iter().any(|l| l.starts_with("[target-id](") && l.ends_with(')')),
        "missing [target-id](...)"
    );
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with("[Target Note](") && l.ends_with(')')),
        "missing [Target Note](...)"
    );
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with("[Target Alias](") && l.ends_with(')')),
        "missing [Target Alias](...)"
    );
    assert!(
        !labels.iter().any(|l| l.starts_with("[other-note](")),
        "other-note should not match 'tar'"
    );

    // Request completion on plain text — should return null (no link context).
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 3 },
            "contentChanges": [{ "text": "just plain text" }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(3));

    harness.send(request(
        4,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 0, "character": 15 },
        })),
    ));

    let plain_completion = harness.expect_message("plain text completion response", |message| message["id"] == 4);
    assert!(
        plain_completion["result"].is_null(),
        "plain text should return null, not completions"
    );

    // Test heading completion: "[[target-id#Get" should produce heading completions.
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 4 },
            "contentChanges": [{ "text": "[[target-id#Get" }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(4));

    harness.send(request(
        5,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 0, "character": 15 },
        })),
    ));

    let heading_completion = harness.expect_message("heading completion response", |message| message["id"] == 5);
    let labels = completion_labels(&heading_completion);
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
        "Overview should not match 'Get'"
    );

    // Test closing-bracket extension: cursor inside "[[tar]]" should replace the whole thing.
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 5 },
            "contentChanges": [{ "text": "[[tar]]" }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(5));

    // position 5: after "[[tar", before "]]"
    harness.send(request(
        6,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 0, "character": 5 },
        })),
    ));

    let bracket_completion =
        harness.expect_message("closing-bracket completion response", |message| message["id"] == 6);
    let first_item = bracket_completion["result"]
        .as_array()
        .expect("result should be an array")
        .iter()
        .find(|item| item["label"] == "[[target-id]]")
        .expect("[[target-id]] should be in the list");
    assert_eq!(
        first_item["textEdit"]["range"]["start"]["character"], 0,
        "range should start at [["
    );
    assert_eq!(
        first_item["textEdit"]["range"]["end"]["character"], 7,
        "range should extend past ]] to position 7"
    );

    // Test anchor-only completion: "[[#Get" completes headings within the current document only.
    // The source document must have its own headings for this to work.
    let anchor_text = "# Overview\n\n## Getting Started\n\n[[#Get";
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 6 },
            "contentChanges": [{ "text": anchor_text }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(6));

    // cursor at line 4, char 6 — end of "[[#Get"
    harness.send(request(
        7,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 4, "character": 6 },
        })),
    ));

    let anchor_completion = harness.expect_message("anchor completion response", |message| message["id"] == 7);
    let labels = completion_labels(&anchor_completion);
    assert!(labels.contains(&"[[#Getting Started]]"), "missing [[#Getting Started]]");
    assert!(
        !labels.iter().any(|l| l.contains("Overview")),
        "Overview should not match 'Get'"
    );
    // All items must be anchor-only (start with [[#), never [[other-note#...]]
    assert!(
        labels.iter().all(|l| l.starts_with("[[#")),
        "all labels should be anchor-only [[#...]] form"
    );

    shutdown_session(&mut harness);
}

#[test]
fn stdio_session_offers_create_note_code_action_for_broken_links() {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

    let source_path = vault_dir.path().join("source.md");
    let source_text = "See [[missing-note|Missing Note]] and [Missing Markdown](missing-markdown.md).";
    fs::write(&source_path, source_text).expect("should write source note");
    let source_path = source_path.canonicalize().expect("source path should canonicalize");
    let source_uri = Url::from_file_path(&source_path).expect("source path should convert to URI");

    let vault_uri = Url::from_file_path(vault_dir.path()).expect("vault path should convert to URI");

    let mut harness = LspHarness::spawn(vault_dir.path());
    initialize_session(&mut harness, &vault_uri, vault_dir.path());

    harness.send(notification(
        "textDocument/didOpen",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
                "languageId": "markdown",
                "version": 1,
                "text": source_text,
            },
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(1));

    // Code action for wiki link [[missing-note|Missing Note]]: cursor on "missing-note"
    let (wiki_line, wiki_char) = position_for_substring(source_text, "[[missing-note|Missing Note]]");
    harness.send(request(
        2,
        "textDocument/codeAction",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "range": {
                "start": { "line": wiki_line, "character": wiki_char },
                "end":   { "line": wiki_line, "character": wiki_char },
            },
            "context": { "diagnostics": [] },
        })),
    ));

    let wiki_action_response = harness.expect_message("wiki code action response", |message| message["id"] == 2);
    let actions = wiki_action_response["result"]
        .as_array()
        .expect("code action result should be an array");
    assert!(!actions.is_empty(), "should have at least one code action");

    let create_action = actions
        .iter()
        .find(|a| a["title"].as_str().map_or(false, |t| t.contains("missing-note")))
        .expect("should have a create action for 'missing-note'");

    assert_eq!(create_action["kind"], "quickfix");
    // Action has a TextDocumentEdit for preview and a command for actual execution.
    assert_eq!(create_action["command"]["command"], "obsidian.createNote");
    let text_edit_changes = create_action["edit"]["documentChanges"]
        .as_array()
        .expect("edit should have documentChanges for preview");
    let preview_edit = text_edit_changes
        .iter()
        .find(|op| {
            op["textDocument"]["uri"]
                .as_str()
                .map_or(false, |u| u.ends_with("missing-note.md"))
        })
        .expect("should have a TextDocumentEdit for the new file");
    let preview_text = preview_edit["edits"][0]["newText"]
        .as_str()
        .expect("TextDocumentEdit should have newText");
    assert_eq!(
        preview_text,
        "---\nid: missing-note\naliases:\n- Missing Note\n---\n\n# Missing Note\n"
    );

    let wiki_path_arg = create_action["command"]["arguments"][0]
        .as_str()
        .expect("command should have a path argument");
    assert_eq!(
        create_action["command"]["arguments"][1], "Missing Note",
        "command should pass the wiki alias through as the note title"
    );
    assert!(
        wiki_path_arg.ends_with("missing-note.md"),
        "command argument should point to missing-note.md, got: {wiki_path_arg}"
    );

    // The new file should be a sibling of source.md (same directory)
    let source_parent = source_path.parent().expect("source should have a parent");
    let expected_new_path = source_parent.join("missing-note.md");
    assert_eq!(wiki_path_arg, expected_new_path.to_string_lossy().as_ref());

    // Execute the command — server creates the file and refreshes diagnostics.
    harness.send(request(
        3,
        "workspace/executeCommand",
        Some(json!({ "command": "obsidian.createNote", "arguments": [wiki_path_arg, "Missing Note"] })),
    ));
    harness.expect_message("executeCommand response", |message| message["id"] == 3);
    // Diagnostics are refreshed after the note is created (still version 1 of the document).
    expect_diagnostics(&mut harness, &source_uri, Some(1));

    let new_note_content =
        fs::read_to_string(&expected_new_path).expect("new note should exist on disk after executeCommand");
    assert_eq!(
        new_note_content, "---\nid: missing-note\naliases:\n- Missing Note\n---\n\n# Missing Note\n",
        "new note should use the wiki alias as its primary alias and heading"
    );

    // Code action for markdown link [Missing Markdown](missing-markdown.md)
    // Re-open the original broken-link document to trigger a fresh action.
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 2 },
            "contentChanges": [{ "text": source_text }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(2));

    let (md_line, md_char) = position_for_substring(source_text, "[Missing Markdown](missing-markdown.md)");
    harness.send(request(
        4,
        "textDocument/codeAction",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "range": {
                "start": { "line": md_line, "character": md_char },
                "end":   { "line": md_line, "character": md_char },
            },
            "context": { "diagnostics": [] },
        })),
    ));

    let md_action_response = harness.expect_message("markdown code action response", |message| message["id"] == 4);
    let md_actions = md_action_response["result"]
        .as_array()
        .expect("markdown code action result should be an array");
    assert!(
        !md_actions.is_empty(),
        "should have a code action for the markdown link"
    );

    let md_create_action = md_actions
        .iter()
        .find_map(|a| {
            let p = a["command"]["arguments"][0].as_str()?;
            p.ends_with("missing-markdown.md").then_some(a)
        })
        .expect("should have a createNote command for 'missing-markdown.md'");
    let md_path_arg = md_create_action["command"]["arguments"][0]
        .as_str()
        .expect("markdown command should have a path argument");

    let vault_canonical = vault_dir.path().canonicalize().expect("vault path should canonicalize");
    let expected_md_path = vault_canonical.join("missing-markdown.md");
    assert_eq!(
        md_path_arg,
        expected_md_path.to_string_lossy().as_ref(),
        "markdown link should create relative to vault root"
    );
    assert_eq!(
        md_create_action["command"]["arguments"][1], "Missing Markdown",
        "command should pass the markdown link text through as the note title"
    );

    let md_text_edit_changes = md_create_action["edit"]["documentChanges"]
        .as_array()
        .expect("markdown edit should have documentChanges for preview");
    let md_preview_edit = md_text_edit_changes
        .iter()
        .find(|op| {
            op["textDocument"]["uri"]
                .as_str()
                .map_or(false, |u| u.ends_with("missing-markdown.md"))
        })
        .expect("should have a TextDocumentEdit for the markdown new file");
    let md_preview_text = md_preview_edit["edits"][0]["newText"]
        .as_str()
        .expect("markdown TextDocumentEdit should have newText");
    assert_eq!(
        md_preview_text,
        "---\nid: missing-markdown\naliases:\n- Missing Markdown\n---\n\n# Missing Markdown\n"
    );

    harness.send(request(
        5,
        "workspace/executeCommand",
        Some(json!({ "command": "obsidian.createNote", "arguments": [md_path_arg, "Missing Markdown"] })),
    ));
    harness.expect_message("markdown executeCommand response", |message| message["id"] == 5);

    let md_note_content =
        fs::read_to_string(&expected_md_path).expect("markdown note should exist on disk after executeCommand");
    assert_eq!(
        md_note_content, "---\nid: missing-markdown\naliases:\n- Missing Markdown\n---\n\n# Missing Markdown\n",
        "new markdown note should use link text as its primary alias and heading"
    );

    // No code action for an existing note.
    let existing_path = vault_dir.path().join("existing.md");
    fs::write(&existing_path, "---\nid: existing\n---\n").expect("should write existing note");

    let source_with_existing = "[[existing]]";
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 3 },
            "contentChanges": [{ "text": source_with_existing }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(3));

    harness.send(request(
        6,
        "textDocument/codeAction",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "range": {
                "start": { "line": 0, "character": 3 },
                "end":   { "line": 0, "character": 3 },
            },
            "context": { "diagnostics": [] },
        })),
    ));

    let existing_response = harness.expect_message("existing note code action response", |message| message["id"] == 6);
    assert!(
        existing_response["result"].is_null(),
        "should return null for an already-resolved link"
    );

    shutdown_session(&mut harness);
}

#[test]
fn stdio_session_rejects_create_note_command_for_outside_or_existing_paths() {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

    let vault_uri = Url::from_file_path(vault_dir.path()).expect("vault path should convert to URI");

    let mut harness = LspHarness::spawn(vault_dir.path());
    initialize_session(&mut harness, &vault_uri, vault_dir.path());

    let outside_dir = tempfile::tempdir().expect("should create outside temp dir");
    let outside_path = outside_dir.path().join("outside.md");
    harness.send(request(
        2,
        "workspace/executeCommand",
        Some(json!({
            "command": "obsidian.createNote",
            "arguments": [outside_path.to_string_lossy().as_ref()],
        })),
    ));
    harness.expect_message("outside executeCommand response", |message| message["id"] == 2);
    assert!(
        !outside_path.exists(),
        "executeCommand should not create files outside the vault"
    );

    let existing_path = vault_dir.path().join("existing.md");
    fs::write(&existing_path, "original content").expect("should write existing note");
    harness.send(request(
        3,
        "workspace/executeCommand",
        Some(json!({
            "command": "obsidian.createNote",
            "arguments": [existing_path.to_string_lossy().as_ref()],
        })),
    ));
    harness.expect_message("existing executeCommand response", |message| message["id"] == 3);
    assert_eq!(
        fs::read_to_string(&existing_path).expect("existing note should still be readable"),
        "original content",
        "executeCommand should not overwrite existing notes"
    );

    shutdown_session(&mut harness);
}

#[test]
fn stdio_session_handles_completion_for_tags() {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

    // Note with frontmatter tags and an inline tag.
    let tagged_path = vault_dir.path().join("tagged.md");
    fs::write(
        &tagged_path,
        "---\nid: tagged\ntags: [project, work]\n---\n\nSome body with #rust tag.\n",
    )
    .expect("should write tagged note");

    let vault_path = vault_dir.path().canonicalize().expect("vault path should canonicalize");
    let vault_uri = Url::from_file_path(&vault_path).expect("vault path should convert to file URI");

    let mut harness = LspHarness::spawn(vault_dir.path());
    initialize_session(&mut harness, &vault_uri, &vault_path);

    // Open a source document with a partial tag "#pro" to test prefix filtering.
    let source_path = vault_dir.path().join("source.md");
    let partial_tag_text = "See #pro";
    fs::write(&source_path, partial_tag_text).expect("should write source note");
    let source_path = source_path.canonicalize().expect("source path should canonicalize");
    let source_uri = Url::from_file_path(&source_path).expect("source path should convert to file URI");

    harness.send(notification(
        "textDocument/didOpen",
        Some(json!({
            "textDocument": {
                "uri": source_uri,
                "languageId": "markdown",
                "version": 1,
                "text": partial_tag_text,
            }
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(1));

    // Request completion at end of "See #pro" (line 0, character 8).
    harness.send(request(
        2,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 0, "character": 8 },
        })),
    ));
    let partial_completion = harness.expect_message("partial tag completion response", |message| message["id"] == 2);
    let labels = completion_labels(&partial_completion);
    assert!(labels.contains(&"#project"), "should include #project for prefix 'pro'");
    assert!(!labels.contains(&"#work"), "#work should not match prefix 'pro'");
    assert!(!labels.contains(&"#rust"), "#rust should not match prefix 'pro'");

    // Update the document to just "#" — empty query should return all vault tags.
    let empty_query_text = "#";
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 2 },
            "contentChanges": [{ "text": empty_query_text }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(2));

    harness.send(request(
        3,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 0, "character": 1 },
        })),
    ));
    let all_tags_completion = harness.expect_message("all tags completion response", |message| message["id"] == 3);
    let labels = completion_labels(&all_tags_completion);
    assert!(
        labels.contains(&"#project"),
        "should include #project when query is empty"
    );
    assert!(labels.contains(&"#work"), "should include #work when query is empty");
    assert!(labels.contains(&"#rust"), "should include #rust when query is empty");

    // Ensure plain text without # returns null (no tag context).
    harness.send(notification(
        "textDocument/didChange",
        Some(json!({
            "textDocument": { "uri": source_uri, "version": 3 },
            "contentChanges": [{ "text": "plain text" }],
        })),
    ));
    expect_diagnostics(&mut harness, &source_uri, Some(3));

    harness.send(request(
        4,
        "textDocument/completion",
        Some(json!({
            "textDocument": { "uri": source_uri },
            "position": { "line": 0, "character": 10 },
        })),
    ));
    let plain_completion = harness.expect_message("plain text no tag completion", |message| message["id"] == 4);
    assert!(
        plain_completion["result"].is_null(),
        "plain text should return null, not tag completions"
    );

    shutdown_session(&mut harness);
}
