use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::auth::secret_store::{PlatformSecretStore, SecretStore};
use crate::domain::SyncStatus;
use crate::errors::AppError;
use crate::storage::database::Database;

pub struct SyncCoordinator {
    tokens: Mutex<std::collections::HashMap<String, CancellationToken>>,
    statuses: Mutex<std::collections::HashMap<String, SyncStatus>>,
    state_changes: watch::Sender<u64>,
}

impl SyncCoordinator {
    pub fn new() -> Self {
        let (state_changes, _) = watch::channel(0_u64);
        Self {
            tokens: Mutex::new(std::collections::HashMap::new()),
            statuses: Mutex::new(std::collections::HashMap::new()),
            state_changes,
        }
    }
    pub fn start(&self, account_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        if let Ok(mut tokens) = self.tokens.lock() {
            if let Some(previous) = tokens.insert(account_id.to_owned(), token.clone()) {
                previous.cancel();
            }
        }
        self.notify_state_change();
        token
    }
    pub fn try_start(&self, account_id: &str) -> Option<CancellationToken> {
        let token = CancellationToken::new();
        let mut tokens = self.tokens.lock().ok()?;
        if tokens.contains_key(account_id) {
            return None;
        }
        tokens.insert(account_id.to_owned(), token.clone());
        drop(tokens);
        self.notify_state_change();
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
        self.notify_state_change();
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

    pub fn is_active(&self, account_id: &str) -> bool {
        self.tokens
            .lock()
            .is_ok_and(|tokens| tokens.contains_key(account_id))
    }

    /// Waits for the account's current sync slot to clear without polling. A background
    /// notification can be coalesced behind a user-initiated refresh without competing for the
    /// account's connection or cancelling the user's work.
    pub async fn wait_until_idle(
        &self,
        account_id: &str,
        cancellation: &CancellationToken,
    ) -> bool {
        let mut changes = self.state_changes.subscribe();
        loop {
            if !self.is_active(account_id) {
                return true;
            }
            tokio::select! {
                _ = changes.changed() => {}
                _ = cancellation.cancelled() => return false,
            }
        }
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
        drop(tokens);
        self.notify_state_change();
        Some(result)
    }

    fn notify_state_change(&self) {
        let next = (*self.state_changes.borrow()).wrapping_add(1);
        self.state_changes.send_replace(next);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RealtimeState {
    network_allowed: bool,
    revision: u64,
}

/// Coordinates the lifecycle of long-lived IMAP listeners. It deliberately has no timer: the
/// realtime service wakes only for account changes or a platform lifecycle transition.
pub struct RealtimeSyncCoordinator {
    state_changes: watch::Sender<RealtimeState>,
    restart_requests: Mutex<std::collections::HashSet<String>>,
}

impl RealtimeSyncCoordinator {
    pub fn new() -> Self {
        let (state_changes, _) = watch::channel(RealtimeState {
            network_allowed: true,
            revision: 0,
        });
        Self {
            state_changes,
            restart_requests: Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn network_allowed(&self) -> bool {
        self.state_changes.borrow().network_allowed
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<RealtimeState> {
        self.state_changes.subscribe()
    }

    pub fn wake(&self) {
        self.update_state(|_| {});
    }

    /// A manual reconnect or newly verified account is an explicit request to retry a listener
    /// that was stopped after an authentication or secure-storage failure.
    pub fn restart_account(&self, account_id: &str) {
        if let Ok(mut requested) = self.restart_requests.lock() {
            requested.insert(account_id.to_owned());
        }
        self.wake();
    }

    pub fn take_restart_requests(&self) -> std::collections::HashSet<String> {
        self.restart_requests
            .lock()
            .map(|mut requested| std::mem::take(&mut *requested))
            .unwrap_or_default()
    }

    #[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
    pub fn suspend(&self) {
        self.update_state(|state| state.network_allowed = false);
    }

    #[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
    pub fn resume(&self) {
        self.update_state(|state| state.network_allowed = true);
    }

    fn update_state(&self, update: impl FnOnce(&mut RealtimeState)) {
        let mut next = *self.state_changes.borrow();
        update(&mut next);
        next.revision = next.revision.wrapping_add(1);
        self.state_changes.send_replace(next);
    }
}

pub struct AppState {
    pub database: Mutex<Database>,
    pub secret_store: Arc<dyn SecretStore>,
    pub sync: Arc<SyncCoordinator>,
    pub realtime: Arc<RealtimeSyncCoordinator>,
}

impl AppState {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        Ok(Self {
            database: Mutex::new(Database::open(path)?),
            secret_store: Arc::new(PlatformSecretStore::new()),
            sync: Arc::new(SyncCoordinator::new()),
            realtime: Arc::new(RealtimeSyncCoordinator::new()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::{RealtimeSyncCoordinator, SyncCoordinator};
    use tokio_util::sync::CancellationToken;

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

    #[tokio::test]
    async fn wait_until_idle_is_event_driven_and_cancellable() {
        let coordinator = Arc::new(SyncCoordinator::new());
        let current = coordinator.start("account");
        let cancellation = CancellationToken::new();
        let waiter = {
            let coordinator = Arc::clone(&coordinator);
            let cancellation = cancellation.clone();
            tokio::spawn(async move { coordinator.wait_until_idle("account", &cancellation).await })
        };

        coordinator.finish_current("account", &current, || ());
        assert!(waiter.await.expect("waiter task"));

        let active = coordinator.start("account");
        let cancellation = CancellationToken::new();
        let waiter = {
            let coordinator = Arc::clone(&coordinator);
            let cancellation = cancellation.clone();
            tokio::spawn(async move { coordinator.wait_until_idle("account", &cancellation).await })
        };
        cancellation.cancel();
        assert!(!waiter.await.expect("cancelled waiter task"));
        coordinator.cancel("account");
        assert!(active.is_cancelled());
    }

    #[test]
    fn realtime_lifecycle_wakes_without_a_polling_timer() {
        let coordinator = RealtimeSyncCoordinator::new();
        let mut changes = coordinator.subscribe();
        coordinator.suspend();
        assert!(changes.has_changed().expect("sender is alive"));
        assert!(!coordinator.network_allowed());
        changes.borrow_and_update();
        coordinator.resume();
        assert!(changes.has_changed().expect("sender is alive"));
        assert!(coordinator.network_allowed());
    }
}
