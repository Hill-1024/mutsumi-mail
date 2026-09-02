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
