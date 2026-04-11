INSPECTOR_VERSION := 0.21.1
MCP_VAULT := /tmp/obsidian-test-vault
MCP_BIN := ./target/debug/obsidian-mcp
MCP_INSPECTOR := npx --yes @modelcontextprotocol/inspector@$(INSPECTOR_VERSION) --cli $(MCP_BIN)

.PHONY : checks
checks :
	cargo fmt --check
	cargo check
	cargo clippy -- -D warnings
	cargo test
	cargo install --bin obsidian --path obsidian-cli/
	cargo install --bin obsidian-mcp --path obsidian-mcp/
	cargo install --bin obsidian-lsp --path obsidian-lsp/

.PHONY : install
install :
	cargo install --bin obsidian --path obsidian-cli/
	cargo install --bin obsidian-mcp --path obsidian-mcp/
	cargo install --bin obsidian-lsp --path obsidian-lsp/

.PHONY : inspect-mcp
inspect-mcp : install
	npx --yes @modelcontextprotocol/inspector@$(INSPECTOR_VERSION) obsidian-mcp

.PHONY : publish
publish : checks
	./scripts/release.sh

.PHONY : mcp-build
mcp-build :
	cargo build -p obsidian-rs-mcp

.PHONY : mcp-test-vault
mcp-test-vault :
	rm -rf $(MCP_VAULT)
	mkdir -p $(MCP_VAULT)
	printf -- '---\nid: note-a-id\ntitle: Note A\naliases: [aliasA]\ntags: [rust, obsidian]\n---\nThis is note A content. #inline-tag\n' > $(MCP_VAULT)/note-a.md
	printf -- '---\ntags: [python]\n---\nSee [[note-a]] for details.\n' > $(MCP_VAULT)/note-b.md

.PHONY : mcp-test-tools-list
mcp-test-tools-list :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/list | tee /tmp/mcp-out.txt
	@for tool in read_note write_note patch_note update_note search_notes rename_note list_tags search_tags; do \
		grep -q "$$tool" /tmp/mcp-out.txt || { echo "Missing tool: $$tool"; exit 1; }; \
	done

.PHONY : mcp-test-list-tags
mcp-test-list-tags :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name list_tags | tee /tmp/mcp-out.txt
	@for tag in rust obsidian python inline-tag; do \
		grep -q "$$tag" /tmp/mcp-out.txt || { echo "Missing tag: $$tag"; exit 1; }; \
	done

.PHONY : mcp-test-search-tags
mcp-test-search-tags :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name search_tags --tool-arg 'tags=["rust"]' | tee /tmp/mcp-out.txt
	@grep -q note-a.md /tmp/mcp-out.txt

.PHONY : mcp-test-read-note
mcp-test-read-note :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name read_note --tool-arg note=note-a.md | tee /tmp/mcp-out.txt
	@grep -q note-a-id /tmp/mcp-out.txt
	@grep -q rust /tmp/mcp-out.txt

.PHONY : mcp-test-search-notes
mcp-test-search-notes :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name search_notes --tool-arg content_contains=content | tee /tmp/mcp-out.txt
	@grep -q note-a.md /tmp/mcp-out.txt

.PHONY : mcp-test-update-note
mcp-test-update-note :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name update_note --tool-arg note=note-a.md --tool-arg 'add_tags=["ci-test"]' | tee /tmp/mcp-out.txt
	@grep -q ci-test /tmp/mcp-out.txt

.PHONY : mcp-test-patch-note
mcp-test-patch-note :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name patch_note --tool-arg note=note-a.md --tool-arg old_string=content --tool-arg new_string=patched | tee /tmp/mcp-out.txt
	@grep -q note-a.md /tmp/mcp-out.txt

.PHONY : mcp-test-write-note
mcp-test-write-note :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name write_note --tool-arg path=new-note.md --tool-arg content=hello --tool-arg title=New | tee /tmp/mcp-out.txt
	@grep -q new-note.md /tmp/mcp-out.txt
	@test -f $(MCP_VAULT)/new-note.md

.PHONY : mcp-test-rename-note
mcp-test-rename-note :
	@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name rename_note --tool-arg note=new-note.md --tool-arg new_path=renamed-note.md | tee /tmp/mcp-out.txt
	@grep -q renamed-note.md /tmp/mcp-out.txt
	@test -f $(MCP_VAULT)/renamed-note.md
	@test ! -f $(MCP_VAULT)/new-note.md

.PHONY : mcp-test-read-note-error
mcp-test-read-note-error :
	-@OBSIDIAN_VAULT=$(MCP_VAULT) $(MCP_INSPECTOR) --method tools/call --tool-name read_note --tool-arg note=nonexistent.md 2>&1 | tee /tmp/mcp-out.txt
	@grep -q "note not found" /tmp/mcp-out.txt
	@echo "Expected error received for nonexistent note."

# Runs all MCP integration tests in order (some tests depend on state from prior ones).
# Run `make mcp-test-vault` first if you want to reset fixture state.
.PHONY : mcp-test
mcp-test : mcp-build mcp-test-vault
	$(MAKE) mcp-test-tools-list
	$(MAKE) mcp-test-list-tags
	$(MAKE) mcp-test-search-tags
	$(MAKE) mcp-test-read-note
	$(MAKE) mcp-test-search-notes
	$(MAKE) mcp-test-update-note
	$(MAKE) mcp-test-patch-note
	$(MAKE) mcp-test-write-note
	$(MAKE) mcp-test-rename-note
	$(MAKE) mcp-test-read-note-error
