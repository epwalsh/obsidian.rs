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
            "--content-contains",
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
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--content-matches",
            r"\d+",
        ])
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
            "--or-alias",
            "alias-alpha",
            "--or-alias",
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
            "--content-matches",
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
    let tags = item["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 2);
    let fm_tag = tags.iter().find(|t| t["location"] == "frontmatter").unwrap();
    assert_eq!(fm_tag["tag"].as_str().unwrap(), "rust");
    let inline_tag = tags.iter().find(|t| t["location"].is_object()).unwrap();
    assert_eq!(inline_tag["tag"].as_str().unwrap(), "rust");
    assert!(inline_tag["location"]["line"].as_u64().unwrap() > 0);
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
            "--add-tag",
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
            "--add-tag",
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
            "--add-tag",
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
            "--add-tag",
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
            "--add-tag",
            "alpha",
            "--add-tag",
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
            "--add-tag",
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
fn note_update_rm_tag_removes_existing_tag() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntags: [rust, obsidian]\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--rm-tag",
            "obsidian",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("rust"));
    assert!(!content.contains("obsidian"));
}

#[test]
fn note_update_rm_tag_no_op_when_tag_absent() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntags: [rust]\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--rm-tag",
            "nonexistent",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("rust"));
}

#[test]
fn note_update_add_and_rm_tag_together() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntags: [rust]\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-tag",
            "obsidian",
            "--rm-tag",
            "rust",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("obsidian"));
    assert!(!content.contains("rust"));
}

#[test]
fn note_update_add_alias_adds_to_frontmatter() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\ntitle: My Note\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-alias",
            "my-alias",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("my-alias"));
}

#[test]
fn note_update_add_alias_creates_frontmatter_when_absent() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "Plain content.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-alias",
            "my-alias",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert!(content.contains("my-alias"));
    assert!(content.contains("---"));
}

#[test]
fn note_update_add_alias_idempotent() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "---\naliases: [my-alias]\n---\nContent.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-alias",
            "my-alias",
            note_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert_eq!(content.matches("my-alias").count(), 1);
}

// --- note update stdin tests ---

#[test]
fn note_update_stdin_adds_tag_to_multiple_notes() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "---\ntags: [rust]\n---\nContent.");
    write_note(vault.path(), "b.md", "---\ntags: [python]\n---\nContent.");
    let a_path = vault.path().join("a.md");
    let b_path = vault.path().join("b.md");
    let stdin_input = format!("{}\n{}\n", a_path.display(), b_path.display());
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-tag",
            "obsidian",
        ])
        .write_stdin(stdin_input)
        .assert()
        .success()
        .stdout(predicate::str::contains("a.md"))
        .stdout(predicate::str::contains("b.md"));
    let a_content = fs::read_to_string(&a_path).unwrap();
    let b_content = fs::read_to_string(&b_path).unwrap();
    assert!(a_content.contains("obsidian"));
    assert!(b_content.contains("obsidian"));
}

#[test]
fn note_update_stdin_json_format_returns_array() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "---\ntags: [rust]\n---\nContent.");
    write_note(vault.path(), "b.md", "---\ntags: [python]\n---\nContent.");
    let a_path = vault.path().join("a.md");
    let b_path = vault.path().join("b.md");
    let stdin_input = format!("{}\n{}\n", a_path.display(), b_path.display());
    let output = obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-tag",
            "obsidian",
            "--format",
            "json",
        ])
        .write_stdin(stdin_input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.is_array());
    assert_eq!(v.as_array().unwrap().len(), 2);
    let paths: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.iter().any(|p| p.contains("a.md")));
    assert!(paths.iter().any(|p| p.contains("b.md")));
}

#[test]
fn note_update_stdin_skips_empty_lines() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "Plain content.");
    let a_path = vault.path().join("a.md");
    let stdin_input = format!("\n{}\n\n", a_path.display());
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-tag",
            "newtag",
        ])
        .write_stdin(stdin_input)
        .assert()
        .success()
        .stdout(predicate::str::contains("a.md"));
    let content = fs::read_to_string(&a_path).unwrap();
    assert!(content.contains("newtag"));
}

