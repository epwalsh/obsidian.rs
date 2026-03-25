# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`obsidian.rs` is a Rust library and CLI for working with Obsidian vaults. It is structured as a Cargo workspace with sub-crates for various features:
- `obsidian-core` (crate name: `obsidian_core`): core API used by the other sub-crates.
- `obsidian-cli` (binary name: `obsidian`): command-line interface exposing `search`, `note`, `tags`, and `check` commands. The `note` subcommand supports `backlinks`, `merge`, `patch`, `rename`, and `update`.
- `obsidian-mcp` (binary name: `obsidian-mcp`): MCP (Model Context Protocol) server over STDIO transport. Exposes vault operations as MCP tools: `read_note`, `write_note`, `patch_note`, `update_note`, `search_notes`, `rename_note`, `list_tags`, `search_tags`. Vault path configured via `OBSIDIAN_VAULT` env var (falls back to `open_from_cwd()`). Uses the `rmcp` crate with `tokio` for async handling of blocking vault I/O.

## Workspace Structure

- `Cargo.toml` — workspace root
- `obsidian-core/` — the core library crate
  - `src/lib.rs` — library entry point
  - `src/note.rs` — defines the `Note` struct; `content` is `Option<String>` (not loaded by default); `links` and `inline_tags` are always pre-computed; `from_path()` omits content, `from_path_with_content()` retains it; `write()` requires content, `write_frontmatter()` reads body from disk
  - `src/link.rs` — parsing markdown/wiki/embedded links
  - `src/search.rs` — `find_note_paths()` for recursively finding `.md` files (public)
  - `src/vault.rs` — defines the `Vault` struct; `notes()` loads all notes (no content), `notes_with_content()` loads with body text, `search()` returns a query builder, `backlinks(&Note)` returns notes linking to a given note, `rename(&Note, new_path)` renames a note and updates all backlinks, `merge(&[Note], dest_path)` merges multiple notes (sources must be loaded with content) into a destination and updates all backlinks, `patch_note(&Note, old_string, new_string)` replaces exactly one occurrence of a string in the raw file
- `obsidian-cli/` — the CLI binary crate
  - `src/main.rs` — entry point, subcommand dispatch
  - `src/args.rs` — clap argument structs and enums
  - `src/check.rs` — `check` command: vault health (duplicate IDs/aliases, broken links)
  - `src/output.rs` — plain and JSON rendering
  - `src/error.rs` — `CliError` type
  - `tests/cli.rs` — integration tests via `assert_cmd`
- `obsidian-mcp/` — the MCP server binary crate
  - `src/main.rs` — entry point: reads `OBSIDIAN_VAULT`, opens vault, starts STDIO server
  - `src/server.rs` — `VaultServer` struct with `#[tool_router]` impl (8 tools) and `#[tool_handler]` `ServerHandler` impl
  - `src/tools.rs` — parameter structs (`Deserialize + JsonSchema`) for all 8 tools
  - `src/error.rs` — `vault_err`, `note_err`, `search_err`, `other_err` helpers converting core errors to `rmcp::ErrorData`

## Development

- Always run `cargo fmt` after making changes to ensure consistent code formatting.
- Always update this file when new modules, crates, or features are added to the project.

## Common Commands

```sh
# Check compilation
cargo check

# Build
cargo build

# Run tests
cargo test

# Run a single test
cargo test <test_name>

# Lint
cargo clippy

# Format
cargo fmt
```
