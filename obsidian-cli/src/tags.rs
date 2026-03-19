use color_eyre::eyre;
use obsidian_core::{LocatedTag, Note, Vault};

use crate::args::{OutputFormat, TagsListArgs, TagsSearchArgs};
use crate::output;
use crate::utils::sort_notes_by;

pub fn cmd_tags_search(vault: Vault, args: TagsSearchArgs) -> eyre::Result<()> {
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
    sort_notes_by(&mut results, |(n, _, _)| &n.path, &args.sort);

    match args.format {
        OutputFormat::Plain => output::print_tags_search_plain(&results, &vault.path),
        OutputFormat::Json => output::print_tags_search_json(&results, &vault.path),
    }
    Ok(())
}

pub fn cmd_tags_list(vault: Vault, args: TagsListArgs) -> eyre::Result<()> {
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
