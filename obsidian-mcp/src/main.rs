mod error;
mod server;
mod tools;

use clap::Parser;

#[derive(Parser)]
#[command(about = "MCP server for an Obsidian vault")]
struct Args {
    /// Path to the Obsidian vault. Overrides the OBSIDIAN_VAULT environment variable.
    #[arg(long)]
    vault: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // All logging goes to stderr — stdout is reserved for the JSON-RPC stream.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let vault_path = if let Some(p) = args.vault {
        p
    } else if let Ok(p) = std::env::var("OBSIDIAN_VAULT") {
        std::path::PathBuf::from(p)
    } else {
        obsidian_core::Vault::open_from_cwd()
            .map_err(|e| color_eyre::eyre::eyre!("could not find vault: {e}"))?
            .path()
            .to_path_buf()
    };

    let vault =
        obsidian_core::Vault::open(&vault_path).map_err(|e| color_eyre::eyre::eyre!("failed to open vault: {e}"))?;

    let server = server::VaultServer::new(vault);
    let transport = rmcp::transport::io::stdio();
    rmcp::ServiceExt::serve(server, transport).await?.waiting().await?;

    Ok(())
}
