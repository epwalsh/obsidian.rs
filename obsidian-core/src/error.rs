use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
    #[error("note already exists at {0}")]
    NoteAlreadyExists(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Note(#[from] NoteError),
}

#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("failed to serialize frontmatter: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("invalid glob pattern: {0}")]
    InvalidGlob(#[from] globset::Error),
    #[error("invalid regex pattern: {0}")]
    InvalidRegex(#[from] regex::Error),
}
