use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn obsidian() -> Command {
    Command::cargo_bin("obsidian").unwrap()
}

fn make_vault() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn write_note(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn search_no_filters_returns_all_notes() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "Note A.");
    write_note(vault.path(), "b.md", "Note B.");
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "search"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.md"))
        .stdout(predicate::str::contains("b.md"));
}

#[test]
fn search_tag_filter() {
    let vault = make_vault();
    write_note(vault.path(), "tagged.md", "---\ntags: [rust]\n---\nContent.");
    write_note(vault.path(), "untagged.md", "No tags.");
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "search", "--tag", "rust"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tagged.md"))
        .stdout(predicate::str::contains("untagged.md").not());
}

#[test]
fn search_tag_and_semantics() {
    let vault = make_vault();
    write_note(vault.path(), "both.md", "---\ntags: [rust, obsidian]\n---\nContent.");
    write_note(vault.path(), "one.md", "---\ntags: [rust]\n---\nContent.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--tag",
            "rust",
            "--tag",
            "obsidian",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("both.md"))
        .stdout(predicate::str::contains("one.md").not());
}

#[test]
fn search_glob_filter() {
    let vault = make_vault();
    write_note(vault.path(), "notes/a.md", "Note A.");
    write_note(vault.path(), "journal/b.md", "Note B.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--glob",
            "notes/**",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("notes/a.md"))
        .stdout(predicate::str::contains("journal/b.md").not());
}

#[test]
fn search_content_filter() {
    let vault = make_vault();
    write_note(vault.path(), "match.md", "This mentions ferris.");
    write_note(vault.path(), "no-match.md", "Nothing special.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--content",
            "ferris",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("match.md"))
        .stdout(predicate::str::contains("no-match.md").not());
}

#[test]
fn search_regex_filter() {
    let vault = make_vault();
    write_note(vault.path(), "match.md", "Score: 42 points.");
    write_note(vault.path(), "no-match.md", "No numbers here.");
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "search", "--regex", r"\d+"])
        .assert()
        .success()
        .stdout(predicate::str::contains("match.md"))
        .stdout(predicate::str::contains("no-match.md").not());
}

#[test]
fn search_alias_exact_filter() {
    let vault = make_vault();
    write_note(vault.path(), "match.md", "---\naliases: [My Alias]\n---\nContent.");
    write_note(vault.path(), "no-match.md", "No aliases.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--alias",
            "my alias",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("match.md"))
        .stdout(predicate::str::contains("no-match.md").not());
}

#[test]
fn search_alias_or_semantics() {
    let vault = make_vault();
    write_note(vault.path(), "alpha.md", "---\naliases: [alias-alpha]\n---\nContent.");
    write_note(vault.path(), "beta.md", "---\naliases: [alias-beta]\n---\nContent.");
    write_note(vault.path(), "gamma.md", "No aliases.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--alias",
            "alias-alpha",
            "--alias",
            "alias-beta",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("alpha.md"))
        .stdout(predicate::str::contains("beta.md"))
        .stdout(predicate::str::contains("gamma.md").not());
}

#[test]
fn search_alias_contains_filter() {
    let vault = make_vault();
    write_note(
        vault.path(),
        "match.md",
        "---\naliases: [Rust Programming]\n---\nContent.",
    );
    write_note(vault.path(), "no-match.md", "No aliases.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--alias-contains",
            "rust",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("match.md"))
        .stdout(predicate::str::contains("no-match.md").not());
}

#[test]
fn search_invalid_regex_exits_with_error() {
    let vault = make_vault();
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--regex",
            "[invalid",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error"));
}

#[test]
fn search_json_format() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntags: [rust]\n---\nContent.");
    let output = obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "search", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.is_array());
    assert_eq!(v.as_array().unwrap().len(), 1);
}

