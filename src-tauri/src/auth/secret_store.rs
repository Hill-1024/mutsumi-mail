#[cfg(any(target_os = "macos", test))]
use std::collections::HashMap;
#[cfg(any(target_os = "macos", test))]
use std::fmt;
#[cfg(any(target_os = "macos", test))]
use std::sync::Arc;
use std::sync::Mutex;

use thiserror::Error;

#[cfg(any(target_os = "macos", test))]
use keyring_core::{Entry, Error as KeyringError};

#[derive(Debug, Clone, Error)]
pub enum SecretStoreError {
    #[cfg(any(target_os = "macos", test))]
    #[error("platform credential store unavailable: {0}")]
    Unavailable(String),
    #[error("尚未保存本地授权码，请在设置中更新账户授权码")]
    NotFound,
    #[error("无法读写本地授权码，请检查应用目录是否可写")]
    OperationFailed,
}

pub trait SecretStore: Send + Sync {
    fn set(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError>;
    fn get(&self, reference: &str) -> Result<String, SecretStoreError>;
    fn delete(&self, reference: &str) -> Result<(), SecretStoreError>;
}

/// App-private local storage. SQLite provides atomic, crash-safe writes; no OS
/// credential API participates in normal reads, writes, or deletion.
pub struct LocalSecretStore {
    connection: Mutex<rusqlite::Connection>,
    #[cfg(target_os = "macos")]
    legacy: std::sync::OnceLock<PlatformSecretStore>,
}

impl LocalSecretStore {
    pub fn open(directory: &std::path::Path) -> Result<Self, SecretStoreError> {
        use std::fs::{self, OpenOptions};
        fs::create_dir_all(directory).map_err(|_| SecretStoreError::OperationFailed)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| SecretStoreError::OperationFailed)?;
        }
        let path = directory.join("credentials.sqlite3");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|_| SecretStoreError::OperationFailed)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| SecretStoreError::OperationFailed)?;
        }
        drop(file);
        let connection =
            rusqlite::Connection::open(path).map_err(|_| SecretStoreError::OperationFailed)?;
        connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; PRAGMA busy_timeout=5000; CREATE TABLE IF NOT EXISTS credentials (reference TEXT PRIMARY KEY NOT NULL, secret TEXT);")
            .map_err(|_| SecretStoreError::OperationFailed)?;
        Ok(Self {
            connection: Mutex::new(connection),
            #[cfg(target_os = "macos")]
            legacy: std::sync::OnceLock::new(),
        })
    }
}

impl SecretStore for LocalSecretStore {
    fn set(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError> {
        self.connection.lock().map_err(|_| SecretStoreError::OperationFailed)?
            .execute("INSERT INTO credentials(reference,secret) VALUES (?,?) ON CONFLICT(reference) DO UPDATE SET secret=excluded.secret", rusqlite::params![reference, secret])
            .map_err(|_| SecretStoreError::OperationFailed)?;
        Ok(())
    }
    fn get(&self, reference: &str) -> Result<String, SecretStoreError> {
        use rusqlite::OptionalExtension;
        let connection = self
            .connection
            .lock()
            .map_err(|_| SecretStoreError::OperationFailed)?;
        let stored: Option<Option<String>> = connection
            .query_row(
                "SELECT secret FROM credentials WHERE reference=?",
                [reference],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| SecretStoreError::OperationFailed)?;
        if let Some(secret) = stored {
            return secret.ok_or(SecretStoreError::NotFound);
        }
        // Legacy macOS reads are explicitly non-interactive. Other platforms never
        // initialize a keyring: recovery uses the account's authorization-code form.
        #[cfg(target_os = "macos")]
        if let Ok(secret) = self
            .legacy
            .get_or_init(PlatformSecretStore::new)
            .get(reference)
        {
            connection
                .execute(
                    "INSERT INTO credentials(reference,secret) VALUES (?,?)",
                    rusqlite::params![reference, secret],
                )
                .map_err(|_| SecretStoreError::OperationFailed)?;
            return Ok(secret);
        }
        Err(SecretStoreError::NotFound)
    }
    fn delete(&self, reference: &str) -> Result<(), SecretStoreError> {
        // A tombstone prevents an old Keychain value from being imported again.
        self.connection.lock().map_err(|_| SecretStoreError::OperationFailed)?
            .execute("INSERT INTO credentials(reference,secret) VALUES (?,NULL) ON CONFLICT(reference) DO UPDATE SET secret=NULL", [reference])
            .map_err(|_| SecretStoreError::OperationFailed)?;
        Ok(())
    }
}

#[cfg(any(target_os = "macos", test))]
pub struct PlatformSecretStore {
    service: Arc<str>,
    // Keep credentials read or written by this process in memory. Besides avoiding needless
    // Keychain round-trips, this prevents one sync phase after another from repeatedly asking
    // macOS to authorize access to the same item. The cache is never included in Debug output.
    cache: Mutex<HashMap<String, Result<String, SecretStoreError>>>,
    initialization_error: Option<String>,
}

#[cfg(any(target_os = "macos", test))]
impl fmt::Debug for PlatformSecretStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlatformSecretStore")
            .field("service", &self.service)
            .finish_non_exhaustive()
    }
}

#[cfg(any(target_os = "macos", test))]
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

#[cfg(any(target_os = "macos", test))]
impl Default for PlatformSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "macos", test))]
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
        let result = self
            .entry(reference)
            .and_then(|entry| match entry.get_password() {
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

#[cfg(any(target_os = "macos", test))]
fn initialize_platform_store() -> Result<(), String> {
    // This process must never bring up macOS Keychain authorization dialogs, including
    // during startup sync. Keep the guard alive for the entire process lifetime.
    #[cfg(target_os = "macos")]
    {
        use security_framework::os::macos::keychain::{KeychainUserInteractionLock, SecKeychain};
        static QUIET_KEYCHAIN: std::sync::OnceLock<Result<KeychainUserInteractionLock, String>> =
            std::sync::OnceLock::new();
        QUIET_KEYCHAIN
            .get_or_init(|| {
                SecKeychain::disable_user_interaction().map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
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

    #[test]
    fn local_credentials_survive_reopen_and_deletion_without_keyring_access() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("credentials");
        let store = super::LocalSecretStore::open(&directory).unwrap();
        store.set("account/local", "first-value").unwrap();
        assert_eq!(store.get("account/local").unwrap(), "first-value");
        drop(store);
        let store = super::LocalSecretStore::open(&directory).unwrap();
        assert_eq!(store.get("account/local").unwrap(), "first-value");
        store.set("account/local", "replacement").unwrap();
        assert_eq!(store.get("account/local").unwrap(), "replacement");
        store.delete("account/local").unwrap();
        assert!(matches!(
            store.get("account/local"),
            Err(super::SecretStoreError::NotFound)
        ));
        #[cfg(target_os = "macos")]
        assert!(
            store.legacy.get().is_none(),
            "local operations must never initialize the keyring"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(directory.join("credentials.sqlite3"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
