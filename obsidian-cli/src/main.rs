mod args;
mod output;

use std::env::current_dir;

use clap::Parser;
use color_eyre::eyre;
use obsidian_core::{Note, Vault};

use args::{BacklinksArgs, Cli, Command, OutputFormat, SearchArgs};

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
    if let Some(t) = &args.title {
        query = query.title_contains(t);
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

fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let vault = Vault::open(&cli.vault)?;
    match cli.command {
        Command::Search(args) => cmd_search(vault, args),
        Command::Backlinks(args) => cmd_backlinks(vault, args),
    }
}
