use std::path::{Path, PathBuf};

use thiserror::Error;
use tower_lsp::lsp_types::Url;

#[derive(Debug, Error)]
pub enum UriError {
    #[error("URI '{0}' is not a file URI")]
    NotAFileUri(Url),
    #[error("path '{0}' could not be converted to a file URI")]
    InvalidPath(PathBuf),
    #[error("path '{path}' is outside vault root '{vault_root}'")]
    PathOutsideVault { path: PathBuf, vault_root: PathBuf },
}

pub fn uri_to_path(uri: &Url) -> Result<PathBuf, UriError> {
    uri.to_file_path()
        .map(normalize_path)
        .map_err(|()| UriError::NotAFileUri(uri.clone()))
}

pub fn path_to_uri(path: &Path) -> Result<Url, UriError> {
    let normalized = normalize_path(path);
    Url::from_file_path(&normalized).map_err(|()| UriError::InvalidPath(normalized))
}

pub fn vault_relative_path(vault_root: &Path, path: &Path) -> Result<PathBuf, UriError> {
    path.strip_prefix(vault_root)
        .map(Path::to_path_buf)
        .map_err(|_| UriError::PathOutsideVault {
            path: path.to_path_buf(),
            vault_root: vault_root.to_path_buf(),
        })
}

fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    obsidian_core::common::normalize_path(path, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn path_to_uri_round_trips_file_paths() {
        let vault_dir = tempfile::tempdir().unwrap();
        let note_path = vault_dir.path().join("Daily Note.md");
        fs::write(&note_path, "body").unwrap();
        let note_path = note_path.canonicalize().unwrap();

        let uri = path_to_uri(&note_path).unwrap();
        let round_trip = uri_to_path(&uri).unwrap();

        assert_eq!(round_trip, note_path);
    }

    #[test]
    fn uri_to_path_rejects_non_file_uris() {
        let uri = Url::parse("untitled:Daily%20Note").unwrap();

        let error = uri_to_path(&uri).unwrap_err();

        assert!(matches!(error, UriError::NotAFileUri(actual) if actual == uri));
    }

    #[test]
    fn vault_relative_path_returns_vault_relative_note_path() {
        let vault_root = PathBuf::from("/vault");
        let note_path = vault_root.join("notes/daily/today.md");

        let relative = vault_relative_path(&vault_root, &note_path).unwrap();

        assert_eq!(relative, PathBuf::from("notes/daily/today.md"));
    }

    #[test]
    fn vault_relative_path_errors_for_paths_outside_the_vault() {
        let vault_root = PathBuf::from("/vault");
        let note_path = PathBuf::from("/other-vault/today.md");

        let error = vault_relative_path(&vault_root, &note_path).unwrap_err();

        assert!(matches!(
            error,
            UriError::PathOutsideVault { path, vault_root: root }
            if path == note_path && root == Path::new("/vault")
        ));
    }
}