#[test]
fn search_nonexistent_vault_exits_with_error() {
    obsidian()
        .args(["--vault", "/nonexistent/vault/path", "search"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a directory"));
}

#[test]
fn search_output_is_sorted() {
    let vault = make_vault();
    write_note(vault.path(), "z.md", "Note Z.");
    write_note(vault.path(), "a.md", "Note A.");
    write_note(vault.path(), "m.md", "Note M.");
    let output = obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "search"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
}

#[test]
fn backlinks_no_links_returns_empty() {
    let vault = make_vault();
    write_note(vault.path(), "target.md", "Target.");
    write_note(vault.path(), "other.md", "No links.");
    let note_path = vault.path().join("target.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "backlinks",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn backlinks_finds_wiki_links() {
    let vault = make_vault();
    write_note(vault.path(), "target.md", "Target.");
    write_note(vault.path(), "source.md", "See [[target]].");
    let note_path = vault.path().join("target.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "backlinks",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("source.md"));
}

#[test]
fn backlinks_finds_markdown_links() {
    let vault = make_vault();
    write_note(vault.path(), "target.md", "Target.");
    write_note(vault.path(), "source.md", "[link](target.md)");
    let note_path = vault.path().join("target.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "backlinks",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("source.md"));
}

#[test]
fn backlinks_json_format() {
    let vault = make_vault();
    write_note(vault.path(), "target.md", "Target.");
    write_note(vault.path(), "source.md", "See [[target]].");
    let note_path = vault.path().join("target.md");
    let output = obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "backlinks",
            "--format",
            "json",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.is_array());
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let item = &arr[0];
    assert!(item["source_path"].as_str().unwrap().contains("source.md"));
    assert!(item["links"].is_array());
    let links = item["links"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["kind"].as_str().unwrap(), "wiki");
}

#[test]
fn rename_renames_note() {
    let vault = make_vault();
    write_note(vault.path(), "old.md", "Content.");
    let note_path = vault.path().join("old.md");
    let new_path = vault.path().join("new.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("new.md"));
    assert!(!vault.path().join("old.md").exists());
    assert!(vault.path().join("new.md").exists());
}

#[test]
fn rename_updates_backlinks() {
    let vault = make_vault();
    write_note(vault.path(), "target.md", "Target.");
    write_note(vault.path(), "source.md", "See [[target]].");
    let note_path = vault.path().join("target.md");
    let new_path = vault.path().join("renamed.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let source = fs::read_to_string(vault.path().join("source.md")).unwrap();
    assert_eq!(source, "See [[renamed]].");
}

#[test]
fn rename_adds_md_extension_if_missing() {
    let vault = make_vault();
    write_note(vault.path(), "old.md", "Content.");
    let note_path = vault.path().join("old.md");
    // Pass path without .md extension — CLI should add it
    let new_path = vault.path().join("new");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("new.md"));
    assert!(vault.path().join("new.md").exists());
}

#[test]
fn rename_moves_to_subdirectory() {
    let vault = make_vault();
    write_note(vault.path(), "root.md", "Root.");
    write_note(vault.path(), "source.md", "[link](root.md)");
    fs::create_dir(vault.path().join("sub")).unwrap();
    let note_path = vault.path().join("root.md");
    let new_path = vault.path().join("sub/root.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(!vault.path().join("root.md").exists());
    assert!(vault.path().join("sub/root.md").exists());
    let source = fs::read_to_string(vault.path().join("source.md")).unwrap();
    assert_eq!(source, "[link](sub/root.md)");
}

#[test]
fn rename_nonexistent_note_exits_with_error() {
    let vault = make_vault();
    let new_path = vault.path().join("new.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            "/nonexistent/note.md",
            new_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("note not found"));
}

#[test]
fn rename_target_directory_not_found_exits_with_error() {
    let vault = make_vault();
    write_note(vault.path(), "old.md", "Old.");
    let note_path = vault.path().join("old.md");
    let new_path = vault.path().join("nonexistent/new.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("directory not found"));
}

#[test]
fn rename_target_already_exists_exits_with_error() {
    let vault = make_vault();
    write_note(vault.path(), "old.md", "Old.");
    write_note(vault.path(), "new.md", "Already exists.");
    let note_path = vault.path().join("old.md");
    let new_path = vault.path().join("new.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

// --- dry-run tests ---

#[test]
fn rename_dry_run_does_not_rename_file() {
    let vault = make_vault();
    write_note(vault.path(), "old.md", "Content.");
    let note_path = vault.path().join("old.md");
    let new_path = vault.path().join("new.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            "--dry-run",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(vault.path().join("old.md").exists());
    assert!(!vault.path().join("new.md").exists());
}

#[test]
fn rename_dry_run_does_not_modify_backlinks() {
    let vault = make_vault();
    write_note(vault.path(), "target.md", "Target.");
    write_note(vault.path(), "source.md", "See [[target]].");
    let note_path = vault.path().join("target.md");
    let new_path = vault.path().join("renamed.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            "--dry-run",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let source = fs::read_to_string(vault.path().join("source.md")).unwrap();
    assert_eq!(source, "See [[target]].");
}

#[test]
fn rename_dry_run_outputs_new_path() {
    let vault = make_vault();
    write_note(vault.path(), "old.md", "Content.");
    let note_path = vault.path().join("old.md");
    let new_path = vault.path().join("new.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            "--dry-run",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("new.md"));
}

#[test]
fn rename_dry_run_outputs_updated_notes() {
    let vault = make_vault();
    write_note(vault.path(), "target.md", "Target.");
    write_note(vault.path(), "source.md", "See [[target]].");
    let note_path = vault.path().join("target.md");
    let new_path = vault.path().join("renamed.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            "--dry-run",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("source.md"));
}

#[test]
fn rename_dry_run_json_format() {
    let vault = make_vault();
    write_note(vault.path(), "target.md", "Target.");
    write_note(vault.path(), "source.md", "See [[target]].");
    let note_path = vault.path().join("target.md");
    let new_path = vault.path().join("renamed.md");
    let output = obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            "--dry-run",
            "--format",
            "json",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v["new_path"].as_str().unwrap().contains("renamed.md"));
    assert!(v["updated_notes"].is_array());
    let notes = v["updated_notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0]["path"].as_str().unwrap().contains("source.md"));
    assert_eq!(notes[0]["link_count"].as_u64().unwrap(), 1);
}

#[test]
fn rename_dry_run_no_backlinks() {
    let vault = make_vault();
    write_note(vault.path(), "standalone.md", "No links to me.");
    let note_path = vault.path().join("standalone.md");
    let new_path = vault.path().join("new-name.md");
    let output = obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "rename",
            "--dry-run",
            "--format",
            "json",
            note_path.to_str().unwrap(),
            new_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v["updated_notes"].as_array().unwrap().is_empty());
}

#[test]
fn backlinks_nonexistent_note_exits_with_error() {
    let vault = make_vault();
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "backlinks",
            "/nonexistent/note.md",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("note not found"));
}

// --- tags list tests ---

#[test]
fn tags_list_basic() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "---\ntags: [rust, obsidian]\n---\nContent.");
    write_note(vault.path(), "b.md", "Some #inline tag here.");
    write_note(vault.path(), "c.md", "No tags.");
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "tags", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rust"))
        .stdout(predicate::str::contains("obsidian"))
        .stdout(predicate::str::contains("inline"));
}

#[test]
fn tags_list_deduplicated_and_sorted() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "---\ntags: [beta, alpha]\n---\nContent #beta.");
    write_note(vault.path(), "b.md", "---\ntags: [alpha]\n---\nContent.");
    let output = obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "tags", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    // Should be sorted and deduplicated
    assert_eq!(lines, vec!["alpha", "beta"]);
}

#[test]
fn tags_list_empty_vault() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "No tags here.");
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "tags", "list"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn tags_list_json_format() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "---\ntags: [rust]\n---\nContent.");
    let output = obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "tags",
            "list",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.is_array());
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0].as_str().unwrap(), "rust");
}

// --- tags search tests ---

#[test]
fn tags_search_frontmatter_match() {
    let vault = make_vault();
    write_note(vault.path(), "match.md", "---\ntags: [rust]\n---\nContent.");
    write_note(vault.path(), "no-match.md", "---\ntags: [python]\n---\nContent.");
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "tags", "search", "rust"])
        .assert()
        .success()
        .stdout(predicate::str::contains("match.md"))
        .stdout(predicate::str::contains("no-match.md").not())
        .stdout(predicate::str::contains("[frontmatter] rust"));
}

