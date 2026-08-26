<h1 align="center">obsidian.rs</h1>
<div><h4 align="center"><a href="#cli">CLI</a> · <a href="#mcp-server">MCP Server</a> · <a href="#lsp-server">LSP Server</a></h4></div>
<div align="center">
<a href="https://github.com/epwalsh/obsidian.rs/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/epwalsh/obsidian.rs?style=for-the-badge&logo=starship&logoColor=D9E0EE&labelColor=302D41&&color=d9b3ff&include_prerelease&sort=semver" /></a>
<a href="https://github.com/epwalsh/obsidian.rs/pulse"><img alt="Last commit" src="https://img.shields.io/github/last-commit/epwalsh/obsidian.rs?style=for-the-badge&logo=github&logoColor=D9E0EE&labelColor=302D41&color=9fdf9f"/></a>
<a href="https://rust-lang.org"><img alt="Built with Rust" src="https://img.shields.io/badge/Rust-Built_with_rust-grey?style=for-the-badge&logo=Rust&logoColor=D9E0EE&labelColor=302D41&color=%23B7410E"></a>
<a href="https://github.com/epwalsh/obsidian.rs/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/crates/l/obsidian-rs-core?style=for-the-badge&logoColor=D9E0EE&labelColor=302D41"></a>
</div>
<hr>

