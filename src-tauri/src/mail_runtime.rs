//! One account store and sync engine shared by the UI and native background entry points.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(not(target_os = "android"))]
use tauri_plugin_notification::NotificationExt;

use crate::app_state::AppState;
use crate::errors::AppError;

pub struct MailRuntime {
    pub state: AppState,
    app: RwLock<Option<AppHandle>>,
    started: AtomicBool,
    pub background_slots: Arc<tokio::sync::Semaphore>,
}

impl MailRuntime {
    pub fn open(path: &Path) -> Result<Arc<Self>, AppError> {
        Ok(Arc::new(Self {
            state: AppState::open(path)?,
            app: RwLock::new(None),
            started: AtomicBool::new(false),
            background_slots: Arc::new(tokio::sync::Semaphore::new(2)),
        }))
    }

    pub fn from_app(app: &AppHandle) -> Arc<Self> {
        Arc::clone(app.state::<Arc<Self>>().inner())
    }

    pub fn attach(&self, app: AppHandle) {
        if let Ok(mut target) = self.app.write() {
            *target = Some(app);
        }
    }

    pub fn start(self: &Arc<Self>) {
        if !self.started.swap(true, Ordering::AcqRel) {
            crate::application::realtime_sync_service::start(Arc::clone(self));
        }
    }

    pub fn emit<T: Serialize + Clone>(&self, event: &str, payload: T) -> tauri::Result<()> {
        #[cfg(target_os = "android")]
        if event == "sync-progress" {
            if let Ok(status) = serde_json::to_value(&payload) {
                if let Some(account) = status["accountId"].as_str() {
                    crate::background::android::sync_activity(
                        account,
                        status["state"] == "syncing",
                    );
                }
            }
        }
        let app = self.app.read().ok().and_then(|app| app.clone());
        if let Some(app) = app {
            app.emit(event, payload)?;
        }
        Ok(())
    }

    pub fn notify_new_mail(&self, inserted: usize) {
        #[cfg(target_os = "android")]
        crate::background::android::notify_new_mail(inserted);
        #[cfg(not(target_os = "android"))]
        if let Some(app) = self.app.read().ok().and_then(|app| app.clone()) {
            if let Err(error) = app
                .notification()
                .builder()
                .id(4_201)
                .title("Mutsumi Mail")
                .body(format!("你有 {inserted} 封新邮件"))
                .show()
            {
                tracing::debug!(%error, "new-mail notification was not delivered");
            }
        }
    }
}