#[test]
fn tags_search_inline_match() {
    let vault = make_vault();
    write_note(vault.path(), "match.md", "See #rust here.");
    write_note(vault.path(), "no-match.md", "No matching tag.");
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "tags", "search", "rust"])
        .assert()
        .success()
        .stdout(predicate::str::contains("match.md"))
        .stdout(predicate::str::contains("no-match.md").not());
}

#[test]
fn tags_search_multiple_tags_or_semantics() {
    let vault = make_vault();
    write_note(vault.path(), "has-foo.md", "---\ntags: [foo]\n---\nContent.");
    write_note(vault.path(), "has-bar.md", "Content with #bar.");
    write_note(vault.path(), "has-neither.md", "No relevant tags.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "tags",
            "search",
            "foo",
            "bar",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("has-foo.md"))
        .stdout(predicate::str::contains("has-bar.md"))
        .stdout(predicate::str::contains("has-neither.md").not());
}

#[test]
fn tags_search_output_sorted() {
    let vault = make_vault();
    write_note(vault.path(), "z.md", "---\ntags: [rust]\n---\nContent.");
    write_note(vault.path(), "a.md", "---\ntags: [rust]\n---\nContent.");
    write_note(vault.path(), "m.md", "---\ntags: [rust]\n---\nContent.");
    let output = obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "tags", "search", "rust"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let note_lines: Vec<&str> = s.lines().filter(|l| !l.starts_with("  ")).collect();
    let mut sorted = note_lines.clone();
    sorted.sort();
    assert_eq!(note_lines, sorted);
}

