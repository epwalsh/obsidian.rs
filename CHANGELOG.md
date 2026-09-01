# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## v0.6.0 - 2026-09-01

### Added

- LSP diagnostics now warn on trailing whitespace, including unsaved open buffers inside the vault.
- Added LSP `textDocument/formatting` support for trimming trailing whitespace and normalizing parseable YAML frontmatter blocks.
- Added LSP startup work-done progress while indexing the vault, with language-feature requests returning empty results until indexing is ready.
- Added `append_to_note` MCP tool for appending content to a note body without rewriting frontmatter, backed by `Vault::append_to_note()`.
- Added note extraction support across the core crate, CLI (`obsidian-rs note extract`), MCP server (`extract_to_note`), and LSP (`obsidian.extractToNote` plus selection preview code actions). Named-section extraction keeps the source heading, promotes extracted heading levels for the new note, and rewrites relative markdown links against the new note path.

### Changed

- Renamed the CLI binary from `obsidian` to `obsidian-rs` to avoid conflicting with the official Obsidian desktop app and CLI, which also install an `obsidian` command on PATH. Update scripts and muscle memory accordingly (`cargo install obsidian-rs-cli` now installs `obsidian-rs`).
- Refactored the `obsidian-rs-core` note/vault representation for a single, unambiguous source of truth:
  - `Note` now stores its full contents (including frontmatter) in a private `text` field, exposed via `Note::text()`, with `Note::body()` returning the frontmatter-stripped body. The public `Note::body` field, `Note::from_path_with_body()`, `Note::load_body()`, `Note::reload_with_body()`, and `NoteError::BodyNotLoaded` are removed; `Note::from_path()` now always loads contents. `Note::add_tag()`, `remove_tag()`, and `add_alias()` now return `Result` and keep `text` in sync.
  - `Vault` now holds a single `cached_notes: Option<HashMap<PathBuf, Arc<Note>>>` snapshot instead of separate override and cached-disk collections. `Some` is an authoritative in-memory snapshot (no filesystem access); `None` is disk-walk mode. `Vault::notes_with_content()` is removed (`notes()` always carries content).
  - `obsidian_core::search` drops the `loaded_notes` parameters and the `_with_content` function variants; `SearchQuery` runs against the cached snapshot when provided.
- Vault health checks in the CLI, MCP server, and LSP now report stranded notes (notes with no incoming or outgoing note links) and ignore `README.md`-style notes by default.
- Filename-derived note IDs are now normalized to lowercase ASCII kebab-case with Unicode transliteration, so notes like `Café Note.md` default to `cafe-note` when no explicit frontmatter `id` is set.
- LSP heading-based extract code actions now derive their default target path from the full matched heading ancestry, so extracting `Bar` under `Foo` defaults to `foo-bar.md` and the existing path-based ID rules then produce `foo-bar`.

### Fixed

- LSP extract-to-note code actions now appear for cursor-only requests on heading lines, not just explicit text selections.
- LSP diagnostics now run outside the request/notification path and skip superseded revisions, reducing stale diagnostics and preventing diagnostics work from blocking formatting requests.
- Vault health connectivity checks now avoid all-pairs note scans, improving diagnostics latency for larger vaults.
- LSP markdown-to-wiki link refactors now use the resolved note ID for generated wiki targets instead of the filename stem, which fixes incorrect suggestions for notes like nested `index.md` files.
- `patch_note` no longer rewrites untouched frontmatter while patching body text, so explicit empty arrays like `tags: []` are preserved.

## v0.5.0 - 2026-06-12

### Added

- Added `list_backlinks` MCP tool: returns notes linking to a given note plus the matching wiki/markdown link metadata and source locations.
- Added LSP `prepareRename` / `rename` support for filename-first note renames with backlink updates and stem-matching frontmatter ID updates.
- Added LSP hover, references, go-to-definition, and rename support for inline and frontmatter tags.
- Added LSP document symbols for note structure and workspace symbols for vault-wide note, tag, and heading search.
- Added LSP code actions for duplicate ID/alias fixes, wiki/markdown link conversion, and wiki-link missing-heading creation.
- Added `Vault::rename_edits()` to expose exact backlink replacement spans for rename previews.
- Added cached-vault support to `obsidian-core::Vault`, including `open_cached()`, cached note text, incremental note refresh/removal, cached search, and health checks over cached notes.
- Added LSP handling for watched Markdown file changes and workspace file create/rename/delete notifications.

