use std::path::PathBuf;

use obsidian_core::SearchError;

pub enum CliError {
    Io(std::io::Error),
    Search(SearchError),
    NoteNotFound(PathBuf),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Io(e) => write!(f, "{e}"),
            CliError::Search(e) => write!(f, "{e}"),
            CliError::NoteNotFound(path) => write!(f, "note not found: {}", path.display()),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Io(e)
    }
}

impl From<SearchError> for CliError {
    fn from(e: SearchError) -> Self {
        CliError::Search(e)
    }
}
