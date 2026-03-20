use std::io::{BufRead, IsTerminal};

use color_eyre::eyre;
use colored::Colorize;
use obsidian_core::{Note, Vault};

use crate::args::{BacklinksArgs, OutputFormat, RenameArgs, UpdateArgs};
use crate::output;
use crate::utils::{resolve_note_path, sort_notes_by};

pub fn cmd_backlinks(vault: Vault, args: BacklinksArgs) -> eyre::Result<()> {
    let (note_path, _) = resolve_note_path(&vault, &args.note)?;
    let note = Note::from_path(&note_path)?;
    let mut results = vault.backlinks(&note);
    sort_notes_by(&mut results, |(n, _)| &n.path, &args.sort);

    match args.format {
        OutputFormat::Plain => output::print_backlinks_plain(&results, &vault.path),
        OutputFormat::Json => output::print_backlinks_json(&results, &vault.path),
    }
    Ok(())
}

pub fn cmd_rename(vault: Vault, args: RenameArgs) -> eyre::Result<()> {
    let (note_path, root) = resolve_note_path(&vault, &args.note)?;
    let note = Note::from_path(&note_path)?;

    let mut new_path = if args.new_path.is_absolute() {
        args.new_path.clone()
    } else {
        root.join(&args.new_path)
    };
    if new_path.extension().and_then(|e| e.to_str()) != Some("md") {
        new_path.set_extension("md");
    }

    if args.dry_run {
        let preview = vault.rename_preview(&note, &new_path)?;
        match args.format {
            OutputFormat::Plain => output::print_rename_preview_plain(&preview, &vault.path),
            OutputFormat::Json => output::print_rename_preview_json(&preview, &vault.path),
        }
    } else {
        let renamed = vault.rename(&note, &new_path)?;
        let rel = renamed.path.strip_prefix(&vault.path).unwrap_or(&renamed.path);
        println!("{}", rel.display().to_string().cyan());
    }
    Ok(())
}

pub fn cmd_note_update(vault: Vault, args: UpdateArgs) -> eyre::Result<()> {
    if let Some(note_path) = args.note {
        let note = update_single_note(&vault, &note_path, &args.tag)?;
        match args.format {
            OutputFormat::Plain => output::print_note_update_plain(&note, &vault.path),
            OutputFormat::Json => output::print_note_update_json(&note, &vault.path),
        }
        return Ok(());
    }

    if std::io::stdin().is_terminal() {
        eyre::bail!("no note path provided and stdin is a TTY");
    }

    let stdin = std::io::stdin();
    let mut notes = Vec::new();
    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let note = update_single_note(&vault, std::path::Path::new(&line), &args.tag)?;
        notes.push(note);
    }

    match args.format {
        OutputFormat::Plain => {
            for note in &notes {
                output::print_note_update_plain(note, &vault.path);
            }
        }
        OutputFormat::Json => output::print_note_update_many_json(&notes, &vault.path),
    }
    Ok(())
}

fn update_single_note(vault: &Vault, note_arg: &std::path::Path, tags: &[String]) -> eyre::Result<Note> {
    let (note_path, _) = resolve_note_path(vault, &note_arg.to_path_buf())?;
    let mut note = Note::from_path(&note_path)?;

    let new_tags: Vec<String> = tags.iter().filter(|t| !note.tags.contains(*t)).cloned().collect();
    if !new_tags.is_empty() {
        let fm = note.frontmatter.get_or_insert_with(indexmap::IndexMap::new);
        let tags_entry = fm
            .entry("tags".to_string())
            .or_insert_with(|| gray_matter::Pod::Array(Vec::new()));
        if let gray_matter::Pod::Array(arr) = tags_entry {
            for tag in &new_tags {
                arr.push(gray_matter::Pod::String(tag.clone()));
            }
        }
        note.tags.extend(new_tags);
        note.write()?;
    }

    Ok(note)
}
