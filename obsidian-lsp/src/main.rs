mod args;
mod server;
mod state;
mod uri;

use std::path::PathBuf;

use clap::Parser;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // All logging goes to stderr — stdout is reserved for the JSON-RPC stream.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = args::Args::parse();
    let vault_path = args::resolve_vault_path(args.vault, std::env::var("OBSIDIAN_VAULT").ok().map(PathBuf::from))
        .map_err(|error| color_eyre::eyre::eyre!("could not find vault: {error}"))?;
    let vault = obsidian_core::Vault::open(&vault_path)
        .map_err(|error| color_eyre::eyre::eyre!("failed to open vault: {error}"))?;

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(move |client| server::Backend::new(client, vault));
    Server::new(stdin, stdout, socket).serve(service).await;

    Ok(())
}
