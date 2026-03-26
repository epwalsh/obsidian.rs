use std::io::{BufRead, IsTerminal, Read};

use color_eyre::eyre;
use obsidian_core::{Note, Vault};

use crate::args::{
    BacklinksArgs, MergeArgs, OutputFormat, PatchArgs, ReadArgs, RenameArgs, ResolveArgs, UpdateArgs, WriteArgs,
};
use crate::output;

pub fn cmd_resolve(vault: Vault, args: ResolveArgs) -> eyre::Result<()> {
    let note = vault.resolve_note(&args.note)?;
    match args.format {
        OutputFormat::Plain => output::print_note_plain(&note, &vault.path),
        OutputFormat::Json => output::print_note_json(&note, &vault.path)?,
    }
    Ok(())
}

pub fn cmd_read(vault: Vault, args: ReadArgs) -> eyre::Result<()> {
    let (note_path, _) = vault.resolve_note_path(&args.note, true)?;
    let note = if args.no_content {
        Note::from_path(&note_path)?
    } else {
        Note::from_path_with_content(&note_path)?
    };

    match args.format {
        OutputFormat::Plain => output::print_note_read_plain(&note, args.frontmatter, args.no_content)?,
        OutputFormat::Json => output::print_note_read_json(&note, args.frontmatter, args.no_content)?,
    }
    Ok(())
}

pub fn cmd_write(vault: Vault, args: WriteArgs) -> eyre::Result<()> {
    let (note_path, _) = vault.resolve_note_path(&args.note, false)?;
    if !args.force && note_path.exists() {
        eyre::bail!("note already exists: {}\nUse --force to overwrite", note_path.display());
    }

    let content = if let Some(c) = args.content {
        c
    } else if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin().lock().read_to_string(&mut buf)?;
        buf
    } else {
        eyre::bail!("no note path provided and stdin is a TTY");
    };

    // Parse note from content, update title, tags, and aliases, then write to disk.
    let mut note = Note::parse(note_path, &content);
    for tag in args.tag {
        note.add_tag(tag);
    }
    for alias in args.alias {
        note.add_alias(alias);
    }
    if let Some(title) = args.title {
        note.title = Some(title.clone());
        note.add_alias(title.clone());
    } else if note.title.is_none() {
        if !note.aliases.is_empty() {
            // If no title but have aliases, use first alias as title
            note.title = Some(note.aliases[0].clone());
        } else {
            eyre::bail!("no title provided and could not infer title from content");
        }
    }
    note.write()?;

    match args.format {
        OutputFormat::Plain => output::print_note_plain(&note, &vault.path),
        OutputFormat::Json => output::print_note_json(&note, &vault.path)?,
    }

    Ok(())
}

pub fn cmd_merge(vault: Vault, args: MergeArgs) -> eyre::Result<()> {
    if args.paths.len() < 2 {
        eyre::bail!("at least one source and one destination are required");
    }
    let dest_arg = args.paths.last().unwrap();
    let source_args = &args.paths[..args.paths.len() - 1];

    // Resolve destination path relative to vault root, adding .md if needed.
    let mut dest_path = if dest_arg.is_absolute() {
        dest_arg.clone()
    } else {
        let cwd = std::env::current_dir()?;
        let candidate = cwd.join(dest_arg);
        if candidate.exists() {
            candidate
        } else {
            vault.path.join(dest_arg)
        }
    };
    if dest_path.extension().and_then(|e| e.to_str()) != Some("md") {
        dest_path.set_extension("md");
    }

    // Resolve and load sources (content required for body concatenation).
    let mut sources: Vec<Note> = Vec::new();
    for src_arg in source_args {
        let (note_path, _) = vault.resolve_note_path(src_arg, true)?;
        sources.push(Note::from_path_with_content(&note_path)?);
    }

    if args.dry_run {
        let preview = vault.merge_preview(&sources, &dest_path)?;
        match args.format {
            OutputFormat::Plain => output::print_merge_preview_plain(&preview, &vault.path),
            OutputFormat::Json => output::print_merge_preview_json(&preview, &vault.path),
        }
    } else {
        let merged = vault.merge(&sources, &dest_path)?;
        match args.format {
            OutputFormat::Plain => output::print_note_plain(&merged, &vault.path),
            OutputFormat::Json => output::print_note_json(&merged, &vault.path)?,
        }
    }
    Ok(())
}

