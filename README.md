# obsidian.rs

A collection of tools for working with Obsidian vaults, written in Rust.

- 📦 `obsidian-rs-core`: A Rust library with the core functionality that the other tools build on top of
- ⌨️ `obsidian-rs-cli`: A command-line tool for querying and managing vaults
  ```
  cargo install obsidian-rs-cli
  ```
- 🤖 `obsidian-rs-mcp`: A model context protocol (MCP) server for agents to interact with vaults
  ```
  cargo install obsidian-rs-mcp
  claude mcp add obsidian --scope user obsidian-mcp --vault /path/to/my/vault
  ```
- 🔡 `obsidian-rs-lsp`: **(WIP)** A language server so you can use your favorite IDE to work on your vault

## Roadmap

- [x] Implement basics of core library
- [x] Basic CLI tool for querying and managing vaults
- [x] Implement an MCP server
- [ ] More core features with corresponding CLI/MCP methods
  - [ ] Rename tag
- [ ] Add capabilities to the LSP server
  - [ ] Go-to definition

## On the use of AI

Though AI coding assistants are making remarkable progress, I still firmly believe that building quality software requires human care at each step.
A coding agent is like an overzealous intern with quick fingers who sometimes get things right, mostly get things *almost* right, and sometimes gets things terribly wrong.
As it is, I've only allowed my digital intern to contribute boilerplate, tests, and relatively simple features that were easy to validate.