#[test]
fn note_update_stdin_empty_input_succeeds_with_no_output() {
    let vault = make_vault();
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-tag",
            "newtag",
        ])
        .write_stdin("")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn note_update_stdin_fail_fast_on_bad_path() {
    let vault = make_vault();
    write_note(vault.path(), "good.md", "Content.");
    let good_path = vault.path().join("good.md");
    let stdin_input = format!("{}\n/nonexistent/bad.md\n", good_path.display());
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "update",
            "--add-tag",
            "newtag",
        ])
        .write_stdin(stdin_input)
        .assert()
        .failure()
        .stderr(predicate::str::contains("note not found"));
}

// --- merge tests ---

#[test]
fn merge_basic() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "Content A.");
    write_note(vault.path(), "b.md", "Content B.");
    let a = vault.path().join("a.md");
    let b = vault.path().join("b.md");
    let dest = vault.path().join("combined.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("combined.md"));
    assert!(dest.exists());
    assert!(!a.exists());
    assert!(!b.exists());
    let content = fs::read_to_string(&dest).unwrap();
    assert!(content.contains("Content A."));
    assert!(content.contains("Content B."));
}

#[test]
fn merge_into_existing() {
    let vault = make_vault();
    write_note(vault.path(), "src.md", "Source content.");
    write_note(vault.path(), "dest.md", "Existing content.");
    let src = vault.path().join("src.md");
    let dest = vault.path().join("dest.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            src.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(!src.exists());
    assert!(dest.exists());
    let content = fs::read_to_string(&dest).unwrap();
    assert!(content.contains("Existing content."));
    assert!(content.contains("Source content."));
}

#[test]
fn merge_updates_backlinks() {
    let vault = make_vault();
    write_note(vault.path(), "src.md", "Source.");
    write_note(vault.path(), "other.md", "See [[src]] and [link](src.md).");
    let src = vault.path().join("src.md");
    let dest = vault.path().join("dest.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            src.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success();
    let other = fs::read_to_string(vault.path().join("other.md")).unwrap();
    assert!(other.contains("[[dest]]"));
    assert!(other.contains("dest.md"));
    assert!(!other.contains("[[src]]"));
}

#[test]
fn merge_multiple_sources() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "Body A.");
    write_note(vault.path(), "b.md", "Body B.");
    write_note(vault.path(), "c.md", "Body C.");
    let a = vault.path().join("a.md");
    let b = vault.path().join("b.md");
    let c = vault.path().join("c.md");
    let dest = vault.path().join("combined.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            c.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(!a.exists());
    assert!(!b.exists());
    assert!(!c.exists());
    assert!(dest.exists());
    let content = fs::read_to_string(&dest).unwrap();
    assert!(content.contains("Body A."));
    assert!(content.contains("Body B."));
    assert!(content.contains("Body C."));
}

#[test]
fn merge_tags_unioned() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "---\ntags: [rust]\n---\nBody A.");
    write_note(vault.path(), "b.md", "---\ntags: [obsidian]\n---\nBody B.");
    let a = vault.path().join("a.md");
    let b = vault.path().join("b.md");
    let dest = vault.path().join("combined.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&dest).unwrap();
    assert!(content.contains("rust"));
    assert!(content.contains("obsidian"));
}

#[test]
fn merge_dry_run_no_filesystem_changes() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "Content A.");
    write_note(vault.path(), "b.md", "Content B.");
    let a = vault.path().join("a.md");
    let b = vault.path().join("b.md");
    let dest = vault.path().join("combined.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            "--dry-run",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("combined.md"));
    assert!(a.exists());
    assert!(b.exists());
    assert!(!dest.exists());
}

#[test]
fn merge_dry_run_shows_sources_and_dest() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "Content A.");
    write_note(vault.path(), "b.md", "Content B.");
    let a = vault.path().join("a.md");
    let b = vault.path().join("b.md");
    let dest = vault.path().join("combined.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            "--dry-run",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.md"))
        .stdout(predicate::str::contains("b.md"))
        .stdout(predicate::str::contains("(new)"));
}