fn unescape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

pub fn cmd_patch(vault: Vault, args: PatchArgs) -> eyre::Result<()> {
    let (note_path, _) = vault.resolve_note_path(&args.note, true)?;
    let note = Note::from_path(&note_path)?;
    let old = unescape(&args.old_string);
    let new = unescape(&args.new_string);
    let patched = vault.patch_note(&note, &old, &new)?;
    match args.format {
        OutputFormat::Plain => output::print_note_plain(&patched, &vault.path),
        OutputFormat::Json => output::print_note_json(&patched, &vault.path)?,
    }
    Ok(())
}

pub fn cmd_backlinks(vault: Vault, args: BacklinksArgs) -> eyre::Result<()> {
    let (note_path, _) = vault.resolve_note_path(&args.note, true)?;
    let note = Note::from_path(&note_path)?;
    let mut results = vault.backlinks(&note);
    if let Some(sort) = args.sort {
        obsidian_core::search::sort_notes_by(&mut results, |(n, _)| Some(n), &sort.into());
    }

    match args.format {
        OutputFormat::Plain => output::print_backlinks_plain(&results, &vault.path),
        OutputFormat::Json => output::print_backlinks_json(&results, &vault.path),
    }
    Ok(())
}

pub fn cmd_rename(vault: Vault, args: RenameArgs) -> eyre::Result<()> {
    let (note_path, root) = vault.resolve_note_path(&args.note, true)?;
    let note = Note::from_path(&note_path)?;

    let mut new_path = if args.new_path.is_absolute() {
        args.new_path.clone()
    } else {
        if let Some(parent) = root {
            parent.join(&args.new_path)
        } else {
            vault.path.join(&args.new_path)
        }
    };

    if new_path.extension().is_none() {
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
        match args.format {
            OutputFormat::Plain => output::print_note_plain(&renamed, &vault.path),
            OutputFormat::Json => output::print_note_json(&renamed, &vault.path)?,
        }
    }
    Ok(())
}

pub fn cmd_update(vault: Vault, args: UpdateArgs) -> eyre::Result<()> {
    if let Some(note_path) = args.note {
        let note = update_single_note(
            &vault,
            &note_path,
            &args.add_tag,
            &args.rm_tag,
            &args.add_alias,
            &args.set,
        )?;
        match args.format {
            OutputFormat::Plain => output::print_note_plain(&note, &vault.path),
            OutputFormat::Json => output::print_note_json(&note, &vault.path)?,
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
        let note = update_single_note(
            &vault,
            std::path::Path::new(&line),
            &args.add_tag,
            &args.rm_tag,
            &args.add_alias,
            &args.set,
        )?;
        notes.push(note);
    }

    match args.format {
        OutputFormat::Plain => output::print_note_many_plain(&notes, &vault.path),
        OutputFormat::Json => output::print_note_many_json(&notes, &vault.path)?,
    }
    Ok(())
}

fn update_single_note(
    vault: &Vault,
    note_arg: &std::path::Path,
    add_tags: &[String],
    rm_tags: &[String],
    add_aliases: &[String],
    set: &[String],
) -> eyre::Result<Note> {
    let (note_path, _) = vault.resolve_note_path(note_arg, true)?;
    let mut note = Note::from_path(&note_path)?;

    let mut dirty = false;

    // Add tags
    if !add_tags.is_empty() {
        for tag in add_tags {
            note.add_tag(tag.clone());
        }
        dirty = true;
    }

    // Remove tags
    if !rm_tags.is_empty() {
        for tag in rm_tags {
            note.remove_tag(tag);
        }
        dirty = true;
    }

    // Add aliases
    if !add_aliases.is_empty() {
        for alias in add_aliases {
            note.add_alias(alias.clone());
        }
        dirty = true;
    }

    // Set other fields
    if !set.is_empty() {
        for kv in set {
            let parts: Vec<&str> = kv.splitn(2, '=').collect();
            if parts.len() != 2 {
                eyre::bail!("invalid --set argument (expected key=value): {}", kv);
            }
            let key = parts[0].trim();
            let value = parts[1].trim();
            let value = serde_yaml::from_str(value).unwrap_or_else(|_| serde_yaml::Value::String(value.to_string()));
            note.set_field(key, &value)?;
        }
        dirty = true;
    }

    if dirty {
        note.write_frontmatter()?;
    }

    Ok(note)
}
