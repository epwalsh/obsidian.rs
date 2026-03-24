use color_eyre::eyre;
use obsidian_core::{Note, Vault};

use crate::args::{OutputFormat, SearchArgs};
use crate::output;
use crate::utils::sort_notes_by;

pub fn cmd_search(vault: Vault, args: SearchArgs) -> eyre::Result<()> {
    let mut query = vault.search();
    for glob in &args.and_glob {
        query = query.and_glob(glob);
    }
    for glob in &args.or_glob {
        query = query.or_glob(glob);
    }
    if let Some(id) = &args.and_id {
        query = query.and_has_id(id);
    }
    for id in &args.or_id {
        query = query.or_has_id(id);
    }
    for tag in &args.and_tag {
        query = query.and_has_tag(tag);
    }
    for tag in &args.or_tag {
        query = query.or_has_tag(tag);
    }
    for s in &args.and_content_contains {
        query = query.and_content_contains(s);
    }
    for s in &args.or_content_contains {
        query = query.or_content_contains(s);
    }
    for r in &args.and_content_matches {
        query = query.and_content_matches(r);
    }
    for r in &args.or_content_matches {
        query = query.or_content_matches(r);
    }
    for t in &args.and_title_contains {
        query = query.and_title_contains(t);
    }
    for t in &args.or_title_contains {
        query = query.or_title_contains(t);
    }
    for alias in &args.and_alias {
        query = query.and_has_alias(alias);
    }
    for alias in &args.or_alias {
        query = query.or_has_alias(alias);
    }
    for s in &args.and_alias_contains {
        query = query.and_alias_contains(s);
    }
    for s in &args.or_alias_contains {
        query = query.or_alias_contains(s);
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
