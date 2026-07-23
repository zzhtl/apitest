use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::StorageError;

const REDACTED: &[u8] = b"[REDACTED]";

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

    pub fn begin_redacted<I, S>(&self, secrets: I) -> Result<RedactingBodySink, StorageError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<[u8]>,
    {
        let mut secrets = secrets
            .into_iter()
            .map(|secret| secret.as_ref().to_vec())
            .filter(|secret| !secret.is_empty())
            .collect::<Vec<_>>();
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        let max_secret_len = secrets.iter().map(Vec::len).max().unwrap_or(1);
        Ok(RedactingBodySink {
            sink: Some(self.begin()?),
            secrets,
            max_secret_len,
            pending: Vec::new(),
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

pub struct RedactingBodySink {
    sink: Option<BodySink>,
    secrets: Vec<Vec<u8>>,
    max_secret_len: usize,
    pending: Vec<u8>,
}

impl RedactingBodySink {
    pub fn commit(mut self) -> Result<BodyRef, StorageError> {
        let path = self
            .sink
            .as_ref()
            .expect("redacting body sink should be open")
            .temporary_path()
            .to_path_buf();
        self.drain_pending(true)
            .map_err(|error| StorageError::io(&path, error))?;
        self.sink
            .take()
            .expect("redacting body sink should be open")
            .commit()
    }

    fn drain_pending(&mut self, final_chunk: bool) -> std::io::Result<()> {
        let safe_end = if final_chunk {
            self.pending.len()
        } else {
            self.pending
                .len()
                .saturating_sub(self.max_secret_len.saturating_sub(1))
        };
        let mut cursor = 0;
        let sink = self
            .sink
            .as_mut()
            .ok_or_else(|| std::io::Error::other("body sink is already committed"))?;
        while cursor < safe_end {
            if let Some(secret) = self
                .secrets
                .iter()
                .find(|secret| self.pending[cursor..].starts_with(secret))
            {
                sink.write_all(REDACTED)?;
                cursor += secret.len();
            } else {
                sink.write_all(&self.pending[cursor..cursor + 1])?;
                cursor += 1;
            }
        }
        self.pending.drain(..cursor);
        Ok(())
    }
}

impl Write for RedactingBodySink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buffer);
        self.drain_pending(false)?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.drain_pending(false)?;
        self.sink
            .as_mut()
            .ok_or_else(|| std::io::Error::other("body sink is already committed"))?
            .flush()
    }
}
