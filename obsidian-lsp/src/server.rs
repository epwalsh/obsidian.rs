use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeActionResponse, CompletionOptions,
    CompletionParams, CompletionResponse, ConfigurationItem, CreateFilesParams, DeleteFilesParams,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentLink,
    DocumentLinkOptions, DocumentLinkParams, DocumentSymbolParams, DocumentSymbolResponse, ExecuteCommandOptions,
    ExecuteCommandParams, FileChangeType, FileOperationFilter, FileOperationPattern, FileOperationPatternKind,
    FileOperationRegistrationOptions, FileSystemWatcher, GlobPattern, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, Location,
    MessageType, OneOf, PrepareRenameResponse, ReferenceParams, Registration, RenameFilesParams, RenameOptions,
    RenameParams, ServerCapabilities, ServerInfo, SymbolInformation, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, WatchKind, WorkspaceEdit,
    WorkspaceFileOperationsServerCapabilities, WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities,
    WorkspaceSymbolParams,
};
use tower_lsp::{Client, LanguageServer};

use crate::state::{
    BackendState, CodeActionRequest, CompletionRequest, Config, DiagnosticUpdate, DiagnosticsRequest,
    DocumentLinksRequest, DocumentSymbolsRequest, FileChange, FileChangeKind, NavigationRequest, PrepareRenameRequest,
    RenameRequest, ResolveDocumentLinkRequest, StateError, WorkspaceSymbolsRequest, new_note_content,
    normalize_new_note_path,
};
use crate::uri::uri_to_path;

pub struct Backend {
    client: Client,
    state: Arc<RwLock<BackendState>>,
    diagnostics_lock: Arc<Mutex<()>>,
    pending_file_changes: Arc<Mutex<Vec<FileChange>>>,
    file_change_debounce_scheduled: AtomicBool,
    supports_pull_config: AtomicBool,
    supports_watched_file_registration: AtomicBool,
}

const FILE_CHANGE_DEBOUNCE: Duration = Duration::from_millis(50);

fn markdown_file_operation_registration() -> FileOperationRegistrationOptions {
    FileOperationRegistrationOptions {
        filters: vec![FileOperationFilter {
            scheme: Some("file".to_string()),
            pattern: FileOperationPattern {
                glob: "**/*.md".to_string(),
                matches: Some(FileOperationPatternKind::File),
                options: None,
            },
        }],
    }
}

impl Backend {
    pub fn new(client: Client, vault: obsidian_core::Vault) -> Self {
        Self {
            client,
            state: Arc::new(RwLock::new(BackendState::new(vault))),
            diagnostics_lock: Arc::new(Mutex::new(())),
            pending_file_changes: Arc::new(Mutex::new(Vec::new())),
            file_change_debounce_scheduled: AtomicBool::new(false),
            supports_pull_config: AtomicBool::new(false),
            supports_watched_file_registration: AtomicBool::new(false),
        }
    }

    async fn pull_and_apply_config(&self) {
        let result = self
            .client
            .configuration(vec![ConfigurationItem {
                scope_uri: None,
                section: Some("obsidian".to_string()),
            }])
            .await;

        let values = match result {
            Ok(v) => v,
            Err(error) => {
                self.log_error(format!("failed to fetch configuration: {error}")).await;
                return;
            }
        };

        let config = parse_config(values.first().unwrap_or(&Value::Null));
        let request = {
            let mut state = self.state.write().await;
            if let Err(error) = state.apply_config(config) {
                self.log_error(format!("failed to apply configuration: {error}")).await;
                return;
            }
            state.global_diagnostics_request()
        };

        self.handle_diagnostics(Ok(request)).await;
    }

    async fn publish_diagnostics(&self, updates: &[DiagnosticUpdate]) {
        for update in updates {
            self.client
                .publish_diagnostics(update.uri.clone(), update.diagnostics.clone(), update.version)
                .await;
        }
    }

    async fn log_error(&self, error: impl std::fmt::Display) {
        self.client.log_message(MessageType::ERROR, error.to_string()).await;
    }

    async fn vault_path(&self) -> PathBuf {
        self.state.read().await.vault_path().to_path_buf()
    }

