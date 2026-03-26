# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- Added `sort` parameter to the `search_notes` and `search_tags` MCP tools, matching the CLI's sort options (`path-asc`, `path-desc`, `modified-asc`, `modified-desc`, `created-asc`, `created-desc`).

### Changed

- Consolidated sorting functionality into `obsidian_core::search` module.
- Made sorting optional in the CLI.

## v0.1.1 - 2026-03-26

Streamlined release process and added LSP workspace crate boilerplate.

## v0.1.0 - 2026-03-26

Initial release of `obsidian-rs-core`, `obsidian-rs-cli`, and `obsidian-rs-mcp`.
