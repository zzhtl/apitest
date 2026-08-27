use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use apitest_core::SecretRef;

use crate::StorageError;

pub trait SecretStore: Send + Sync {
    fn get(&self, reference: &SecretRef) -> Result<Option<String>, StorageError>;
    fn set(&self, reference: &SecretRef, value: &str) -> Result<(), StorageError>;
    fn delete(&self, reference: &SecretRef) -> Result<(), StorageError>;
}

#[derive(Debug, Clone)]
pub struct SystemSecretStore {
    service: String,
}

impl SystemSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, reference: &SecretRef) -> Result<keyring::Entry, StorageError> {
        let account = reference
            .as_str()
            .strip_prefix("keyring://")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| StorageError::InvalidSecretReference(reference.0.clone()))?;
        keyring::Entry::new(&self.service, account)
            .map_err(|error| StorageError::Secret(error.to_string()))
    }
}

impl SecretStore for SystemSecretStore {
    fn get(&self, reference: &SecretRef) -> Result<Option<String>, StorageError> {
        match self.entry(reference)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(StorageError::Secret(error.to_string())),
        }
    }

    fn set(&self, reference: &SecretRef, value: &str) -> Result<(), StorageError> {
        self.entry(reference)?
            .set_password(value)
            .map_err(|error| StorageError::Secret(error.to_string()))
    }

    fn delete(&self, reference: &SecretRef) -> Result<(), StorageError> {
        match self.entry(reference)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(StorageError::Secret(error.to_string())),
        }
    }
}

/// Read-through cache over another [`SecretStore`].
///
/// The system store is an IPC round-trip per call (D-Bus Secret Service on
/// Linux, Keychain on macOS) and a send resolves every referenced secret, so
/// uncached lookups dominated request latency. Writes and deletes go straight
/// through and invalidate their entry.
///
/// Trade-off, stated on purpose: plaintext values sit in process memory for
/// up to the TTL — the same exposure window as the materialized variables the
/// executors already hold.
pub struct CachingSecretStore<S> {
    inner: S,
    ttl: Duration,
    values: Mutex<HashMap<SecretRef, (Instant, Option<String>)>>,
}

impl<S: SecretStore> CachingSecretStore<S> {
    pub fn new(inner: S, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            values: Mutex::new(HashMap::new()),
        }
    }
}

impl<S: SecretStore> SecretStore for CachingSecretStore<S> {
    fn get(&self, reference: &SecretRef) -> Result<Option<String>, StorageError> {
        {
            let values = self.values.lock().map_err(|_| StorageError::Poisoned)?;
            if let Some((cached_at, value)) = values.get(reference)
                && cached_at.elapsed() < self.ttl
            {
                return Ok(value.clone());
            }
        }
        let value = self.inner.get(reference)?;
        let mut values = self.values.lock().map_err(|_| StorageError::Poisoned)?;
        values.insert(reference.clone(), (Instant::now(), value.clone()));
        Ok(value)
    }

    fn set(&self, reference: &SecretRef, value: &str) -> Result<(), StorageError> {
        self.inner.set(reference, value)?;
        let mut values = self.values.lock().map_err(|_| StorageError::Poisoned)?;
        values.insert(reference.clone(), (Instant::now(), Some(value.to_owned())));
        Ok(())
    }

    fn delete(&self, reference: &SecretRef) -> Result<(), StorageError> {
        self.inner.delete(reference)?;
        let mut values = self.values.lock().map_err(|_| StorageError::Poisoned)?;
        values.remove(reference);
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemorySecretStore {
    values: Arc<Mutex<HashMap<SecretRef, String>>>,
}

impl SecretStore for MemorySecretStore {
    fn get(&self, reference: &SecretRef) -> Result<Option<String>, StorageError> {
        let values = self.values.lock().map_err(|_| StorageError::Poisoned)?;
        Ok(values.get(reference).cloned())
    }

    fn set(&self, reference: &SecretRef, value: &str) -> Result<(), StorageError> {
        let mut values = self.values.lock().map_err(|_| StorageError::Poisoned)?;
        values.insert(reference.clone(), value.to_owned());
        Ok(())
    }

    fn delete(&self, reference: &SecretRef) -> Result<(), StorageError> {
        let mut values = self.values.lock().map_err(|_| StorageError::Poisoned)?;
        values.remove(reference);
        Ok(())
    }
}
