use std::path::Path;

use obsidian_core::{Link, LocatedLink, Note, RenamePreview};
use serde::Serialize;

pub fn print_search_plain(notes: &[Note], vault_path: &Path) {
    for note in notes {
        let rel = note.path.strip_prefix(vault_path).unwrap_or(&note.path);
        println!("{}", rel.display());
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
        println!("{}", rel.display());
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
    println!("{}", rel_new.display());
    for (path, count) in &preview.updated_notes {
        let rel = path.strip_prefix(vault_path).unwrap_or(path);
        println!(
            " ➡️ update: {} ({} link{})",
            rel.display(),
            count,
            if *count == 1 { "" } else { "s" }
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
