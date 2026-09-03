use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::auth::secret_store::{PlatformSecretStore, SecretStore};
use crate::domain::SyncStatus;
use crate::errors::AppError;
use crate::storage::database::Database;

pub struct SyncCoordinator {
    tokens: Mutex<std::collections::HashMap<String, CancellationToken>>,
    statuses: Mutex<std::collections::HashMap<String, SyncStatus>>,
}

impl SyncCoordinator {
    pub fn new() -> Self {
        Self {
            tokens: Mutex::new(std::collections::HashMap::new()),
            statuses: Mutex::new(std::collections::HashMap::new()),
        }
    }
    pub fn start(&self, account_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        if let Ok(mut tokens) = self.tokens.lock() {
            if let Some(previous) = tokens.insert(account_id.to_owned(), token.clone()) {
                previous.cancel();
            }
        }
        token
    }
    pub fn try_start(&self, account_id: &str) -> Option<CancellationToken> {
        let token = CancellationToken::new();
        let mut tokens = self.tokens.lock().ok()?;
        if tokens.contains_key(account_id) {
            return None;
        }
        tokens.insert(account_id.to_owned(), token.clone());
        Some(token)
    }
    pub fn cancel(&self, account_id: &str) {
        if let Ok(mut tokens) = self.tokens.lock() {
            if let Some(token) = tokens.remove(account_id) {
                token.cancel();
            }
        }
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.insert(
                account_id.to_owned(),
                SyncStatus {
                    account_id: account_id.to_owned(),
                    state: "idle".into(),
                    phase: None,
                    processed: None,
                    total: None,
                    message: Some("同步已取消".into()),
                    retryable: false,
                },
            );
        }
    }

    pub fn set_status(&self, status: SyncStatus) {
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.insert(status.account_id.clone(), status);
        }
    }

    pub fn status(&self, account_id: &str) -> Option<SyncStatus> {
        self.statuses
            .lock()
            .ok()
            .and_then(|statuses| statuses.get(account_id).cloned())
    }

    /// Runs a side effect only while `token` still owns this account's active sync slot.
    pub fn with_current<R>(
        &self,
        account_id: &str,
        token: &CancellationToken,
        action: impl FnOnce() -> R,
    ) -> Option<R> {
        let tokens = self.tokens.lock().ok()?;
        if tokens.get(account_id) != Some(token) {
            return None;
        }
        Some(action())
    }

    /// Runs a finalizer only when `token` still owns this account's sync slot.
    /// Keeping the slot locked through the finalizer prevents an older, cancelled task from
    /// overwriting the persistent status of a newer sync for the same account.
    pub fn finish_current<R>(
        &self,
        account_id: &str,
        token: &CancellationToken,
        finalizer: impl FnOnce() -> R,
    ) -> Option<R> {
        let mut tokens = self.tokens.lock().ok()?;
        if tokens.get(account_id) != Some(token) {
            return None;
        }
        let result = finalizer();
        tokens.remove(account_id);
        Some(result)
    }
}

pub struct AppState {
    pub database: Mutex<Database>,
    pub secret_store: Arc<dyn SecretStore>,
    pub sync: Arc<SyncCoordinator>,
}

impl AppState {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        Ok(Self {
            database: Mutex::new(Database::open(path)?),
            secret_store: Arc::new(PlatformSecretStore::new()),
            sync: Arc::new(SyncCoordinator::new()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::SyncCoordinator;

    #[test]
    fn stale_sync_cannot_finalize_after_a_newer_sync_started() {
        let coordinator = SyncCoordinator::new();
        let old = coordinator.start("account");
        let current = coordinator.start("account");
        let finalized = AtomicUsize::new(0);

        assert!(coordinator
            .finish_current("account", &old, || finalized.fetch_add(1, Ordering::SeqCst))
            .is_none());
        assert_eq!(finalized.load(Ordering::SeqCst), 0);
        assert!(coordinator
            .finish_current("account", &current, || finalized
                .fetch_add(1, Ordering::SeqCst))
            .is_some());
        assert_eq!(finalized.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn try_start_does_not_interrupt_an_active_sync() {
        let coordinator = SyncCoordinator::new();
        let active = coordinator.try_start("account").expect("first sync");
        assert!(coordinator.try_start("account").is_none());
        assert!(!active.is_cancelled());

        coordinator.cancel("account");
        assert!(coordinator.try_start("account").is_some());
    }

    #[test]
    fn stale_sync_cannot_publish_progress() {
        let coordinator = SyncCoordinator::new();
        let stale = coordinator.start("account");
        let _current = coordinator.start("account");
        assert!(coordinator.with_current("account", &stale, || ()).is_none());
    }
}
