use std::path::Path;

use colored::Colorize;
use obsidian_core::{Link, LocatedLink, LocatedTag, Note, RenamePreview};
use serde::Serialize;

pub fn print_search_plain(notes: &[Note], vault_path: &Path) {
    for note in notes {
        let rel = note.path.strip_prefix(vault_path).unwrap_or(&note.path);
        println!("{}", rel.display().to_string().cyan());
    }
}

#[derive(Serialize)]
struct NoteJson<'a> {
    path: String,
    id: &'a str,
    title: Option<&'a str>,
    tags: &'a [String],
}

pub fn print_search_json(notes: &[Note], vault_path: &Path) {
    let items: Vec<NoteJson> = notes
        .iter()
        .map(|n| {
            let rel = n.path.strip_prefix(vault_path).unwrap_or(&n.path);
            NoteJson {
                path: rel.display().to_string(),
                id: &n.id,
                title: n.title.as_deref(),
                tags: &n.tags,
            }
        })
        .collect();
    println!("{}", serde_json::to_string(&items).unwrap());
}

pub fn print_backlinks_plain(results: &[(Note, Vec<LocatedLink>)], vault_path: &Path) {
    for (note, _) in results {
        let rel = note.path.strip_prefix(vault_path).unwrap_or(&note.path);
        println!("{}", rel.display().to_string().cyan());
    }
}

#[derive(Serialize)]
struct LinkJson {
    kind: &'static str,
    target: String,
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
    let rel_new = preview.new_path.strip_prefix(vault_path).unwrap_or(&preview.new_path);
    println!("{}", rel_new.display().to_string().cyan().bold());
    for (path, count) in &preview.updated_notes {
        let rel = path.strip_prefix(vault_path).unwrap_or(path);
        let link_word = if *count == 1 { "link" } else { "links" };
        let count_str = format!("({} {})", count, link_word).dimmed();
        println!(
            " {} {} {}",
            "➡️update:".green(),
            rel.display().to_string().cyan(),
            count_str
        );
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
    let rel_new = preview.new_path.strip_prefix(vault_path).unwrap_or(&preview.new_path);
    let updated_notes = preview
        .updated_notes
        .iter()
        .map(|(path, count)| {
            let rel = path.strip_prefix(vault_path).unwrap_or(path);
            RenamePreviewNoteJson {
                path: rel.display().to_string(),
                link_count: *count,
            }
        })
        .collect();
    let out = RenamePreviewJson {
        new_path: rel_new.display().to_string(),
        id_will_update: preview.id_will_update,
        updated_notes,
    };
    println!("{}", serde_json::to_string(&out).unwrap());
}

pub fn print_tags_search_plain(results: &[(Note, Vec<String>, Vec<LocatedTag>)], vault_path: &Path) {
    for (note, fm_tags, inline_tags) in results {
        let rel = note.path.strip_prefix(vault_path).unwrap_or(&note.path);
        println!("{}", rel.display().to_string().cyan());
        if !fm_tags.is_empty() {
            let tags_colored: Vec<String> = fm_tags.iter().map(|t| t.yellow().to_string()).collect();
            println!("  {} {}", "[frontmatter]".dimmed(), tags_colored.join(", "));
        }
        for lt in inline_tags {
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

pub fn print_tags_search_json(results: &[(Note, Vec<String>, Vec<LocatedTag>)], vault_path: &Path) {
    let items: Vec<TagsSearchResultJson> = results
        .iter()
        .map(|(note, fm_tags, inline_tags)| {
            let rel = note.path.strip_prefix(vault_path).unwrap_or(&note.path);
            TagsSearchResultJson {
                path: rel.display().to_string(),
                frontmatter_tags: fm_tags.clone(),
                inline_occurrences: inline_tags
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
            let rel = note.path.strip_prefix(vault_path).unwrap_or(&note.path);
            let link_jsons = links
                .iter()
                .map(|ll| {
                    let (kind, target) = match &ll.link {
                        Link::Wiki { target, .. } => ("wiki", target.clone()),
                        Link::Markdown { url, .. } => ("markdown", url.clone()),
                        Link::Embed { target, .. } => ("embed", target.clone()),
                    };
                    LinkJson {
                        kind,
                        target,
                        line: ll.location.line,
                        col_start: ll.location.col_start,
                        col_end: ll.location.col_end,
                    }
                })
                .collect();
            BacklinkJson {
                source_path: rel.display().to_string(),
                source_id: &note.id,
                links: link_jsons,
            }
        })
        .collect();
    println!("{}", serde_json::to_string(&items).unwrap());
}
