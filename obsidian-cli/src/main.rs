mod args;
mod output;

use std::env::current_dir;

use clap::Parser;
use color_eyre::eyre;
use colored::Colorize;
use obsidian_core::{LocatedTag, Note, Vault};

use args::{BacklinksArgs, Cli, Command, OutputFormat, RenameArgs, SearchArgs, TagsListArgs, TagsSearchArgs};

fn cmd_search(vault: Vault, args: SearchArgs) -> eyre::Result<()> {
    let mut query = vault.search();
    for tag in &args.tag {
        query = query.has_tag(tag);
    }
    for glob in &args.glob {
        query = query.glob(glob);
    }
    for s in &args.content {
        query = query.content_contains(s);
    }
    for t in &args.title_contains {
        query = query.title_contains(t);
    }
    for alias in &args.alias {
        query = query.has_alias(alias);
    }
    for s in &args.alias_contains {
        query = query.alias_contains(s);
    }
    if let Some(r) = &args.regex {
        query = query.content_matches(r);
    }

    let results = query.execute()?;
    let mut notes: Vec<Note> = results.into_iter().filter_map(|r| r.ok()).collect();
    notes.sort_by(|a, b| a.path.cmp(&b.path));

    match args.format {
        OutputFormat::Plain => output::print_search_plain(&notes, &vault.path),
        OutputFormat::Json => output::print_search_json(&notes, &vault.path),
    }
    Ok(())
}

fn cmd_backlinks(vault: Vault, args: BacklinksArgs) -> eyre::Result<()> {
    let note_path = if args.note.is_absolute() {
        args.note.clone()
    } else {
        current_dir()?.join(&args.note)
    };
    if !note_path.exists() {
        return Err(eyre::eyre!("note not found: {}", note_path.display()));
    }

    let note = Note::from_path(&note_path)?;
    let mut results = vault.backlinks(&note);
    results.sort_by(|(a, _), (b, _)| a.path.cmp(&b.path));

    match args.format {
        OutputFormat::Plain => output::print_backlinks_plain(&results, &vault.path),
        OutputFormat::Json => output::print_backlinks_json(&results, &vault.path),
    }
    Ok(())
}

fn cmd_rename(vault: Vault, args: RenameArgs) -> eyre::Result<()> {
    let note_path = if args.note.is_absolute() {
        args.note.clone()
    } else {
        current_dir()?.join(&args.note)
    };
    if !note_path.exists() {
        return Err(eyre::eyre!("note not found: {}", note_path.display()));
    }

    let mut new_path = if args.new_path.is_absolute() {
        args.new_path.clone()
    } else {
        current_dir()?.join(&args.new_path)
    };
    if new_path.extension().and_then(|e| e.to_str()) != Some("md") {
        new_path.set_extension("md");
    }

    let note = Note::from_path(&note_path)?;

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

fn cmd_tags_search(vault: Vault, args: TagsSearchArgs) -> eyre::Result<()> {
    let notes: Vec<Note> = vault.search().execute()?.into_iter().filter_map(|r| r.ok()).collect();

    // A note tag matches a search term if it equals the term exactly or is a sub-tag of it
    // (e.g. "workout/upper-body" matches search term "workout").
    let tag_matches_search = |tag: &str| args.tags.iter().any(|s| tag == s || tag.starts_with(&format!("{s}/")));

    let mut results: Vec<(Note, Vec<String>, Vec<LocatedTag>)> = notes
        .into_iter()
        .filter_map(|note| {
            let fm_matches: Vec<String> = note.tags.iter().filter(|t| tag_matches_search(t)).cloned().collect();
            let inline_matches: Vec<LocatedTag> = note
                .inline_tags()
                .into_iter()
                .filter(|lt| tag_matches_search(&lt.tag))
                .collect();
            if fm_matches.is_empty() && inline_matches.is_empty() {
                None
            } else {
                Some((note, fm_matches, inline_matches))
            }
        })
        .collect();
    results.sort_by(|(a, _, _), (b, _, _)| a.path.cmp(&b.path));

    match args.format {
        OutputFormat::Plain => output::print_tags_search_plain(&results, &vault.path),
        OutputFormat::Json => output::print_tags_search_json(&results, &vault.path),
    }
    Ok(())
}

fn cmd_tags_list(vault: Vault, args: TagsListArgs) -> eyre::Result<()> {
    let notes: Vec<Note> = vault.search().execute()?.into_iter().filter_map(|r| r.ok()).collect();

    let mut all_tags: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for note in &notes {
        for tag in &note.tags {
            all_tags.insert(tag.clone());
        }
        for lt in note.inline_tags() {
            all_tags.insert(lt.tag);
        }
    }
    let tags: Vec<String> = all_tags.into_iter().collect();

    match args.format {
        OutputFormat::Plain => output::print_tags_list_plain(&tags),
        OutputFormat::Json => output::print_tags_list_json(&tags),
    }
    Ok(())
}

fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    if cli.color && cli.no_color {
        eyre::bail!("--color and --no-color are mutually exclusive");
    } else if cli.color {
        colored::control::set_override(true);
    } else if cli.no_color {
        colored::control::set_override(false);
    }
    let vault = Vault::open(&cli.vault)?;
    match cli.command {
        Command::Search(args) => cmd_search(vault, args),
        Command::Backlinks(args) => cmd_backlinks(vault, args),
        Command::Rename(args) => cmd_rename(vault, args),
        Command::Tags(tags_args) => match tags_args.subcommand {
            args::TagsCommand::Search(args) => cmd_tags_search(vault, args),
            args::TagsCommand::List(args) => cmd_tags_list(vault, args),
        },
    }
}
