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
    write_note(
        vault.path(),
        "tagged.md",
        "---\ntags: [rust]\n---\nContent.",
    );
    write_note(vault.path(), "untagged.md", "No tags.");
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--tag",
            "rust",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("tagged.md"))
        .stdout(predicate::str::contains("untagged.md").not());
}

#[test]
fn search_tag_and_semantics() {
    let vault = make_vault();
    write_note(
        vault.path(),
        "both.md",
        "---\ntags: [rust, obsidian]\n---\nContent.",
    );
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
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
            "--regex",
            r"\d+",
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
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "search",
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
    assert_eq!(v.as_array().unwrap().len(), 1);
}

#[test]
fn search_nonexistent_vault_exits_with_error() {
    obsidian()
        .args(["--vault", "/nonexistent/vault/path", "search"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error"));
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
fn backlinks_nonexistent_note_exits_with_error() {
    let vault = make_vault();
    obsidian()
        .args([
            "--vault",
            vault.path().to_str().unwrap(),
            "backlinks",
            "/nonexistent/note.md",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error"));
}
