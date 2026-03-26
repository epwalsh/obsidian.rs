.PHONY : checks
checks :
	cargo fmt --check
	cargo check
	cargo clippy -- -D warnings
	cargo test
	cargo install --bin obsidian --path obsidian-cli/
	cargo install --bin obsidian-mcp --path obsidian-mcp/
	cargo install --bin obsidian-lsp --path obsidian-lsp/

.PHONY : 

.PHONY : install
install :
	cargo install --bin obsidian --path obsidian-cli/
	cargo install --bin obsidian-mcp --path obsidian-mcp/
	cargo install --bin obsidian-lsp --path obsidian-lsp/

.PHONY : inspect-mcp
inspect-mcp : install
	npx @modelcontextprotocol/inspector obsidian-mcp

.PHONY : publish
publish : checks
	./scripts/release.sh
