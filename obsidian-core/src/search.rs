use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::Note;

/// Returns an iterator over all `.md` file paths found recursively under `root`.
pub fn find_note_paths(root: impl AsRef<Path>) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().and_then(|e| e.to_str()) == Some("md")
        })
        .map(|entry| entry.into_path())
}

/// Loads all notes found recursively under `root` in parallel.
pub fn find_notes(root: impl AsRef<Path>) -> Vec<Result<Note, std::io::Error>> {
    find_note_paths(root)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(Note::from_path)
        .collect()
}
