# AGENTS.md

This file provides guidance to coding agents when working in this repository.

## Project Overview

`obsidian.rs` is a Rust library and CLI for working with Obsidian vaults. It is structured as a Cargo workspace with sub-crates for various features:
- `obsidian-core` (crate name: `obsidian-rs-core`; library name: `obsidian_core`): core API used by the other sub-crates.
- `obsidian-cli` (crate name: `obsidian-rs-cli`; binary name: `obsidian`): command-line interface exposing `search`, `note`, `tags`, and `check` commands. The `note` subcommand supports `backlinks`, `merge`, `patch`, `rename`, and `update`. The `check` command reports duplicate IDs or aliases, broken links, and stranded notes, while ignoring `README.md`-style notes for stranded-note reporting.
- `obsidian-mcp` (crate name: `obsidian-rs-mcp`; binary name: `obsidian-mcp`): MCP (Model Context Protocol) server over STDIO transport. Exposes vault operations as MCP tools: `read_note`, `list_notes`, `list_backlinks`, `write_note`, `append_to_note`, `patch_note`, `update_note`, `search_notes`, `rename_note`, `list_tags`, `search_tags`, `check_vault`. Vault path resolved in order: `--vault <PATH>` CLI arg, then `OBSIDIAN_VAULT` env var, then `open_from_cwd()`. Uses the `rmcp` crate with `tokio` for async handling of blocking vault I/O. `check_vault` reports duplicate IDs or aliases, broken links, and stranded notes.
- `obsidian-lsp` (crate name: `obsidian-rs-lsp`; binary name: `obsidian-lsp`): Language Server Protocol server over STDIO transport. Resolves the vault with the same precedence as `obsidian-mcp` and currently provides initialization, full-document sync for open buffers, cached vault state through `obsidian_core::Vault::open_cached()` with open-buffer overlays, watched Markdown file and workspace file-operation refreshes, health diagnostics for broken links, duplicate IDs or aliases, stranded notes, and trailing whitespace, document formatting that trims trailing whitespace and normalizes parseable YAML frontmatter, hover metadata for note links and tags, document links with resolve support, document symbols for note structure, workspace symbols for vault-wide note/tag/heading search, backlinks-based references, go-to-definition for note links, heading anchors and nested sub-anchors, completion for wiki/markdown note links and tags, create-note quick fixes for broken note links, duplicate ID/alias quick fixes, wiki/markdown link conversion refactors, wiki-link missing-heading quick fixes, filename-first note rename via `textDocument/prepareRename` / `textDocument/rename` with backlink updates, and tag references/definition/rename for inline and frontmatter tags.

## Workspace Structure

- `Cargo.toml` — workspace root
- `obsidian-core/` — the core library crate
  - `src/lib.rs` — library entry point
  - `src/note.rs` — defines the `Note` struct; `content` is `Option<String>` (not loaded by default); `links` and `tags` are always pre-computed; `from_path()` omits content, `from_path_with_content()` retains it; `write()` requires content, `write_frontmatter()` reads body from disk. `tags: Vec<LocatedTag>` holds all tags — frontmatter tags have `location: Location::Frontmatter`, inline body tags have `location: Location::Inline(InlineLocation)`
  - `src/link.rs` — parsing markdown/wiki/embedded links
  - `src/search.rs` — `find_note_paths()` for recursively finding `.md` files (public)
  - `src/health.rs` — `VaultHealthReport`, `DuplicateId`, `DuplicateAlias`, `BrokenLink`, `StrandedNote`, `NoteRef` types returned by `Vault::check()`, plus reusable `check_notes()` / `backlinks_from()` helpers for cached note sets
  - `src/vault.rs` — defines the `Vault` struct; `notes()` loads all notes (no content), `notes_with_content()` loads with body text, `open_cached()` keeps a normalized cached note/text snapshot for long-lived processes, `refresh_cached_note()` / `remove_cached_note()` update cached state incrementally, `search()` returns a query builder, `backlinks(&Note)` returns notes linking to a given note, `rename(&Note, new_path)` renames a note and updates all backlinks, `rename_edits(&Note, new_path)` previews exact backlink replacement spans for rename integrations, `merge(&[Note], dest_path)` merges multiple notes (sources must be loaded with content) into a destination and updates all backlinks, `append_to_note(&Note, content)` appends raw body content without rewriting frontmatter, `patch_note(&Note, old_string, new_string)` replaces exactly one occurrence of a string in the raw file, `check(filter)` scans for duplicate IDs/aliases, broken links, and stranded notes returning a `VaultHealthReport`
- `obsidian-cli/` — the CLI binary crate
  - `src/main.rs` — entry point, subcommand dispatch
  - `src/args.rs` — clap argument structs and enums
  - `src/check.rs` — `check` command: vault health (duplicate IDs/aliases, broken links, stranded notes)
  - `src/output.rs` — plain and JSON rendering
  - `src/error.rs` — `CliError` type
  - `tests/cli.rs` — integration tests via `assert_cmd`
- `obsidian-mcp/` — the MCP server binary crate
  - `src/main.rs` — entry point: reads `OBSIDIAN_VAULT`, opens vault, starts STDIO server
  - `src/server.rs` — `VaultServer` struct with `#[tool_router]` impl (12 tools) and `#[tool_handler]` `ServerHandler` impl
  - `src/tools.rs` — parameter structs (`Deserialize + JsonSchema`) for all 12 tools
  - `src/error.rs` — `vault_err`, `note_err`, `search_err`, `other_err` helpers converting core errors to `rmcp::ErrorData`
- `obsidian-lsp/` — the LSP server binary crate
  - `src/main.rs` — entry point: parses CLI args, initializes error reporting/logging, resolves the vault, and starts the STDIO LSP server
  - `src/args.rs` — clap args and vault resolution helper for `--vault`, `OBSIDIAN_VAULT`, and `open_from_cwd()`
  - `src/server.rs` — `Backend` implementation of `tower_lsp::LanguageServer` for initialize/open/change/close flows, watched-file and workspace file-operation events, diagnostics publication, document formatting, hover, document links, document symbols, workspace symbols, references, definition, completion, code action, and rename requests
  - `src/state/` — shared backend state plus cached `Vault` snapshot overlays and snapshot-based diagnostics, formatting, navigation, completion, code action, rename, tag, and symbol computation helpers
  - `src/uri.rs` — file URI/path conversion helpers and vault-relative path validation
  - `tests/lsp_integration.rs` — end-to-end stdio JSON-RPC harness covering initialize, dynamic watched-file registration, diagnostics, hover, document links, document symbols, workspace symbols, references, definition, completion, code actions, rename, document sync and watched-file notifications, shutdown, and exit

## Development

- After making changes, always run `cargo fmt` to ensure consistent code formatting and `cargo clippy -- -D warnings` to check for linting issues.
- Always update this file when new modules, crates, or features are added to the project.
- Always update the @CHANGELOG.md and @README.md when adding new features, changing an API, or fixing bugs.

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
cargo clippy -- -D warnings

# Format
cargo fmt
```
