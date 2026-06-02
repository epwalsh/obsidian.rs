use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, Hover, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, MessageType, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities,
};
use tower_lsp::{Client, LanguageServer};

use crate::state::{BackendState, DiagnosticUpdate, DiagnosticsRequest, HoverRequest, StateError};

pub struct Backend {
    client: Client,
    state: Arc<RwLock<BackendState>>,
    diagnostics_lock: Arc<Mutex<()>>,
}

impl Backend {
    pub fn new(client: Client, vault: obsidian_core::Vault) -> Self {
        Self {
            client,
            state: Arc::new(RwLock::new(BackendState::new(vault))),
            diagnostics_lock: Arc::new(Mutex::new(())),
        }
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

    async fn compute_hover(&self, request: std::result::Result<HoverRequest, StateError>) -> Option<Hover> {
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                self.log_error(error).await;
                return None;
            }
        };

        match tokio::task::spawn_blocking(move || request.compute()).await {
            Ok(Ok(hover)) => hover,
            Ok(Err(error)) => {
                self.log_error(error).await;
                None
            }
            Err(error) => {
                self.log_error(format!("hover task failed: {error}")).await;
                None
            }
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(false),
                        change_notifications: None,
                    }),
                    file_operations: None,
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
        let vault_path = self.vault_path().await;
        self.client
            .log_message(
                MessageType::INFO,
                format!("obsidian-lsp ready for vault {}", vault_path.display()),
            )
            .await;
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

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let request = {
            let state = self.state.read().await;
            state.hover_request(
                params.text_document_position_params.text_document.uri,
                params.text_document_position_params.position,
            )
        };

        Ok(self.compute_hover(request).await)
    }
}
