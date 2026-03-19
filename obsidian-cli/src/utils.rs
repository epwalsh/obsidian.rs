use std::env::current_dir;

use color_eyre::eyre;
use obsidian_core::Vault;

use crate::args::SortOrder;

pub fn modified_time(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}

pub fn sort_notes_by<T>(items: &mut [T], key: impl Fn(&T) -> &std::path::Path, sort: &SortOrder) {
    match sort {
        SortOrder::PathAsc => items.sort_by(|a, b| key(a).cmp(key(b))),
        SortOrder::PathDesc => items.sort_by(|a, b| key(b).cmp(key(a))),
        SortOrder::ModifiedAsc => items.sort_by_key(|a| modified_time(key(a))),
        SortOrder::ModifiedDesc => items.sort_by_key(|b| std::cmp::Reverse(modified_time(key(b)))),
    }
}

pub fn resolve_note_path(
    vault: &Vault,
    note_arg: &std::path::PathBuf,
) -> eyre::Result<(std::path::PathBuf, std::path::PathBuf)> {
    let (note_path, root) = if note_arg.is_absolute() {
        (note_arg.clone(), vault.path.clone())
    } else {
        let cwd = current_dir()?;
        let candidate1 = cwd.join(note_arg);
        let candidate2 = vault.path.join(note_arg);
        if candidate1.exists() {
            (candidate1, cwd)
        } else if candidate2.exists() {
            (candidate2, vault.path.clone())
        } else {
            (note_arg.clone(), vault.path.clone())
        }
    };

    if !note_path.exists() {
        return Err(eyre::eyre!("note not found: {}", note_path.display()));
    }

    Ok((note_path, root))
}
