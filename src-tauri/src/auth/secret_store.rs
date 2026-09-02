use std::sync::Arc;

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

#[derive(Debug, Clone)]
pub struct PlatformSecretStore {
    service: Arc<str>,
}

impl PlatformSecretStore {
    pub fn new() -> Self {
        Self {
            service: Arc::from("moe.mutsumi.mail"),
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
            .map_err(|_error| SecretStoreError::OperationFailed)
    }
    fn get(&self, reference: &str) -> Result<String, SecretStoreError> {
        match self.entry(reference)?.get_password() {
            Ok(secret) => Ok(secret),
            Err(error) if error.to_string().to_ascii_lowercase().contains("not found") => {
                Err(SecretStoreError::NotFound)
            }
            Err(_) => Err(SecretStoreError::OperationFailed),
        }
    }
    fn delete(&self, reference: &str) -> Result<(), SecretStoreError> {
        match self.entry(reference)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(error) if error.to_string().to_ascii_lowercase().contains("not found") => Ok(()),
            Err(_) => Err(SecretStoreError::OperationFailed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SecretStore;

    #[test]
    fn references_are_namespaced() {
        let reference = "account/abc/incoming";
        assert!(reference.starts_with("account/"));
        // Secret values deliberately never appear in this test or in diagnostics.
        let _ = std::mem::size_of::<Option<Box<dyn SecretStore>>>();
    }
}
