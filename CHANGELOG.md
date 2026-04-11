# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

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
