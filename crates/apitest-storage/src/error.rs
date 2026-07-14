use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database failure: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("I/O failure at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("serialization failure: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("secret store failure: {0}")]
    Secret(String),
    #[error("storage lock was poisoned")]
    Poisoned,
    #[error("invalid secret reference `{0}`")]
    InvalidSecretReference(String),
}

impl StorageError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
