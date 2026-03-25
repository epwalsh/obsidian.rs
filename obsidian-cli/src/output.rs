use std::path::Path;

use color_eyre::eyre;
use colored::Colorize;
use obsidian_core::{Link, LocatedLink, MergePreview, Note, RenamePreview, search};
use serde::Serialize;
use serde_json::json;

pub fn get_rel_path(path: impl AsRef<Path>, vault_path: &Path) -> String {
    let rel = if let Ok(cwd) = std::env::current_dir() {
        path.as_ref().strip_prefix(cwd).unwrap_or(path.as_ref())
    } else {
        path.as_ref().strip_prefix(vault_path).unwrap_or(path.as_ref())
    };
    rel.display().to_string()
}

pub fn get_note_json(note: &Note, vault_path: &Path) -> eyre::Result<serde_json::Map<String, serde_json::Value>> {
    let rel = get_rel_path(&note.path, vault_path);
    let mut json = note.frontmatter_json()?;
    json.insert("path".to_string(), serde_json::Value::String(rel));
    Ok(json)
}

pub fn print_note_json(note: &Note, vault_path: &Path) -> eyre::Result<()> {
    let out = get_note_json(note, vault_path)?;
    println!("{}", serde_json::to_string(&out).unwrap());
    Ok(())
}

pub fn print_note_many_json(notes: &[Note], vault_path: &Path) -> eyre::Result<()> {
    let items: eyre::Result<Vec<serde_json::Map<String, serde_json::Value>>> =
        notes.iter().map(|n| get_note_json(n, vault_path)).collect();
    println!("{}", serde_json::to_string(&items?).unwrap());
    Ok(())
}

pub fn print_note_plain(note: &Note, vault_path: &Path) {
    println!("{}", get_rel_path(&note.path, vault_path).cyan());
}

pub fn print_note_many_plain(notes: &[Note], vault_path: &Path) {
    for note in notes {
        let rel = get_rel_path(&note.path, vault_path);
        println!("{}", rel.cyan());
    }
}

pub fn print_note_read_plain(note: &Note, frontmatter: bool, no_content: bool) -> eyre::Result<()> {
    if no_content {
        let fm = note.frontmatter_string()?;
        println!("---\n{}---", fm);
    } else {
        let content = note.read(frontmatter)?;
        println!("{}", content);
    }
    Ok(())
}

pub fn print_note_read_json(note: &Note, frontmatter: bool, no_content: bool) -> eyre::Result<()> {
    // Build up raw content
    let mut content = json!({});
    if frontmatter || no_content {
        content["frontmatter"] = serde_json::Value::Object(note.frontmatter_json()?);
    }
    if !no_content {
        content["content"] = json!(note.read(false)?);
    }
    println!("{}", serde_json::to_string(&content)?);
    Ok(())
}

pub fn print_backlinks_plain(results: &[(Note, Vec<LocatedLink>)], vault_path: &Path) {
    for (note, _) in results {
        let rel = get_rel_path(&note.path, vault_path);
        println!("{}", rel.cyan());
    }
}

#[derive(Serialize)]
struct LinkJson {
    kind: &'static str,
    target: String,
    display: String,
    line: usize,
    col_start: usize,
    col_end: usize,
}

#[derive(Serialize)]
struct BacklinkJson<'a> {
    source_path: String,
    source_id: &'a str,
    links: Vec<LinkJson>,
}

pub fn print_rename_preview_plain(preview: &RenamePreview, vault_path: &Path) {
    let rel_new = get_rel_path(&preview.new_path, vault_path);
    println!("{}", rel_new.cyan().bold());
    for (path, count) in &preview.updated_notes {
        let rel = get_rel_path(path, vault_path);
        let link_word = if *count == 1 { "link" } else { "links" };
        let count_str = format!("({} {})", count, link_word).dimmed();
        println!(" {} {} {}", "➡️update:".green(), rel.cyan(), count_str);
    }
}

#[derive(Serialize)]
struct RenamePreviewNoteJson {
    path: String,
    link_count: usize,
}

#[derive(Serialize)]
struct RenamePreviewJson {
    new_path: String,
    id_will_update: bool,
    updated_notes: Vec<RenamePreviewNoteJson>,
}

pub fn print_rename_preview_json(preview: &RenamePreview, vault_path: &Path) {
    let rel_new = get_rel_path(&preview.new_path, vault_path);
    let updated_notes = preview
        .updated_notes
        .iter()
        .map(|(path, count)| {
            let rel = get_rel_path(path, vault_path);
            RenamePreviewNoteJson {
                path: rel,
                link_count: *count,
            }
        })
        .collect();
    let out = RenamePreviewJson {
        new_path: rel_new,
        id_will_update: preview.id_will_update,
        updated_notes,
    };
    println!("{}", serde_json::to_string(&out).unwrap());
}

