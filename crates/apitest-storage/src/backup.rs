use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::Utc;
use uuid::Uuid;

use crate::{Database, StorageError};

#[derive(Debug, Clone)]
pub struct BackupManager {
    directory: PathBuf,
    retention: usize,
}

impl BackupManager {
    pub fn new(directory: impl AsRef<Path>, retention: usize) -> Result<Self, StorageError> {
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir_all(&directory).map_err(|error| StorageError::io(&directory, error))?;
        Ok(Self {
            directory,
            retention: retention.max(1),
        })
    }

    pub fn snapshot(&self, database: &Database) -> Result<PathBuf, StorageError> {
        let name = format!(
            "apitest-{}-{}.db",
            Utc::now().format("%Y%m%d-%H%M%S"),
            Uuid::now_v7()
        );
        let destination = self.directory.join(name);
        database.create_backup(&destination)?;
        self.prune()?;
        Ok(destination)
    }

    /// Snapshot by path with its own pair of connections, so the caller's
    /// live connection — and every reader waiting on its mutex — stays free
    /// while SQLite copies pages (WAL permits the concurrent reader).
    pub fn snapshot_file(&self, source: impl AsRef<Path>) -> Result<PathBuf, StorageError> {
        let name = format!(
            "apitest-{}-{}.db",
            Utc::now().format("%Y%m%d-%H%M%S"),
            Uuid::now_v7()
        );
        let destination = self.directory.join(name);
        Database::backup_file(source, &destination)?;
        self.prune()?;
        Ok(destination)
    }

    pub fn list(&self) -> Result<Vec<PathBuf>, StorageError> {
        let mut backups = fs::read_dir(&self.directory)
            .map_err(|error| StorageError::io(&self.directory, error))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "db"))
            .collect::<Vec<_>>();
        backups.sort();
        backups.reverse();
        Ok(backups)
    }

    fn prune(&self) -> Result<(), StorageError> {
        for stale in self.list()?.into_iter().skip(self.retention) {
            fs::remove_file(&stale).map_err(|error| StorageError::io(&stale, error))?;
        }
        Ok(())
    }
}
