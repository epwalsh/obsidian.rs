.PHONY : checks
checks :
	cargo fmt --check
	cargo check
	cargo clippy
	cargo test

.PHONY : install
install :
	cargo install --bin obsidian --path obsidian-cli/