#[test]
fn tags_search_json_format() {
    let vault = make_vault();
    write_note(
        vault.path(),
        "note.md",
        "---\ntags: [rust]\n---\nContent with #rust inline.",
    );
    let output = obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "tags",
            "search",
            "--format",
            "json",
            "rust",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.is_array());
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let item = &arr[0];
    assert!(item["path"].as_str().unwrap().contains("note.md"));
    assert_eq!(item["frontmatter_tags"].as_array().unwrap().len(), 1);
    assert_eq!(item["frontmatter_tags"][0].as_str().unwrap(), "rust");
    assert_eq!(item["inline_occurrences"].as_array().unwrap().len(), 1);
    let occ = &item["inline_occurrences"][0];
    assert_eq!(occ["tag"].as_str().unwrap(), "rust");
    assert!(occ["line"].as_u64().unwrap() > 0);
}

#[test]
fn tags_search_sub_tag_matched_by_parent() {
    let vault = make_vault();
    write_note(
        vault.path(),
        "has-subtag.md",
        "---\ntags: [workout/upper-body]\n---\nContent with #workout/legs.",
    );
    write_note(vault.path(), "unrelated.md", "---\ntags: [diet]\n---\nContent.");
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "tags", "search", "workout"])
        .assert()
        .success()
        .stdout(predicate::str::contains("has-subtag.md"))
        .stdout(predicate::str::contains("[frontmatter] workout/upper-body"))
        .stdout(predicate::str::contains("#workout/legs"))
        .stdout(predicate::str::contains("unrelated.md").not());
}

#[test]
fn tags_search_no_tags_arg_exits_with_error() {
    let vault = make_vault();
    obsidian()
        .args(["--vault", vault.path().to_str().unwrap(), "tags", "search"])
        .assert()
        .failure();
}

// --- note update tests ---

#[test]
fn note_update_adds_tag_to_existing_frontmatter_tags() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntags: [rust]\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--tag",
            "obsidian",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("note.md"));
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("obsidian"));
    assert!(content.contains("rust"));
}

#[test]
fn note_update_creates_tags_field_when_frontmatter_has_none() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntitle: My Note\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--tag",
            "newtag",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("newtag"));
}

#[test]
fn note_update_creates_frontmatter_when_note_has_none() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "Plain content, no frontmatter.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--tag",
            "newtag",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("newtag"));
    assert!(content.contains("---"));
}

#[test]
fn note_update_idempotent_does_not_duplicate_existing_tag() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntags: [rust]\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--tag",
            "rust",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert_eq!(content.matches("rust").count(), 1);
}

#[test]
fn note_update_multiple_tags() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntags: [existing]\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--tag",
            "alpha",
            "--tag",
            "beta",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("alpha"));
    assert!(content.contains("beta"));
    assert!(content.contains("existing"));
}

#[test]
fn note_update_json_format() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntags: [rust]\n---\nContent.");
    let note_path = vault.path().join("note.md");
    let output = obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--tag",
            "obsidian",
            "--format",
            "json",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v["path"].as_str().unwrap().contains("note.md"));
    let tags = v["tags"].as_array().unwrap();
    let tag_strs: Vec<&str> = tags.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tag_strs.contains(&"rust"));
    assert!(tag_strs.contains(&"obsidian"));
}

#[test]
fn color_and_no_color_are_mutually_exclusive() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "Note A.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "--color",
            "--no-color",
            "search",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--color and --no-color"));
}
