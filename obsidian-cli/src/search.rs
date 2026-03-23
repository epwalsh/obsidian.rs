use color_eyre::eyre;
use obsidian_core::{Note, Vault};

use crate::args::{OutputFormat, SearchArgs};
use crate::output;
use crate::utils::sort_notes_by;

pub fn cmd_search(vault: Vault, args: SearchArgs) -> eyre::Result<()> {
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
    for r in &args.regex {
        query = query.content_matches(r);
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
    if let Some(id) = &args.id {
        query = query.id(id);
    }

    let results = query.execute()?;
    let mut notes: Vec<Note> = results.into_iter().filter_map(|r| r.ok()).collect();
    sort_notes_by(&mut notes, |n| &n.path, &args.sort);

    match args.format {
        OutputFormat::Plain => output::print_note_many_plain(&notes, &vault.path),
        OutputFormat::Json => output::print_note_many_json(&notes, &vault.path)?,
    }
    Ok(())
}
