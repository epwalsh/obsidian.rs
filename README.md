# obsidian.rs

A collection of tools for working with Obsidian vaults, written in Rust.

- 📦 `obsidian-core`: A Rust library with the core functionality that the other tools build on top of
- ⌨️ `obsidian-cli`: A command-line tool for querying and managing vaults
- 🤖 `obsidian-mcp`: A model context protocol (MCP) server for agents to interact with vaults
- 🔡 `obsidian-lsp`: A language server so you can use your favorite IDE to work on your vault

## Roadmap

- [x] Implement basics of core library
- [x] Basic CLI tool for querying and managing vaults
- [ ] Implement an MCP server
- [ ] Implement an LSP server
- [ ] Integrate into Obsidian.nvim

## On the use of AI

Though AI coding assistants are making remarkable progress, I still firmly believe that building quality software requires human care at each step.
A coding agent is like an overzealous intern with quick fingers who sometimes get things right, mostly get things *almost* right, and sometimes gets things terribly wrong.
As it is, I've only allowed my digital intern to contribute boilerplate, tests, and relatively simple features that were easy to validate.