#[test]
fn merge_dry_run_json_format() {
    let vault = make_vault();
    write_note(vault.path(), "a.md", "Content A.");
    write_note(vault.path(), "b.md", "Content B.");
    write_note(vault.path(), "other.md", "See [[a]].");
    let a = vault.path().join("a.md");
    let b = vault.path().join("b.md");
    let dest = vault.path().join("combined.md");
    let output = obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            "--dry-run",
            "--format",
            "json",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v["dest_path"].as_str().unwrap().contains("combined.md"));
    assert_eq!(v["dest_is_new"].as_bool().unwrap(), true);
    assert!(v["sources"].as_array().unwrap().len() == 2);
    let updated = v["updated_notes"].as_array().unwrap();
    assert_eq!(updated.len(), 1);
    assert!(updated[0]["path"].as_str().unwrap().contains("other.md"));
    assert_eq!(updated[0]["link_count"].as_u64().unwrap(), 1);
}

#[test]
fn merge_source_is_dest_errors() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "Content.");
    write_note(vault.path(), "other.md", "Content.");
    let note = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            note.to_str().unwrap(),
            note.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("same as destination"));
}

#[test]
fn merge_dest_dir_missing_errors() {
    let vault = make_vault();
    write_note(vault.path(), "src.md", "Content.");
    let src = vault.path().join("src.md");
    let dest = vault.path().join("nonexistent/dest.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            src.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("directory not found"));
}

#[test]
fn merge_adds_md_extension_if_missing() {
    let vault = make_vault();
    write_note(vault.path(), "src.md", "Content.");
    let src = vault.path().join("src.md");
    let dest_no_ext = vault.path().join("combined");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            src.to_str().unwrap(),
            dest_no_ext.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("combined.md"));
    assert!(vault.path().join("combined.md").exists());
}

#[test]
fn merge_dry_run_updated_notes_backlinks() {
    let vault = make_vault();
    write_note(vault.path(), "src.md", "Source.");
    write_note(vault.path(), "linker.md", "See [[src]].");
    let src = vault.path().join("src.md");
    let dest = vault.path().join("dest.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "merge",
            "--dry-run",
            src.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("linker.md"));
    // Dry run: no changes
    let linker = fs::read_to_string(vault.path().join("linker.md")).unwrap();
    assert_eq!(linker, "See [[src]].");
}

// --- note patch tests ---

#[test]
fn note_patch_replaces_string() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "Hello world.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "patch",
            note_path.to_str().unwrap(),
            "--old-string",
            "world",
            "--new-string",
            "Rust",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("note.md"));
    let content = fs::read_to_string(&note_path).unwrap();
    assert_eq!(content, "Hello Rust.");
}

#[test]
fn note_patch_newline_escape_in_new_string() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "Hello world.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "patch",
            note_path.to_str().unwrap(),
            "--old-string",
            "world",
            "--new-string",
            "world\nfoo",
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert_eq!(content, "Hello world\nfoo.");
}

#[test]
fn note_patch_newline_escape_in_old_string() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "Hello\nworld.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "patch",
            note_path.to_str().unwrap(),
            "--old-string",
            "Hello\nworld",
            "--new-string",
            "Goodbye",
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&note_path).unwrap();
    assert_eq!(content, "Goodbye.");
}

#[test]
fn note_patch_string_not_found_fails() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "Hello world.");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "patch",
            note_path.to_str().unwrap(),
            "--old-string",
            "missing",
            "--new-string",
            "replacement",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn note_patch_multiple_matches_fails() {
    let vault = make_vault();
    write_note(vault.path(), "note.md", "foo and foo");
    let note_path = vault.path().join("note.md");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "note",
            "patch",
            note_path.to_str().unwrap(),
            "--old-string",
            "foo",
            "--new-string",
            "bar",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("multiple times"));
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
