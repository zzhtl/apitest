use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::StorageError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyRef {
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct BodyStore {
    root: PathBuf,
    temporary: PathBuf,
}

impl BodyStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let root = root.as_ref().to_path_buf();
        let temporary = root.join(".tmp");
        fs::create_dir_all(&temporary).map_err(|error| StorageError::io(&temporary, error))?;
        Ok(Self { root, temporary })
    }

    pub fn begin(&self) -> Result<BodySink, StorageError> {
        let id = Uuid::now_v7().to_string();
        let temporary_path = self.temporary.join(format!("{id}.part"));
        let final_path = self.root.join(format!("{id}.body"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|error| StorageError::io(&temporary_path, error))?;

        Ok(BodySink {
            file: Some(file),
            temporary_path,
            final_path,
            size: 0,
            committed: false,
        })
    }

    pub fn read_all(&self, body: &BodyRef) -> Result<Vec<u8>, StorageError> {
        fs::read(&body.path).map_err(|error| StorageError::io(&body.path, error))
    }

    pub fn read_range(
        &self,
        body: &BodyRef,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, StorageError> {
        let mut file =
            File::open(&body.path).map_err(|error| StorageError::io(&body.path, error))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| StorageError::io(&body.path, error))?;
        let available = body.size.saturating_sub(offset).min(length as u64) as usize;
        let mut bytes = vec![0; available];
        file.read_exact(&mut bytes)
            .map_err(|error| StorageError::io(&body.path, error))?;
        Ok(bytes)
    }

    pub fn delete(&self, body: &BodyRef) -> Result<(), StorageError> {
        match fs::remove_file(&body.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::io(&body.path, error)),
        }
    }
}

pub struct BodySink {
    file: Option<File>,
    temporary_path: PathBuf,
    final_path: PathBuf,
    size: u64,
    committed: bool,
}

impl BodySink {
    pub fn temporary_path(&self) -> &Path {
        &self.temporary_path
    }

    pub fn commit(mut self) -> Result<BodyRef, StorageError> {
        if let Some(mut file) = self.file.take() {
            file.flush()
                .map_err(|error| StorageError::io(&self.temporary_path, error))?;
            file.sync_data()
                .map_err(|error| StorageError::io(&self.temporary_path, error))?;
        }
        fs::rename(&self.temporary_path, &self.final_path)
            .map_err(|error| StorageError::io(&self.temporary_path, error))?;
        self.committed = true;
        Ok(BodyRef {
            path: self.final_path.clone(),
            size: self.size,
        })
    }
}

impl Write for BodySink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("body sink is already committed"))?
            .write(buffer)?;
        self.size = self.size.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("body sink is already committed"))?
            .flush()
    }
}

impl Drop for BodySink {
    fn drop(&mut self) {
        self.file.take();
        if !self.committed {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}
