# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`obsidian.rs` is a Rust library for working with Obsidian vaults. It is structured as a Cargo workspace with sub-crates for various features:
- `obsidian-core` (crate name: `obsidian_core`): core API used by the other sub-crates.

## Workspace Structure

- `Cargo.toml` — workspace root
- `obsidian-core/` — the core library crate
  - `src/lib.rs` — library entry point
  - `src/note.rs` — defines the `Note` struct
  - `src/link.rs` — parsing markdown/wiki/embedded links
  - `src/search.rs` — `find_note_paths()` for recursively finding `.md` files (public)
  - `src/vault.rs` — defines the `Vault` struct; `notes()` loads all notes, `search()` returns a query builder, `backlinks(&Note)` returns notes linking to a given note

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
