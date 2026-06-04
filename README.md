# obsidian.rs

[![crates.io (core)](https://img.shields.io/crates/v/obsidian-rs-core?label=obsidian-rs-core)](https://crates.io/crates/obsidian-rs-core)
[![crates.io (cli)](https://img.shields.io/crates/v/obsidian-rs-cli?label=obsidian-rs-cli)](https://crates.io/crates/obsidian-rs-cli)
[![crates.io (mcp)](https://img.shields.io/crates/v/obsidian-rs-mcp?label=obsidian-rs-mcp)](https://crates.io/crates/obsidian-rs-mcp)
[![crates.io (lsp)](https://img.shields.io/crates/v/obsidian-rs-lsp?label=obsidian-rs-lsp)](https://crates.io/crates/obsidian-rs-lsp)
[![docs.rs](https://img.shields.io/docsrs/obsidian-rs-core)](https://docs.rs/obsidian-rs-core)
[![License](https://img.shields.io/crates/l/obsidian-rs-core)](LICENSE)

<h1 align="center">obsidian.rs</h1>
<div><h4 align="center"><a href="#cli">CLI</a> · <a href="#mcp-server">MCP Server</a> · <a href="#lsp-server">LSP Server</a>
<div align="center">
<a href="https://github.com/epwalsh/obsidian.rs/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/epwalsh/obsidian.rs?style=for-the-badge&logo=starship&logoColor=D9E0EE&labelColor=302D41&&color=d9b3ff&include_prerelease&sort=semver" /></a>
<a href="https://github.com/epwalsh/obsidian.rs/pulse"><img alt="Last commit" src="https://img.shields.io/github/last-commit/epwalsh/obsidian.rs?style=for-the-badge&logo=github&logoColor=D9E0EE&labelColor=302D41&color=9fdf9f"/></a>
<a href="https://rust-lang.org"><img alt="Built with Rust" src="https://img.shields.io/badge/Rust-Built_with_rust-grey?style=for-the-badge&logo=Rust&logoColor=D9E0EE&labelColor=302D41&color=%23B7410E"></a>
<a href="https://github.com/epwalsh/obsidian.rs/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/crates/l/obsidian-rs-core?style=for-the-badge&logoColor=D9E0EE&labelColor=302D41"></a>
</div>
<hr>

A collection of tools for working with [Obsidian](https://obsidian.md) vaults, written in Rust.

| Crate | Description |
|---|---|
| [`obsidian-rs-core`](https://crates.io/crates/obsidian-rs-core) | Core library — the foundation the other tools build on |
| [`obsidian-rs-cli`](https://crates.io/crates/obsidian-rs-cli) | Command-line tool for querying and managing vaults |
| [`obsidian-rs-mcp`](https://crates.io/crates/obsidian-rs-mcp) | MCP server so agents can interact with your vault |
| [`obsidian-rs-lsp`](https://crates.io/crates/obsidian-rs-lsp) | A language server for vault editing in your IDE |

## CLI

Install with Cargo:

```sh
cargo install obsidian-rs-cli
```

The `obsidian` binary resolves your vault automatically — it walks up from the current directory looking for a folder containing `.obsidian/`. You can also set `OBSIDIAN_VAULT` or pass `--vault <PATH>` explicitly.

### Commands

```
obsidian search     Search for notes
obsidian note       Work with individual notes
  resolve           Resolve a note by path, ID, or alias
  list              List all notes
  read              Read contents or frontmatter
  write             Write a new note
  backlinks         Find notes that link to a given note
  merge             Merge multiple notes into one
  patch             Replace one exact string in a note's content
  rename            Rename a note and update all backlinks
  update            Update frontmatter metadata fields
obsidian tags       Work with tags
  list              List all tags used across the vault
  search            Find all occurrences of given tags
obsidian check      Vault health check (broken links, duplicate IDs/aliases)
```

### Examples

```sh
# Find all notes tagged #project that mention "rust"
obsidian search --tag project --content-contains rust

# Find notes matching a regex pattern in their content
obsidian search --content-matches 'TODO:.*urgent'

# List notes sorted by last modified
obsidian note list --sort modified-desc

# Read a note's frontmatter as JSON
obsidian note read "My Note" --format json

# Rename a note and automatically update every backlink
obsidian note rename "Old Title" "New Title"

# Find all notes that link to a given note
obsidian note backlinks "My Note"

# List all tags, sorted alphabetically
obsidian tags list --sort path-asc

# Check the vault for broken links and duplicate IDs
obsidian check
```

## MCP Server

Install and register with Claude in one step:

```sh
cargo install obsidian-rs-mcp
claude mcp add obsidian --scope project obsidian-mcp --vault .
```

### Tools exposed

| Tool | Description |
|---|---|
| `check_vault` | Report duplicate IDs, duplicate aliases, and broken links in the vault |
| `list_notes` | List all notes in the vault |
| `read_note` | Read the body and frontmatter of a note |
| `write_note` | Write a new note |
| `patch_note` | Replace one exact string in a note |
| `update_note` | Update frontmatter fields of a note |
| `search_notes` | Search for notes with filters (tag, title, content, glob, regex) |
| `rename_note` | Rename a note and update all backlinks |
| `list_tags` | List all tags used in the vault |
| `search_tags` | Find all occurrences of given tags |

## LSP Server

Install with Cargo:

```sh
cargo install obsidian-rs-lsp
```

The `obsidian-lsp` binary speaks stdio LSP and resolves the vault the same way as `obsidian-mcp`: `--vault <PATH>`, then `OBSIDIAN_VAULT`, then the nearest parent containing `.obsidian/`.

Current functionality:

- clean LSP initialization and shutdown
- full-document text sync for open buffers
- in-memory buffer shadowing through `obsidian_core::Vault::load_note()` / `unload_note()`
- diagnostics for broken links, duplicate IDs, and duplicate aliases across the vault
- hover on note links with basic target-note metadata
- document links for wiki and markdown note links, with resolve support
- references/backlinks for linked notes, or for the current note when the cursor is not on a link
- go-to definition for linked notes, including heading anchors and nested sub-anchors
- diagnostics clearing and refresh on open/change/close events
- completion for wiki links (`[[`) and markdown links (`[`) with per-note variants: bare ID, bare title, alias override, and bare alias forms

Advanced editor features such as rename are not implemented yet.