    async fn handle_diagnostics(&self, request: std::result::Result<DiagnosticsRequest, StateError>) {
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                self.log_error(error).await;
                return;
            }
        };

        let _guard = self.diagnostics_lock.lock().await;
        let batch = match tokio::task::spawn_blocking(move || request.compute()).await {
            Ok(Ok(batch)) => batch,
            Ok(Err(error)) => {
                self.log_error(error).await;
                return;
            }
            Err(error) => {
                self.log_error(format!("diagnostics task failed: {error}")).await;
                return;
            }
        };

        if self.state.read().await.diagnostics_revision() != batch.revision {
            return;
        }

        self.publish_diagnostics(&batch.updates).await;

        let mut state = self.state.write().await;
        if state.diagnostics_revision() == batch.revision {
            state.set_published_diagnostics(batch.published_diagnostics);
        }
    }

    async fn compute_hover(&self, request: std::result::Result<NavigationRequest, StateError>) -> Option<Hover> {
        self.compute_request(request, "hover", |request| request.compute_hover())
            .await
            .flatten()
    }

    async fn compute_request<Request, Output, Compute>(
        &self,
        request: std::result::Result<Request, StateError>,
        label: &'static str,
        compute: Compute,
    ) -> Option<Output>
    where
        Request: Send + 'static,
        Output: Send + 'static,
        Compute: FnOnce(Request) -> std::result::Result<Output, StateError> + Send + 'static,
    {
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                self.log_error(error).await;
                return None;
            }
        };

        match tokio::task::spawn_blocking(move || compute(request)).await {
            Ok(Ok(output)) => Some(output),
            Ok(Err(error)) => {
                self.log_error(error).await;
                None
            }
            Err(error) => {
                self.log_error(format!("{label} task failed: {error}")).await;
                None
            }
        }
    }

    async fn register_watched_files(&self) {
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher {
                glob_pattern: GlobPattern::String("**/*.md".to_string()),
                kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
            }],
        };
        let registration = Registration {
            id: "obsidian-rs-lsp-markdown-watch".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: serde_json::to_value(options).ok(),
        };

        if let Err(error) = self.client.register_capability(vec![registration]).await {
            self.log_error(format!("failed to register markdown file watcher: {error}"))
                .await;
        }
    }

    async fn handle_file_changes(&self, changes: Vec<FileChange>) {
        if changes.is_empty() {
            return;
        }

        self.pending_file_changes.lock().await.extend(changes);
        if self.file_change_debounce_scheduled.swap(true, Ordering::AcqRel) {
            return;
        }

        tokio::time::sleep(FILE_CHANGE_DEBOUNCE).await;

        loop {
            let changes = {
                let mut pending = self.pending_file_changes.lock().await;
                pending.drain(..).collect::<Vec<_>>()
            };

            if changes.is_empty() {
                self.file_change_debounce_scheduled.store(false, Ordering::Release);

                let has_pending_changes = !self.pending_file_changes.lock().await.is_empty();
                if has_pending_changes && !self.file_change_debounce_scheduled.swap(true, Ordering::AcqRel) {
                    tokio::time::sleep(FILE_CHANGE_DEBOUNCE).await;
                    continue;
                }
                return;
            }

            self.apply_file_changes_now(changes).await;
        }
    }

    async fn apply_file_changes_now(&self, changes: Vec<FileChange>) {
        let request = {
            let mut state = self.state.write().await;
            match state.apply_file_changes(changes) {
                Ok(request) => request,
                Err(error) => {
                    self.log_error(error).await;
                    return;
                }
            }
        };

        if let Some(request) = request {
            self.handle_diagnostics(Ok(request)).await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let supports_config = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.configuration)
            .unwrap_or(false);
        self.supports_pull_config.store(supports_config, Ordering::Relaxed);
        let supports_watched_files = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref())
            .and_then(|capabilities| capabilities.dynamic_registration)
            .unwrap_or(false);
        self.supports_watched_file_registration
            .store(supports_watched_files, Ordering::Relaxed);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(true),
                    work_done_progress_options: Default::default(),
                }),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["[".to_string(), "#".to_string()]),
                    ..Default::default()
                }),
                references_provider: Some(OneOf::Left(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["obsidian.createNote".to_string()],
                    work_done_progress_options: Default::default(),
                }),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(false),
                        change_notifications: None,
                    }),
                    file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                        did_create: Some(markdown_file_operation_registration()),
                        did_rename: Some(markdown_file_operation_registration()),
                        did_delete: Some(markdown_file_operation_registration()),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        if self.supports_watched_file_registration.load(Ordering::Relaxed) {
            self.register_watched_files().await;
        }

        if self.supports_pull_config.load(Ordering::Relaxed) {
            self.pull_and_apply_config().await;
        }

        let vault_path = self.vault_path().await;
        self.client
            .log_message(
                MessageType::INFO,
                format!("obsidian-lsp ready for vault {}", vault_path.display()),
            )
            .await;
    }

    async fn did_change_configuration(&self, _: DidChangeConfigurationParams) {
        if self.supports_pull_config.load(Ordering::Relaxed) {
            self.pull_and_apply_config().await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let request = {
            let mut state = self.state.write().await;
            state.open_document(
                params.text_document.uri,
                params.text_document.version,
                params.text_document.text,
            )
        };

        self.handle_diagnostics(request).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let request = {
            let mut state = self.state.write().await;
            state.change_document(
                params.text_document.uri,
                params.text_document.version,
                &params.content_changes,
            )
        };

        self.handle_diagnostics(request).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let request = {
            let mut state = self.state.write().await;
            state.close_document(params.text_document.uri)
        };

        self.handle_diagnostics(request).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let changes = params
            .changes
            .into_iter()
            .filter_map(|event| {
                let kind = if event.typ == FileChangeType::CREATED {
                    FileChangeKind::Created
                } else if event.typ == FileChangeType::CHANGED {
                    FileChangeKind::Changed
                } else if event.typ == FileChangeType::DELETED {
                    FileChangeKind::Deleted
                } else {
                    return None;
                };
                uri_to_path(&event.uri).ok().map(|path| FileChange { path, kind })
            })
            .collect();

        self.handle_file_changes(changes).await;
    }

    async fn did_create_files(&self, params: CreateFilesParams) {
        let changes = params
            .files
            .into_iter()
            .filter_map(|file| {
                tower_lsp::lsp_types::Url::parse(&file.uri)
                    .ok()
                    .and_then(|uri| uri_to_path(&uri).ok())
                    .map(|path| FileChange {
                        path,
                        kind: FileChangeKind::Created,
                    })
            })
            .collect();

        self.handle_file_changes(changes).await;
    }

    async fn did_rename_files(&self, params: RenameFilesParams) {
        let mut changes = Vec::new();
        for file in params.files {
            if let Ok(uri) = tower_lsp::lsp_types::Url::parse(&file.old_uri)
                && let Ok(path) = uri_to_path(&uri)
            {
                changes.push(FileChange {
                    path,
                    kind: FileChangeKind::Deleted,
                });
            }
            if let Ok(uri) = tower_lsp::lsp_types::Url::parse(&file.new_uri)
                && let Ok(path) = uri_to_path(&uri)
            {
                changes.push(FileChange {
                    path,
                    kind: FileChangeKind::Created,
                });
            }
        }

        self.handle_file_changes(changes).await;
    }

    async fn did_delete_files(&self, params: DeleteFilesParams) {
        let changes = params
            .files
            .into_iter()
            .filter_map(|file| {
                tower_lsp::lsp_types::Url::parse(&file.uri)
                    .ok()
                    .and_then(|uri| uri_to_path(&uri).ok())
                    .map(|path| FileChange {
                        path,
                        kind: FileChangeKind::Deleted,
                    })
            })
            .collect();

        self.handle_file_changes(changes).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let request = {
            let state = self.state.read().await;
            state.navigation_request(
                params.text_document_position_params.text_document.uri,
                params.text_document_position_params.position,
            )
        };

        Ok(self.compute_hover(request).await)
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let request = {
            let state = self.state.read().await;
            state.document_links_request(params.text_document.uri)
        };

        Ok(self
            .compute_request(request, "documentLink", |request: DocumentLinksRequest| {
                request.compute()
            })
            .await)
    }

    async fn document_link_resolve(&self, params: DocumentLink) -> Result<DocumentLink> {
        let fallback = params.clone();
        let request = {
            let state = self.state.read().await;
            state.resolve_document_link_request(params)
        };

        Ok(self
            .compute_request(
                request,
                "documentLink/resolve",
                |request: ResolveDocumentLinkRequest| request.compute(),
            )
            .await
            .unwrap_or(fallback))
    }

    async fn document_symbol(&self, params: DocumentSymbolParams) -> Result<Option<DocumentSymbolResponse>> {
        let request = {
            let state = self.state.read().await;
            state.document_symbols_request(params.text_document.uri)
        };

        Ok(self
            .compute_request(request, "documentSymbol", |request: DocumentSymbolsRequest| {
                request.compute()
            })
            .await)
    }

    async fn symbol(&self, params: WorkspaceSymbolParams) -> Result<Option<Vec<SymbolInformation>>> {
        let request = {
            let state = self.state.read().await;
            state.workspace_symbols_request(params.query)
        };

        Ok(self
            .compute_request(Ok(request), "workspace/symbol", |request: WorkspaceSymbolsRequest| {
                request.compute()
            })
            .await)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let request = {
            let state = self.state.read().await;
            state.navigation_request(
                params.text_document_position.text_document.uri,
                params.text_document_position.position,
            )
        };

        Ok(self
            .compute_request(request, "references", |request: NavigationRequest| {
                request.compute_references()
            })
            .await
            .flatten())
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let request = {
            let state = self.state.read().await;
            state.completion_request(
                params.text_document_position.text_document.uri,
                params.text_document_position.position,
            )
        };

        Ok(self
            .compute_request(request, "completion", |request: CompletionRequest| request.compute())
            .await
            .flatten()
            .map(CompletionResponse::Array))
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let request = {
            let state = self.state.read().await;
            state.navigation_request(
                params.text_document_position_params.text_document.uri,
                params.text_document_position_params.position,
            )
        };

        Ok(self
            .compute_request(request, "definition", |request: NavigationRequest| {
                request.compute_definition()
            })
            .await
            .flatten())
    }

    async fn prepare_rename(&self, params: TextDocumentPositionParams) -> Result<Option<PrepareRenameResponse>> {
        let request = {
            let state = self.state.read().await;
            state.prepare_rename_request(params.text_document.uri, params.position)
        };

        Ok(self
            .compute_request(request, "prepareRename", |request: PrepareRenameRequest| {
                request.compute()
            })
            .await
            .flatten())
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let request = {
            let state = self.state.read().await;
            state.rename_request(
                params.text_document_position.text_document.uri,
                params.text_document_position.position,
                params.new_name,
            )
        };

        Ok(self
            .compute_request(request, "rename", |request: RenameRequest| request.compute())
            .await
            .flatten())
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let request = {
            let state = self.state.read().await;
            state.code_action_request(params.text_document.uri, params.range, params.context.diagnostics)
        };
        Ok(self
            .compute_request(request, "codeAction", |request: CodeActionRequest| request.compute())
            .await
            .flatten()
            .map(|actions| actions.into_iter().map(CodeActionOrCommand::CodeAction).collect()))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<serde_json::Value>> {
        if params.command != "obsidian.createNote" {
            return Ok(None);
        }

        let path_str = match params.arguments.first().and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                self.log_error("obsidian.createNote: missing path argument").await;
                return Ok(None);
            }
        };

        let path = {
            let state = self.state.read().await;
            normalize_new_note_path(state.vault_path(), &path_str)
        };
        let Some(path) = path else {
            self.log_error(format!("obsidian.createNote: invalid note path: {path_str}"))
                .await;
            return Ok(None);
        };
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("note").to_string();
        let note_title = params.arguments.get(1).and_then(Value::as_str).map(str::to_string);
        let created_path = path.clone();

        match tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
            file.write_all(new_note_content(&stem, note_title.as_deref()).as_bytes())
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.log_error(format!("obsidian.createNote: failed to write file: {e}"))
                    .await;
                return Ok(None);
            }
            Err(e) => {
                self.log_error(format!("obsidian.createNote task failed: {e}")).await;
                return Ok(None);
            }
        }

        // Proactively refresh diagnostics so the broken-link warning clears immediately.
        let request = {
            let mut state = self.state.write().await;
            state.apply_file_changes(vec![FileChange {
                path: created_path,
                kind: FileChangeKind::Created,
            }])
        };
        match request {
            Ok(Some(request)) => self.handle_diagnostics(Ok(request)).await,
            Ok(None) => {}
            Err(error) => self.log_error(error).await,
        }

        Ok(None)
    }
}

fn parse_config(value: &Value) -> Config {
    let vault_path_override = value
        .get("vault")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    let diagnostics_ignore = value
        .get("diagnostics")
        .and_then(|d| d.get("ignore"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();

    Config {
        vault_path_override,
        diagnostics_ignore,
    }
}
