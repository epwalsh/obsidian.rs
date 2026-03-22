.PHONY : checks
checks :
	cargo fmt --check
	cargo check
	cargo clippy -- -D warnings
	cargo test

.PHONY : install
install :
	cargo install --bin obsidian --path obsidian-cli/
