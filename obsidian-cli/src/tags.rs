use color_eyre::eyre;
use obsidian_core::Vault;

use crate::args::{OutputFormat, TagsListArgs, TagsSearchArgs};
use crate::output;

pub fn cmd_tags_search(vault: Vault, args: TagsSearchArgs) -> eyre::Result<()> {
    let mut results = vault.find_tags(&args.tags)?;
    if let Some(sort) = args.sort {
        obsidian_core::search::sort_notes_by(&mut results, |(n, _)| Some(n), &sort.into());
    }
    match args.format {
        OutputFormat::Plain => output::print_tags_search_plain(&results, vault.path()),
        OutputFormat::Json => output::print_tags_search_json(&results, vault.path()),
    }
    Ok(())
}

pub fn cmd_tags_list(vault: Vault, args: TagsListArgs) -> eyre::Result<()> {
    let tags = vault.list_tags()?;
    match args.format {
        OutputFormat::Plain => output::print_tags_list_plain(&tags),
        OutputFormat::Json => output::print_tags_list_json(&tags),
    }
    Ok(())
}
