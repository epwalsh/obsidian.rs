use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("'{0}' is not a directory")]
    NotADirectory(PathBuf),
    #[error("note already exists at '{0}'")]
    NoteAlreadyExists(PathBuf),
    #[error("note not found: '{0}'")]
    NoteNotFound(String),
    #[error("ambigous note identifier '{0}'; multiple matches found")]
    AmbiguousNoteIdentifier(String, Vec<PathBuf>),
    #[error("directory not found: {0}")]
    DirectoryNotFound(PathBuf),
    #[error("source note is the same as destination: '{0}'")]
    MergeSourceIsDestination(PathBuf),
    #[error("old-string not found in '{0}'")]
    StringNotFound(PathBuf),
    #[error("old-string found multiple times in '{0}'; must match exactly once")]
    StringFoundMultipleTimes(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Note(#[from] NoteError),
    #[error(transparent)]
    Search(#[from] SearchError),
}

#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("failed to serialize frontmatter: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("failed to serialize frontmatter: {0}")]
    Json(String),
    #[error("note content not loaded; use from_path_with_content() or load_content()")]
    ContentNotLoaded,
    #[error("'{0}' is not a valid note path")]
    InvalidPath(PathBuf),
    #[error("{0}")]
    InvalidFieldName(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("invalid glob pattern: '{0}'")]
    InvalidGlob(#[from] globset::Error),
    #[error("invalid regex pattern: '{0}'")]
    InvalidRegex(#[from] regex::Error),
}