A collection of tools for working with [Obsidian](https://obsidian.md) vaults, written in Rust.

When a note relies on a filename-derived ID instead of an explicit frontmatter `id`, obsidian.rs normalizes that ID to lowercase ASCII kebab-case with Unicode transliteration. For example, `Café Note.md` defaults to `cafe-note`.


| Crate | Docs | Description |
|---|---|---|
| [`obsidian-rs-core`](https://crates.io/crates/obsidian-rs-core) | [docs.rs/obsidian-rs-core](https://docs.rs/obsidian-rs-core) | Core library — the foundation the other tools build on |
| [`obsidian-rs-cli`](https://crates.io/crates/obsidian-rs-cli) | [docs.rs/obsidian-rs-cli](https://docs.rs/obsidian-rs-cli) | Command-line tool for querying and managing vaults |
| [`obsidian-rs-mcp`](https://crates.io/crates/obsidian-rs-mcp) | [docs.rs/obsidian-rs-mcp](https://docs.rs/obsidian-rs-mcp) | MCP server so agents can interact with your vault |
| [`obsidian-rs-lsp`](https://crates.io/crates/obsidian-rs-lsp) | [docs.rs/obsidian-rs-lsp](https://docs.rs/obsidian-rs-lsp) | A language server for vault editing in your IDE |

## CLI

Install with Cargo:

```sh
cargo install obsidian-rs-cli
```

The installed binary is `obsidian-rs` (not `obsidian`), so it does not collide with the official Obsidian desktop app or [Obsidian CLI](https://obsidian.md/cli), which also register an `obsidian` command on PATH.

The CLI resolves your vault automatically — it walks up from the current directory looking for a folder containing `.obsidian/`. You can also set `OBSIDIAN_VAULT` or pass `--vault <PATH>` explicitly.

### Commands

```
obsidian-rs search     Search for notes
obsidian-rs note       Work with individual notes
  resolve              Resolve a note by path, ID, or alias
  list                 List all notes
  read                 Read contents or frontmatter
  write                Write a new note
  backlinks            Find notes that link to a given note
  extract              Extract a section or span into a new note
  merge                Merge multiple notes into one
  patch                Replace one exact string in a note's body
  rename               Rename a note and update all backlinks
  update               Update frontmatter metadata fields
obsidian-rs tags       Work with tags
  list                 List all tags used across the vault
  search               Find all occurrences of given tags
obsidian-rs check      Vault health check (broken links, duplicate IDs/aliases, stranded notes)
```

### Examples

```sh
# Find all notes tagged #project that mention "rust"
obsidian-rs search --tag project --content-contains rust

# Find notes matching a regex pattern in their content
obsidian-rs search --content-matches 'TODO:.*urgent'

# List notes sorted by last modified
obsidian-rs note list --sort modified-desc

# Read a note's frontmatter as JSON
obsidian-rs note read "My Note" --frontmatter --format json

# Rename a note and automatically update every backlink
obsidian-rs note rename "Old Title" "New Title"

# Extract a named section into a new note, leaving a wiki link behind
obsidian-rs note extract "Source Note" notes/section.md --section Section

# Find all notes that link to a given note
obsidian-rs note backlinks "My Note"

# List all tags, sorted alphabetically
obsidian-rs tags list --sort path-asc

# Check the vault for broken links, duplicate IDs, and stranded notes
obsidian-rs check
```

Stranded-note health checks ignore `README.md`-style notes by default; use the existing ignore options to exclude additional vault-relative paths.

## MCP Server

Install and register with Claude in one step:

```sh
cargo install obsidian-rs-mcp
claude mcp add obsidian --scope project obsidian-mcp --vault .
```

When initialized by an MCP client, the server describes the vault as a collection of Markdown notes and summarizes the safe workflow for consuming agents: discover/read before writing, use `extract_to_note` when splitting content into a new note, use `append_to_note` for additive body updates, use exact-match patching for surgical edits, rely on rename tools for backlink updates, and prefer vault-relative paths for writes and renames.

### Tools exposed

| Tool | Description |
|---|---|
| `check_vault` | Report duplicate IDs, duplicate aliases, broken links, and stranded notes in the vault |
| `list_notes` | List all notes in the vault |
| `list_backlinks` | List notes that link to a given note, including matching link locations |
| `read_note` | Read the body and frontmatter of a note |
| `write_note` | Write a new note |
| `extract_to_note` | Extract a named section or explicit span into a new note and update the source note |
| `append_to_note` | Append content exactly to the end of a note's body without rewriting frontmatter |
| `patch_note` | Replace one exact string in a note's body without rewriting frontmatter |
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
- cached vault state via `obsidian_core::Vault::open_cached()`, with open buffers layered on top as authoritative in-memory shadows
- startup work-done progress while indexing the vault; language-feature requests return empty results until indexing is ready
- diagnostics for broken links, duplicate IDs, duplicate aliases, stranded notes, and trailing whitespace across the vault and open buffers
- document formatting that removes trailing whitespace and normalizes parseable YAML frontmatter blocks
- hover on note links with basic target-note metadata
- document links for wiki and markdown note links, with resolve support
- document symbols for headings, frontmatter keys, aliases, tags, and outbound links
- workspace symbol search across note IDs, titles, aliases, tags, and headings
- references/backlinks for the target of wiki or markdown note links, or for the current note when the cursor is not on a link
- go-to definition for linked notes, including heading anchors and nested sub-anchors
- diagnostics clearing and refresh on open/change/close events, watched Markdown file changes, and workspace file create/rename/delete notifications
- completion for wiki links (`[[`) and markdown links (`[`) with per-note variants (bare ID/title, alias override, and bare alias), plus inline tag completion (`#`)
- quick fixes for broken note links that create missing notes inside the active vault without overwriting existing files, using wiki aliases or markdown link text as the new note's primary alias and heading when available
- execute-command extraction of named sections or explicit spans into new notes, plus code action previews for extracting either a selected span or the section under a heading-line cursor; heading-based previews derive their default target path from the full heading ancestry (for example `Foo` > `Bar` -> `foo-bar.md`)
- quick fixes for duplicate IDs and aliases, wiki-only missing-heading creation, and refactors that convert between wiki and markdown note links while preserving resolved note IDs in generated wiki targets
- filename-first note rename via `textDocument/prepareRename` and `textDocument/rename`, with backlink updates and automatic frontmatter ID updates when the current ID matches the old filename stem
- hover, references, go-to-definition, and rename support for inline and frontmatter tags

Additional explicit refactor modes are still planned.

### LSP roadmap

Planned LSP work prioritizes Obsidian-specific editing and refactoring features that generic Markdown language servers cannot provide:

1. **Rename/refactor follow-ups**: add explicit commands or code actions for path-only, ID-only, and forced combined rename modes. Standard LSP rename already defaults to renaming the filename/path; it also renames the frontmatter ID when the current ID exactly matches the old filename stem, and otherwise leaves custom IDs unchanged.
2. **Quality-of-life configuration**: add settings for preferred new-note folder, preferred link style, completion limits, diagnostics toggles, case sensitivity, and tag completion behavior.
