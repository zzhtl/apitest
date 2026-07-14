use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
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
