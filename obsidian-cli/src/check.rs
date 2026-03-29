use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use color_eyre::eyre;
use colored::Colorize;
use globset::{GlobBuilder, GlobSetBuilder};
use obsidian_core::{Link, Vault};

use crate::args::CheckArgs;

fn build_ignore_set(patterns: &[String]) -> eyre::Result<globset::GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| eyre::eyre!("invalid ignore pattern '{}': {}", pattern, e))?;
        builder.add(glob);
    }
    Ok(builder.build()?)
}

pub fn cmd_check(vault: Vault, args: CheckArgs) -> eyre::Result<()> {
    println!("{} {}", "Vault root:".bold(), vault.path().display());

    let ignore_set = build_ignore_set(&args.ignore)?;

    let notes: Vec<_> = vault
        .notes_filtered(|path| {
            let rel = path.strip_prefix(vault.path()).unwrap_or(path);
            !ignore_set.is_match(rel)
        })
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    println!("{} {}", "Notes:     ".bold(), notes.len());
    println!();

    let mut has_issues = false;

    // --- Duplicate IDs ---
    let mut id_map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for note in notes.iter() {
        id_map.entry(note.id.clone()).or_default().push(note.path.clone());
    }
    let mut dup_ids: Vec<(&String, &Vec<PathBuf>)> = id_map.iter().filter(|(_, paths)| paths.len() > 1).collect();
    dup_ids.sort_by_key(|(id, _)| id.as_str());

    if dup_ids.is_empty() {
        println!("{}", "✓ No duplicate IDs".green());
    } else {
        has_issues = true;
        println!("{}", "✘ Duplicate IDs:".red().bold());
        for (id, paths) in dup_ids {
            println!("  {}", id.yellow());
            let mut sorted = paths.clone();
            sorted.sort();
            for path in &sorted {
                let note = notes.iter().find(|n| &n.path == path).unwrap();
                let backlink_count = vault.backlinks_from(&notes, note).len();
                let rel = path.strip_prefix(vault.path()).unwrap_or(path);
                println!(
                    "    {} ({} backlinks)",
                    rel.display().to_string().cyan(),
                    backlink_count
                );
            }
        }
    }

    // --- Duplicate aliases ---
    let mut alias_map: HashMap<String, HashSet<PathBuf>> = HashMap::new();
    for note in notes.iter() {
        for alias in &note.aliases {
            alias_map
                .entry(alias.to_lowercase())
                .or_default()
                .insert(note.path.clone());
        }
    }
    let mut dup_aliases: Vec<(&String, &HashSet<PathBuf>)> =
        alias_map.iter().filter(|(_, paths)| paths.len() > 1).collect();
    dup_aliases.sort_by_key(|(alias, _)| alias.as_str());

    if dup_aliases.is_empty() {
        println!("{}", "✓ No duplicate aliases".green());
    } else {
        has_issues = true;
        println!("{}", "✘ Duplicate aliases:".red().bold());
        for (alias, paths) in dup_aliases {
            println!("  {}", alias.yellow());
            let mut sorted = paths.iter().cloned().collect::<Vec<_>>();
            sorted.sort();
            for path in &sorted {
                let note = notes.iter().find(|n| &n.path == path).unwrap();
                let backlink_count = vault.backlinks_from(&notes, note).len();
                let rel = path.strip_prefix(vault.path()).unwrap_or(path);
                println!(
                    "    {} ({} backlinks)",
                    rel.display().to_string().cyan(),
                    backlink_count
                );
            }
        }
    }

    // --- Broken links ---
    let mut valid_wiki_targets: HashSet<String> = HashSet::new();
    for note in notes.iter() {
        valid_wiki_targets.insert(note.id.clone());
        if let Some(stem) = note.path.file_stem().and_then(|s| s.to_str()) {
            valid_wiki_targets.insert(stem.to_string());
        }
        for alias in &note.aliases {
            valid_wiki_targets.insert(alias.clone());
            valid_wiki_targets.insert(alias.to_lowercase());
        }
    }

    let mut broken: Vec<(PathBuf, usize, String)> = Vec::new();
    for note in notes.iter() {
        for ll in &note.links {
            match &ll.link {
                Link::Wiki { target, .. } => {
                    if !target.is_empty() && !valid_wiki_targets.contains(target.as_str()) {
                        broken.push((note.path.clone(), ll.location.line, format!("[[{}]]", target)));
                    }
                }
                Link::Markdown { url, .. } => {
                    // Skip external and absolute links; only check local .md links.
                    if url.contains("://") || url.starts_with('/') {
                        continue;
                    }
                    let url_path = match url.find('#') {
                        Some(i) => &url[..i],
                        None => url.as_str(),
                    };
                    if !url_path.ends_with(".md") {
                        continue;
                    }
                    let source_dirs = [vault.path(), note.path.parent().unwrap_or(note.path.as_path())];
                    if !source_dirs.iter().any(|dir| dir.join(url_path).exists()) {
                        broken.push((note.path.clone(), ll.location.line, format!("[...]({})", url)));
                    }
                }
                _ => {}
            }
        }
    }

    broken.sort_by(|(a, al, _), (b, bl, _)| a.cmp(b).then(al.cmp(bl)));

    if broken.is_empty() {
        println!("{}", "✓ No broken links".green());
    } else {
        has_issues = true;
        println!("{}", "✘ Broken links:".red().bold());
        let mut current_path: Option<&PathBuf> = None;
        for (path, line, text) in &broken {
            if current_path != Some(path) {
                current_path = Some(path);
                let rel = path.strip_prefix(vault.path()).unwrap_or(path);
                println!("  {}", rel.display().to_string().cyan());
            }
            println!("    {} {}", format!("line {}:", line).dimmed(), text.yellow());
        }
    }

    if has_issues {
        std::process::exit(1);
    }
    Ok(())
}
