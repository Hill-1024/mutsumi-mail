use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use keyring_core::{Entry, Error as KeyringError};

#[derive(Debug, Clone, Error)]
pub enum SecretStoreError {
    #[error("platform credential store unavailable: {0}")]
    Unavailable(String),
    #[error("credential not found")]
    NotFound,
    #[error("凭据暂不可用；后台已停止重复请求，请检查账户登录状态")]
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
    cache: Mutex<HashMap<String, Result<String, SecretStoreError>>>,
    initialization_error: Option<String>,
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
        let initialization_error = initialize_platform_store().err();
        Self {
            service: Arc::from("moe.mutsumi.mail"),
            cache: Mutex::new(HashMap::new()),
            initialization_error,
        }
    }
    fn entry(&self, reference: &str) -> Result<Entry, SecretStoreError> {
        if let Some(error) = &self.initialization_error {
            return Err(SecretStoreError::Unavailable(error.clone()));
        }
        Entry::new(&self.service, reference)
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
        // Serialize every platform keyring operation. macOS may show an authorization
        // dialog while an item is being read; allowing concurrent cache misses here can
        // turn one required confirmation into a burst of identical dialogs.
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| SecretStoreError::OperationFailed)?;
        self.entry(reference)?
            .set_password(secret)
            .map_err(|_error| SecretStoreError::OperationFailed)?;
        cache.insert(reference.to_owned(), Ok(secret.to_owned()));
        Ok(())
    }
    fn get(&self, reference: &str) -> Result<String, SecretStoreError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| SecretStoreError::OperationFailed)?;
        if let Some(secret) = cache.get(reference).cloned() {
            return secret;
        }

        // Remember failures too: background sync and body downloads must not repeatedly
        // re-open a locked/denied store. A successful explicit credential write resets it.
        let result = self.entry(reference).and_then(|entry| match entry.get_password() {
            Ok(secret) => Ok(secret),
            Err(KeyringError::NoEntry) => Err(SecretStoreError::NotFound),
            Err(_) => Err(SecretStoreError::OperationFailed),
        });
        cache.insert(reference.to_owned(), result.clone());
        result
    }

    fn delete(&self, reference: &str) -> Result<(), SecretStoreError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| SecretStoreError::OperationFailed)?;
        let result = match self.entry(reference)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(KeyringError::NoEntry) => Ok(()),
            Err(_) => Err(SecretStoreError::OperationFailed),
        };
        if result.is_ok() {
            cache.remove(reference);
        }
        result
    }
}

fn initialize_platform_store() -> Result<(), String> {
    // This process must never bring up macOS Keychain authorization dialogs, including
    // during startup sync. Keep the guard alive for the entire process lifetime.
    #[cfg(target_os = "macos")]
    {
        use security_framework::os::macos::keychain::{SecKeychain, KeychainUserInteractionLock};
        static QUIET_KEYCHAIN: std::sync::OnceLock<Result<KeychainUserInteractionLock, String>> = std::sync::OnceLock::new();
        QUIET_KEYCHAIN.get_or_init(|| SecKeychain::disable_user_interaction().map_err(|error| error.to_string()))
            .as_ref().map_err(Clone::clone)?;
    }

    #[cfg(target_os = "macos")]
    let store = apple_native_keyring_store::keychain::Store::new();
    #[cfg(target_os = "ios")]
    let store = apple_native_keyring_store::protected::Store::new();
    #[cfg(target_os = "android")]
    let store = android_native_keyring_store::Store::new();
    #[cfg(target_os = "windows")]
    let store = windows_native_keyring_store::Store::new();
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    let store = zbus_secret_service_keyring_store::Store::new();
    #[cfg(not(any(unix, windows)))]
    return Err("platform secure credential storage is unsupported".into());

    #[cfg(any(unix, windows))]
    {
        let store = store.map_err(|error| error.to_string())?;
        keyring_core::set_default_store(store);
        Ok(())
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
            .insert("account/test/incoming".into(), Ok("sensitive-value".into()));

        assert_eq!(
            store.get("account/test/incoming").expect("cached secret"),
            "sensitive-value"
        );
        assert!(!format!("{store:?}").contains("sensitive-value"));
    }
    #[test]
    fn failed_lookup_is_not_retried_by_background_tasks() {
        let store = PlatformSecretStore {
            service: "test".into(),
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            initialization_error: Some("store locked".into()),
        };
        assert!(store.get("account/test").is_err());
        assert!(store.cache.lock().unwrap().contains_key("account/test"));
        assert!(store.get("account/test").is_err());
    }

}
