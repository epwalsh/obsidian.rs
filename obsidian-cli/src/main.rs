mod args;
mod check;
mod note;
mod output;
mod search;
mod tags;

use clap::Parser;
use color_eyre::eyre;
use obsidian_core::Vault;

use args::{Cli, Command};

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

    let vault = match cli.vault {
        Some(ref path) => Vault::open(path)?,
        None => Vault::open_from_cwd()?,
    };

    match cli.command {
        Command::Search(args) => search::cmd_search(vault, *args),
        Command::Note(note_args) => match note_args.subcommand {
            args::NoteCommand::Resolve(args) => note::cmd_resolve(vault, args),
            args::NoteCommand::List(args) => note::cmd_list(vault, args),
            args::NoteCommand::Search(args) => search::cmd_search(vault, *args),
            args::NoteCommand::Read(args) => note::cmd_read(vault, args),
            args::NoteCommand::Write(args) => note::cmd_write(vault, args),
            args::NoteCommand::Backlinks(args) => note::cmd_backlinks(vault, args),
            args::NoteCommand::Merge(args) => note::cmd_merge(vault, args),
            args::NoteCommand::Patch(args) => note::cmd_patch(vault, args),
            args::NoteCommand::Rename(args) => note::cmd_rename(vault, args),
            args::NoteCommand::Update(args) => note::cmd_update(vault, args),
        },
        Command::Tags(tags_args) => match tags_args.subcommand {
            args::TagsCommand::Search(args) => tags::cmd_tags_search(vault, args),
            args::TagsCommand::List(args) => tags::cmd_tags_list(vault, args),
        },
        Command::Check(args) => check::cmd_check(vault, args),
    }
}
