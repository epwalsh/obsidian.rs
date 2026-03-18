use std::path::Path;

use obsidian_core::{Link, LocatedLink, Note};
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
