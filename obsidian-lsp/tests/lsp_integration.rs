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

fn create_feature_vault() -> (tempfile::TempDir, Url, Url, String) {
    let vault_dir = tempfile::tempdir().expect("should create temp dir");
    fs::create_dir(vault_dir.path().join(".obsidian")).expect("should create .obsidian directory");

    let target_path = vault_dir.path().join("target.md");
    fs::write(
        &target_path,
        "---\nid: target-id\ntitle: Target Note\naliases: [target-alias]\ntags: [rust]\n---\n\nBody.\n",
    )
    .expect("should write target note");

    let duplicate_a_path = vault_dir.path().join("duplicate-a.md");
    let duplicate_a_text =
        "---\nid: shared-id\naliases: [shared-alias]\n---\n\nSee [[missing-note]] and [[target-id]].\n";
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

    (
        vault_dir,
        duplicate_a_uri,
        duplicate_b_uri,
        duplicate_a_text.to_string(),
    )
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
    let (vault_dir, duplicate_a_uri, duplicate_b_uri, duplicate_a_text) = create_feature_vault();
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

    shutdown_session(&mut harness);
}