pub fn print_merge_preview_plain(preview: &MergePreview, vault_path: &Path) {
    let rel_dest = get_rel_path(&preview.dest_path, vault_path);
    let new_label = if preview.dest_is_new { "  (new)" } else { "" };
    println!("{}{}", rel_dest.cyan().bold(), new_label.dimmed());
    for src in &preview.sources {
        let rel = get_rel_path(src, vault_path);
        println!(" {} {}", "delete:".yellow(), rel.cyan());
    }
    for (path, count) in &preview.updated_notes {
        let rel = get_rel_path(path, vault_path);
        let link_word = if *count == 1 { "link" } else { "links" };
        let count_str = format!("({} {})", count, link_word).dimmed();
        println!(" {} {} {}", "update:".green(), rel.cyan(), count_str);
    }
}

#[derive(Serialize)]
struct MergePreviewNoteJson {
    path: String,
    link_count: usize,
}

#[derive(Serialize)]
struct MergePreviewJson {
    dest_path: String,
    dest_is_new: bool,
    sources: Vec<String>,
    updated_notes: Vec<MergePreviewNoteJson>,
}

pub fn print_merge_preview_json(preview: &MergePreview, vault_path: &Path) {
    let rel_dest = get_rel_path(&preview.dest_path, vault_path);
    let sources = preview.sources.iter().map(|s| get_rel_path(s, vault_path)).collect();
    let updated_notes = preview
        .updated_notes
        .iter()
        .map(|(path, count)| {
            let rel = get_rel_path(path, vault_path);
            MergePreviewNoteJson {
                path: rel,
                link_count: *count,
            }
        })
        .collect();
    let out = MergePreviewJson {
        dest_path: rel_dest,
        dest_is_new: preview.dest_is_new,
        sources,
        updated_notes,
    };
    println!("{}", serde_json::to_string(&out).unwrap());
}

pub fn print_tags_search_plain(results: &[search::NoteTags], vault_path: &Path) {
    for nt in results {
        let rel = get_rel_path(&nt.path, vault_path);
        println!("{}", rel.cyan());
        if !nt.frontmatter_tags.is_empty() {
            let tags_colored: Vec<String> = nt.frontmatter_tags.iter().map(|t| t.yellow().to_string()).collect();
            println!("  {} {}", "[frontmatter]".dimmed(), tags_colored.join(", "));
        }
        for lt in &nt.inline_tags {
            let marker = format!("[{}:{}]", lt.location.line, lt.location.col_start);
            println!("  {} #{}", marker.dimmed(), lt.tag.yellow());
        }
    }
}

#[derive(Serialize)]
struct InlineTagJson {
    tag: String,
    line: usize,
    col_start: usize,
    col_end: usize,
}

#[derive(Serialize)]
struct TagsSearchResultJson {
    path: String,
    frontmatter_tags: Vec<String>,
    inline_occurrences: Vec<InlineTagJson>,
}

pub fn print_tags_search_json(results: &[search::NoteTags], vault_path: &Path) {
    let items: Vec<TagsSearchResultJson> = results
        .iter()
        .map(|nt| {
            let rel = get_rel_path(&nt.path, vault_path);
            TagsSearchResultJson {
                path: rel,
                frontmatter_tags: nt.frontmatter_tags.clone(),
                inline_occurrences: nt
                    .inline_tags
                    .iter()
                    .map(|lt| InlineTagJson {
                        tag: lt.tag.clone(),
                        line: lt.location.line,
                        col_start: lt.location.col_start,
                        col_end: lt.location.col_end,
                    })
                    .collect(),
            }
        })
        .collect();
    println!("{}", serde_json::to_string(&items).unwrap());
}

pub fn print_tags_list_plain(tags: &[String]) {
    for tag in tags {
        println!("{}", tag.yellow());
    }
}

pub fn print_tags_list_json(tags: &[String]) {
    println!("{}", serde_json::to_string(tags).unwrap());
}

pub fn print_backlinks_json(results: &[(Note, Vec<LocatedLink>)], vault_path: &Path) {
    let items: Vec<BacklinkJson> = results
        .iter()
        .map(|(note, links)| {
            let rel = get_rel_path(&note.path, vault_path);
            let link_jsons = links
                .iter()
                .map(|ll| {
                    let (kind, mut target, heading, display) = match &ll.link {
                        Link::Wiki {
                            target, heading, alias, ..
                        } => (
                            "wiki",
                            target.clone(),
                            heading.clone(),
                            alias.clone().unwrap_or(target.clone()),
                        ),
                        Link::Markdown { url, text, .. } => ("markdown", url.clone(), None, text.clone()),
                        Link::Embed {
                            target, heading, alias, ..
                        } => (
                            "embed",
                            target.clone(),
                            heading.clone(),
                            alias.clone().unwrap_or(target.clone()),
                        ),
                    };
                    if let Some(h) = heading {
                        target = format!("{}#{}", target, h);
                    }
                    LinkJson {
                        kind,
                        target,
                        display,
                        line: ll.location.line,
                        col_start: ll.location.col_start,
                        col_end: ll.location.col_end,
                    }
                })
                .collect();
            BacklinkJson {
                source_path: rel,
                source_id: &note.id,
                links: link_jsons,
            }
        })
        .collect();
    println!("{}", serde_json::to_string(&items).unwrap());
}