### Changed

- Expanded the MCP server initialization instructions so consuming agents see a richer overview of vault capabilities and safe tool usage.
- LSP create-note quick fixes now use wiki aliases and markdown link text as the new note's primary alias and heading.
- LSP diagnostics, navigation, completion, symbols, and code actions now use cached vault snapshots with open-document overlays instead of reparsing the whole vault for every request.

### Fixed

- Fixed LSP completion replacement ranges for wiki links, markdown links, and tags on lines containing non-ASCII characters.
- Fixed LSP duplicate-alias diagnostics and quick fixes to attach to the exact frontmatter alias token when available, and to appear for cursor-line code-action requests even when clients omit diagnostics.
- Fixed stale LSP diagnostics for external note creates, edits, deletes, and renames when clients send watched-file or workspace file-operation notifications.
- Fixed a bug with resolving notes to `index.md` files from the name of their parent directory.

## v0.4.0 - 2026-06-05

### Added

- Added a code action to the LSP for creating missing notes.
- Added LSP completion for tags.

## v0.3.0 - 2026-06-03

### Added

- Added a real `obsidian-rs-lsp` stdio server using `tower-lsp`, including vault resolution via `--vault`, `OBSIDIAN_VAULT`, or `open_from_cwd()`, full-document buffer syncing into `Vault::load_note()` / `unload_note()`, workspace health diagnostics for broken links and duplicate IDs or aliases, link hover metadata, document links with resolve support, backlinks-based references, completion for note links, go-to-definition for note links, heading anchors, and nested sub-anchors, and end-to-end stdio integration coverage.
- Added `check_vault` MCP tool: reports duplicate IDs, duplicate aliases, and broken links — equivalent to the CLI's `check` command. Accepts an optional `ignore` parameter (list of vault-relative glob patterns to exclude).
- Added `Vault::check(filter: impl Fn(&Path) -> bool) -> VaultHealthReport` to `obsidian-rs-core`. Returns a `VaultHealthReport` with `duplicate_ids`, `duplicate_aliases`, and `broken_links` fields. Health report types (`VaultHealthReport`, `DuplicateId`, `DuplicateAlias`, `BrokenLink`, `NoteRef`) are exported from the `health` module.

### Fixed

- Markdown links with percent-encoded URLs (e.g. `[text](My%20Note.md)`) are now correctly resolved for backlink detection, health checks, and rename/merge operations.

## v0.2.0 - 2026-04-11

### Added

- Added `sort` parameter to the `search_notes` and `search_tags` MCP tools, matching the CLI's sort options (`path-asc`, `path-desc`, `modified-asc`, `modified-desc`, `created-asc`, `created-desc`).
- Added `note list` command to `obsidian-rs-cli` and `list_notes` tool to `obsidian-rs-mcp`.
- Added `SearchQuery::with_loaded_notes(&HashMap<PathBuf, Note>)` builder method: in-memory notes shadow their on-disk counterparts and are processed through all filters; notes with no on-disk counterpart are included as additional candidates. `SearchQuery` now carries a lifetime parameter (`SearchQuery<'a>`) reflecting the borrow.
- Added `loaded_notes: Option<&HashMap<PathBuf, Note>>` parameter to `find_all_tags`, `find_tags`, `find_notes_filtered`, and `find_notes_filtered_with_content` in `obsidian_core::search`.
- Added `Vault::load_note(note: Note)` and `Vault::unload_note(path: &Path)` to manage in-memory note overrides. Loaded notes are automatically included in `search()`, `list_tags()`, `find_tags()`, `notes_filtered()`, and `notes_filtered_with_content()`.
- `Note` now derives `Clone`.
- Added `Vault.rename_tag()` method.
- Added `content_matches` option to MCP `search_notes` tool.

### Changed

- Consolidated sorting functionality into `obsidian_core::search` module.
- Made sorting optional in the CLI.
- `Vault.path` field is now private. Use accessing method `Vault.path()` instead.
- Renamed `Note` "content" fields/methods to "body".

## v0.1.1 - 2026-03-26

Streamlined release process and added LSP workspace crate boilerplate.

## v0.1.0 - 2026-03-26

Initial release of `obsidian-rs-core`, `obsidian-rs-cli`, and `obsidian-rs-mcp`.
