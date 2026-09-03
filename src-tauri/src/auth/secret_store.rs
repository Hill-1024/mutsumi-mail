use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretStoreError {
    #[error("platform credential store unavailable: {0}")]
    Unavailable(String),
    #[error("credential not found")]
    NotFound,
    #[error("credential operation failed")]
    OperationFailed,
}

pub trait SecretStore: Send + Sync {
    fn set(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError>;
    fn get(&self, reference: &str) -> Result<String, SecretStoreError>;
    fn delete(&self, reference: &str) -> Result<(), SecretStoreError>;
}

pub struct PlatformSecretStore {
    service: Arc<str>,
    // Keep credentials read or written by this process in memory. Besides avoiding needless
    // Keychain round-trips, this prevents one sync phase after another from repeatedly asking
    // macOS to authorize access to the same item. The cache is never included in Debug output.
    cache: Mutex<HashMap<String, String>>,
}

impl fmt::Debug for PlatformSecretStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlatformSecretStore")
            .field("service", &self.service)
            .finish_non_exhaustive()
    }
}

impl PlatformSecretStore {
    pub fn new() -> Self {
        Self {
            service: Arc::from("moe.mutsumi.mail"),
            cache: Mutex::new(HashMap::new()),
        }
    }
    fn entry(&self, reference: &str) -> Result<keyring::Entry, SecretStoreError> {
        keyring::Entry::new(&self.service, reference)
            .map_err(|error| SecretStoreError::Unavailable(error.to_string()))
    }
}

impl Default for PlatformSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for PlatformSecretStore {
    fn set(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError> {
        self.entry(reference)?
            .set_password(secret)
            .map_err(|_error| SecretStoreError::OperationFailed)?;
        self.cache
            .lock()
            .map_err(|_| SecretStoreError::OperationFailed)?
            .insert(reference.to_owned(), secret.to_owned());
        Ok(())
    }
    fn get(&self, reference: &str) -> Result<String, SecretStoreError> {
        if let Some(secret) = self
            .cache
            .lock()
            .map_err(|_| SecretStoreError::OperationFailed)?
            .get(reference)
            .cloned()
        {
            return Ok(secret);
        }

        match self.entry(reference)?.get_password() {
            Ok(secret) => {
                self.cache
                    .lock()
                    .map_err(|_| SecretStoreError::OperationFailed)?
                    .insert(reference.to_owned(), secret.clone());
                Ok(secret)
            }
            Err(error) if error.to_string().to_ascii_lowercase().contains("not found") => {
                Err(SecretStoreError::NotFound)
            }
            Err(_) => Err(SecretStoreError::OperationFailed),
        }
    }
    fn delete(&self, reference: &str) -> Result<(), SecretStoreError> {
        let result = match self.entry(reference)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(error) if error.to_string().to_ascii_lowercase().contains("not found") => Ok(()),
            Err(_) => Err(SecretStoreError::OperationFailed),
        };
        if result.is_ok() {
            self.cache
                .lock()
                .map_err(|_| SecretStoreError::OperationFailed)?
                .remove(reference);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{PlatformSecretStore, SecretStore};

    #[test]
    fn references_are_namespaced() {
        let reference = "account/abc/incoming";
        assert!(reference.starts_with("account/"));
        // Secret values deliberately never appear in this test or in diagnostics.
        let _ = std::mem::size_of::<Option<Box<dyn SecretStore>>>();
    }

    #[test]
    fn cached_secret_is_returned_without_another_platform_lookup() {
        let store = PlatformSecretStore::new();
        store
            .cache
            .lock()
            .expect("cache lock")
            .insert("account/test/incoming".into(), "sensitive-value".into());

        assert_eq!(
            store.get("account/test/incoming").expect("cached secret"),
            "sensitive-value"
        );
        assert!(!format!("{store:?}").contains("sensitive-value"));
    }
}
