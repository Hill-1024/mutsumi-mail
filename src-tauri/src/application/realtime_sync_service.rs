use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::mail_runtime::MailRuntime;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app_state::RealtimeSyncCoordinator;
use crate::application::sync_service;
use crate::backends::imap::ImapIncomingBackend;
use crate::backends::incoming::{IncomingConfig, IncomingError};
use crate::domain::{Account, SyncStatus};
use crate::errors::AppError;

// Keep a selected push connection alive independently of sync. Renew well below RFC 2177's
// 29-minute ceiling so a half-open connection is detected after sleep or a network change.
const IDLE_RENEWAL: Duration = Duration::from_secs(4 * 60);
const RETRY_BASE: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(60);

// Also catch notifications lost by the provider, reconnect gaps and changes in other folders.
// This same timer handles servers without IDLE; it must not wait half an hour to receive mail.
#[cfg(any(target_os = "android", target_os = "ios"))]
const REFRESH_INTERVAL: Duration = Duration::from_secs(120);
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

struct WorkerSlot {
    cancellation: CancellationToken,
    generation: u64,
}

struct WorkerExit {
    account_id: String,
    generation: u64,
    terminal: bool,
}

enum WorkerOutcome {
    Cancelled,
    Terminal,
}

enum WatchError {
    Cancelled,
    Retryable(AppError),
    Terminal(AppError),
}

impl WatchError {
    fn from_sync(error: AppError) -> Self {
        if matches!(error, AppError::Cancelled) {
            Self::Cancelled
        } else if error.retryable() || matches!(error, AppError::Protocol(_)) {
            Self::Retryable(error)
        } else {
            Self::Terminal(error)
        }
    }

    fn from_idle(error: IncomingError) -> Self {
        match error {
            IncomingError::Authentication => Self::Terminal(AppError::Authentication),
            IncomingError::Unsupported(message) => Self::Terminal(AppError::Capability(message)),
            IncomingError::Network(message) | IncomingError::Tls(message) => {
                Self::Retryable(AppError::Network(message))
            }
            // A server may close an IDLE socket without a BYE response. The connection itself is
            // disposable, so retry this listener instead of permanently disabling realtime mail.
            IncomingError::Protocol(message) => Self::Retryable(AppError::Protocol(message)),
        }
    }
}

pub fn start(app: Arc<MailRuntime>) {
    let state = &app.state;
    let coordinator = Arc::clone(&state.realtime);
    tauri::async_runtime::spawn(async move {
        run_supervisor(app, coordinator).await;
    });
}

async fn run_supervisor(app: Arc<MailRuntime>, coordinator: Arc<RealtimeSyncCoordinator>) {
    let (worker_exits, mut worker_exit_events) = mpsc::unbounded_channel::<WorkerExit>();
    let mut workers = HashMap::<String, WorkerSlot>::new();
    let mut blocked = HashSet::<String>::new();
    let mut lifecycle_changes = coordinator.subscribe();
    let mut next_generation = 0_u64;
    let mut connection_epoch = coordinator.connection_epoch();

    loop {
        let current_epoch = coordinator.connection_epoch();
        if current_epoch != connection_epoch {
            cancel_workers(&mut workers);
            connection_epoch = current_epoch;
        }
        reconcile_workers(
            &app,
            &coordinator,
            &worker_exits,
            &mut workers,
            &mut blocked,
            &mut next_generation,
        );

        tokio::select! {
            changed = lifecycle_changes.changed() => {
                if changed.is_err() {
                    cancel_workers(&mut workers);
                    return;
                }
            }
            Some(exit) = worker_exit_events.recv() => {
                record_worker_exit(&mut workers, &mut blocked, exit);
            }
        }
    }
}

fn record_worker_exit(
    workers: &mut HashMap<String, WorkerSlot>,
    blocked: &mut HashSet<String>,
    exit: WorkerExit,
) {
    if workers
        .get(&exit.account_id)
        .is_some_and(|worker| worker.generation == exit.generation)
    {
        workers.remove(&exit.account_id);
        // Only credential/configuration failures require an explicit reconnect. An obsolete
        // generation must never remove or block the replacement listener for the same account.
        if exit.terminal {
            blocked.insert(exit.account_id);
        }
    }
}

fn reconcile_workers(
    app: &Arc<MailRuntime>,
    coordinator: &RealtimeSyncCoordinator,
    worker_exits: &mpsc::UnboundedSender<WorkerExit>,
    workers: &mut HashMap<String, WorkerSlot>,
    blocked: &mut HashSet<String>,
    next_generation: &mut u64,
) {
    let desired = match automatic_incoming_accounts(app) {
        Ok(accounts) => accounts,
        Err(error) => {
            tracing::warn!(error = %error, "unable to reconcile realtime IMAP workers");
            return;
        }
    };

    #[cfg(target_os = "android")]
    crate::background::android::update_policy(
        !desired.is_empty() && crate::background::enabled(&app.state),
    );

    for account_id in coordinator.take_restart_requests() {
        blocked.remove(&account_id);
        if let Some(worker) = workers.remove(&account_id) {
            worker.cancellation.cancel();
        }
    }

    // A disabled/paused account loses its terminal marker, so enabling it later starts a fresh
    // listener without needing a full application restart.
    blocked.retain(|account_id| desired.contains(account_id));

    if !coordinator.network_allowed() {
        cancel_workers(workers);
        return;
    }

    let no_longer_desired = workers
        .keys()
        .filter(|account_id| !desired.contains(*account_id))
        .cloned()
        .collect::<Vec<_>>();
    for account_id in no_longer_desired {
        if let Some(worker) = workers.remove(&account_id) {
            worker.cancellation.cancel();
        }
    }

    for account_id in desired {
        if blocked.contains(&account_id) || workers.contains_key(&account_id) {
            continue;
        }
        *next_generation = next_generation.wrapping_add(1);
        let generation = *next_generation;
        let cancellation = CancellationToken::new();
        workers.insert(
            account_id.clone(),
            WorkerSlot {
                cancellation: cancellation.clone(),
                generation,
            },
        );
        let app = app.clone();
        let worker_exits = worker_exits.clone();
        tauri::async_runtime::spawn(async move {
            let outcome = watch_account(app, account_id.clone(), cancellation.clone()).await;
            // Every unexpected exit must release its slot. Otherwise a cancelled sync can
            // leave a dead worker registered forever, with no listener and no refresh timer.
            if !cancellation.is_cancelled() {
                let _ = worker_exits.send(WorkerExit {
                    account_id,
                    generation,
                    terminal: matches!(outcome, WorkerOutcome::Terminal),
                });
            }
        });
    }
}

pub(crate) fn automatic_incoming_accounts(
    app: &Arc<MailRuntime>,
) -> Result<HashSet<String>, AppError> {
    let state = &app.state;
    let database = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
    let automatic = database.get_settings()?["syncPolicy"].as_str() == Some("automatic");
    let accounts = if automatic {
        database.list_accounts()?
    } else {
        Vec::new()
    };
    Ok(accounts
        .into_iter()
        .filter(is_realtime_candidate)
        .map(|account| account.id)
        .collect())
}

fn is_realtime_candidate(account: &Account) -> bool {
    account.enabled && account.incoming_configured && account.sync_policy == "automatic"
}

fn cancel_workers(workers: &mut HashMap<String, WorkerSlot>) {
    for worker in workers.values() {
        worker.cancellation.cancel();
    }
    workers.clear();
}

async fn watch_account(
    app: Arc<MailRuntime>,
    account_id: String,
    cancellation: CancellationToken,
) -> WorkerOutcome {
    let mut attempt = 0;
    let (config, secret) = loop {
        match load_realtime_session(&app, &account_id) {
            Ok(session) => break session,
            Err(error) => match WatchError::from_sync(error) {
                WatchError::Retryable(error) => {
                    report_realtime_problem(&app, &account_id, &error, true);
                    if !wait_for_retry(retry_delay(attempt, &account_id), &cancellation).await {
                        return WorkerOutcome::Cancelled;
                    }
                    attempt = attempt.saturating_add(1);
                }
                WatchError::Terminal(error) => {
                    report_realtime_problem(&app, &account_id, &error, false);
                    return WorkerOutcome::Terminal;
                }
                WatchError::Cancelled => return WorkerOutcome::Cancelled,
            },
        }
    };
    // Capacity one coalesces a burst while retaining a wake that arrives DURING a sync. The
    // listener remains in IDLE on its own connection while the sync session walks folders.
    let (signals, receiver) = mpsc::channel(1);
    tokio::select! {
        outcome = watch_inbox(&app, &account_id, &config, &secret, &cancellation, signals) => outcome,
        outcome = refresh_on_signals(&account_id, &cancellation, receiver, REFRESH_INTERVAL, |full_refresh| {
            synchronize_after_signal(&app, &account_id, &config, &secret, &cancellation, full_refresh)
        }) => outcome,
    }
}

async fn watch_inbox(
    app: &Arc<MailRuntime>,
    account_id: &str,
    config: &IncomingConfig,
    secret: &str,
    cancellation: &CancellationToken,
    signals: mpsc::Sender<()>,
) -> WorkerOutcome {
    let backend = ImapIncomingBackend::new(config.clone());
    let mut retry_attempt = 0_u32;
    loop {
        let connection = tokio::select! {
            _ = cancellation.cancelled() => return WorkerOutcome::Cancelled,
            result = backend.open_idle_connection(secret) => result,
        };
        let error = match connection {
            Ok(mut idle) => {
                // SELECT may have absorbed an arrival before IDLE was established. Catch up
                // after every new connection, even when the server sends no subsequent EXISTS.
                let _ = signals.try_send(());
                loop {
                    let started = tokio::time::Instant::now();
                    let result = tokio::select! {
                        _ = cancellation.cancelled() => return WorkerOutcome::Cancelled,
                        result = idle.wait_for_change(IDLE_RENEWAL) => result,
                    };
                    match result {
                        Ok(changed) => {
                            retry_attempt = 0;
                            if changed {
                                let _ = signals.try_send(());
                            } else if started.elapsed() < RETRY_BASE
                                && !wait_for_retry(RETRY_BASE, cancellation).await
                            {
                                return WorkerOutcome::Cancelled;
                            }
                            // DONE completed. Re-enter IDLE on the SAME selected session so
                            // changes during a sync or renewal cannot disappear into SELECT.
                        }
                        Err(error) => break error,
                    }
                }
            }
            Err(IncomingError::Unsupported(_)) => {
                // The independent refresh timer remains active, including its immediate pass.
                cancellation.cancelled().await;
                return WorkerOutcome::Cancelled;
            }
            Err(error) => error,
        };
        match WatchError::from_idle(error) {
            WatchError::Retryable(error) => {
                tracing::debug!(account_id, error = %error, "realtime listener will reconnect");
                report_realtime_problem(app, account_id, &error, true);
                if !wait_for_retry(retry_delay(retry_attempt, account_id), cancellation).await {
                    return WorkerOutcome::Cancelled;
                }
                retry_attempt = retry_attempt.saturating_add(1);
            }
            WatchError::Terminal(error) => {
                report_realtime_problem(app, account_id, &error, false);
                return WorkerOutcome::Terminal;
            }
            WatchError::Cancelled => return WorkerOutcome::Cancelled,
        }
    }
}

async fn refresh_on_signals<F, Fut>(
    account_id: &str,
    cancellation: &CancellationToken,
    mut signals: mpsc::Receiver<()>,
    refresh_interval: Duration,
    mut synchronize: F,
) -> WorkerOutcome
where
    F: FnMut(bool) -> Fut,
    Fut: Future<Output = Result<(), WatchError>>,
{
    let mut retry_attempt = 0_u32;
    let mut full_refresh = true;
    let mut last_full = tokio::time::Instant::now();
    loop {
        let result = tokio::select! {
            _ = cancellation.cancelled() => return WorkerOutcome::Cancelled,
            result = synchronize(full_refresh) => result,
        };
        match result {
            Ok(()) => {
                retry_attempt = 0;
                if full_refresh {
                    last_full = tokio::time::Instant::now();
                }
                full_refresh = tokio::select! {
                    _ = cancellation.cancelled() => return WorkerOutcome::Cancelled,
                    signal = signals.recv() => {
                        if signal.is_none() { return WorkerOutcome::Cancelled; }
                        true
                    }
                    _ = tokio::time::sleep(refresh_interval) => last_full.elapsed() >= Duration::from_secs(15 * 60),
                };
            }
            Err(WatchError::Cancelled) => return WorkerOutcome::Cancelled,
            Err(WatchError::Retryable(error)) => {
                tracing::debug!(account_id, error = %error, "background mail refresh will retry");
                if !wait_for_retry(retry_delay(retry_attempt, account_id), cancellation).await {
                    return WorkerOutcome::Cancelled;
                }
                retry_attempt = retry_attempt.saturating_add(1);
            }
            Err(WatchError::Terminal(error)) => {
                tracing::warn!(account_id, error = %error, "background mail refresh stopped for this account");
                return WorkerOutcome::Terminal;
            }
        }
    }
}

async fn synchronize_after_signal(
    app: &Arc<MailRuntime>,
    account_id: &str,
    config: &IncomingConfig,
    secret: &str,
    cancellation: &CancellationToken,
    full_refresh: bool,
) -> Result<(), WatchError> {
    loop {
        if cancellation.is_cancelled() {
            return Err(WatchError::Cancelled);
        }
        let permit = tokio::select! {
            _ = cancellation.cancelled() => return Err(WatchError::Cancelled),
            permit = app.background_slots.clone().acquire_owned() => permit.map_err(|_| WatchError::Cancelled)?,
        };
        let (sync, started) = {
            let state = &app.state;
            let sync = Arc::clone(&state.sync);
            let started = match sync_service::start_sync_if_idle_with_session(
                state,
                app.clone(),
                account_id.to_owned(),
                config.clone(),
                secret.to_owned(),
                full_refresh,
                sync_service::BackgroundSync {
                    cancellation: cancellation.clone(),
                    permit,
                },
            ) {
                // A manual refresh can replace our token between try_start and publication.
                // Wait for that refresh and catch up; it did not cancel the realtime worker.
                Err(AppError::Cancelled) if !cancellation.is_cancelled() => None,
                result => result.map_err(WatchError::from_sync)?,
            };
            (sync, started)
        };

        if !sync.wait_until_idle(account_id, cancellation).await {
            return Err(WatchError::Cancelled);
        }
        if started.is_none() {
            // A manual sync was already using this account. Once it completes, start one bounded
            // incremental pass to make sure an IDLE notification that raced it is not lost.
            continue;
        }

        match sync.status(account_id) {
            Some(status) if status.state == "error" => {
                let message = status
                    .message
                    .unwrap_or_else(|| "background sync failed".into());
                return if status.retryable {
                    Err(WatchError::Retryable(AppError::Network(message)))
                } else {
                    Err(WatchError::Terminal(AppError::Protocol(message)))
                };
            }
            Some(status) if status.state == "offline" => {
                return Err(WatchError::Retryable(AppError::Network(
                    status
                        .message
                        .unwrap_or_else(|| "IMAP listener is offline".into()),
                )));
            }
            _ => return Ok(()),
        }
    }
}

fn load_realtime_session(
    app: &Arc<MailRuntime>,
    account_id: &str,
) -> Result<(IncomingConfig, String), AppError> {
    let state = &app.state;
    sync_service::load_incoming_session(state, account_id)
}

async fn wait_for_retry(delay: Duration, cancellation: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancellation.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

fn retry_delay(attempt: u32, account_id: &str) -> Duration {
    let multiplier = 1_u64 << attempt.min(6);
    let base_millis = u64::try_from(RETRY_BASE.as_millis())
        .unwrap_or(u64::MAX)
        .saturating_mul(multiplier)
        .min(u64::try_from(RETRY_MAX.as_millis()).unwrap_or(u64::MAX));
    let jitter_bound = (base_millis / 5).max(1);
    let jitter = account_id
        .bytes()
        .fold(1_469_598_103_934_665_603_u64, |hash, byte| {
            hash.wrapping_mul(1_099_511_628_211)
                .wrapping_add(u64::from(byte))
        })
        % jitter_bound;
    Duration::from_millis(base_millis.saturating_add(jitter))
}

fn report_realtime_problem(
    app: &Arc<MailRuntime>,
    account_id: &str,
    error: &AppError,
    offline: bool,
) {
    let state = &app.state;
    if state.sync.is_active(account_id) {
        return;
    }
    let status = SyncStatus {
        account_id: account_id.to_owned(),
        state: if offline { "offline" } else { "error" }.into(),
        phase: Some("realtime".into()),
        processed: None,
        total: None,
        message: Some(if offline {
            "实时连接已断开，正在自动重连".into()
        } else {
            error.to_string()
        }),
        retryable: offline,
    };
    if let Ok(mut database) = state.database.lock() {
        let result = if offline {
            database.mark_account_sync_offline(account_id)
        } else {
            database.mark_account_sync_failed(account_id, &error.to_string())
        };
        if let Err(error) = result {
            tracing::debug!(account_id, error = %error, "unable to persist realtime listener status");
        }
    }
    state.sync.set_status(status.clone());
    let _ = app.emit("sync-progress", status);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Account;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn account(sync_policy: &str) -> Account {
        Account {
            id: "account-a".into(),
            provider_id: "generic".into(),
            email: "person@example.com".into(),
            display_name: "Person".into(),
            enabled: true,
            sync_policy: sync_policy.into(),
            incoming_configured: true,
            outgoing_configured: true,
            sync_status: "idle".into(),
            last_synced_at: None,
        }
    }

    #[test]
    fn only_enabled_automatic_incoming_accounts_keep_a_listener() {
        assert!(is_realtime_candidate(&account("automatic")));
        assert!(!is_realtime_candidate(&account("manual")));
        assert!(!is_realtime_candidate(&account("paused")));
        let mut no_incoming = account("automatic");
        no_incoming.incoming_configured = false;
        assert!(!is_realtime_candidate(&no_incoming));
    }

    #[test]
    fn retry_backoff_is_bounded_and_stable_per_account() {
        let initial = retry_delay(0, "account-a");
        let repeated = retry_delay(0, "account-a");
        let capped = retry_delay(99, "account-a");
        assert_eq!(initial, repeated);
        assert!(capped <= Duration::from_secs(72));
        assert!(capped > initial);
    }

    #[test]
    fn stale_exit_cannot_disable_replacement_and_cancelled_exit_releases_slot() {
        let mut workers = HashMap::from([(
            "a".to_owned(),
            WorkerSlot {
                generation: 2,
                cancellation: CancellationToken::new(),
            },
        )]);
        let mut blocked = HashSet::new();
        record_worker_exit(
            &mut workers,
            &mut blocked,
            WorkerExit {
                account_id: "a".into(),
                generation: 1,
                terminal: true,
            },
        );
        assert!(workers.contains_key("a"));
        assert!(!blocked.contains("a"));
        record_worker_exit(
            &mut workers,
            &mut blocked,
            WorkerExit {
                account_id: "a".into(),
                generation: 2,
                terminal: false,
            },
        );
        assert!(workers.is_empty());
        assert!(
            !blocked.contains("a"),
            "unexpected cancellation must allow a restart"
        );
        workers.insert(
            "a".into(),
            WorkerSlot {
                generation: 3,
                cancellation: CancellationToken::new(),
            },
        );
        record_worker_exit(
            &mut workers,
            &mut blocked,
            WorkerExit {
                account_id: "a".into(),
                generation: 3,
                terminal: true,
            },
        );
        assert!(workers.is_empty());
        assert!(
            blocked.contains("a"),
            "authentication failure must not retry forever"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn arrival_during_sync_is_retained_and_bursts_are_coalesced() {
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let finished = Arc::new(tokio::sync::Notify::new());
        let worker_finished = Arc::clone(&finished);
        let (signals, receiver) = mpsc::channel(1);
        let worker = tokio::spawn(async move {
            refresh_on_signals("a", &worker_token, receiver, REFRESH_INTERVAL, |_| {
                let calls = Arc::clone(&worker_calls);
                let finished = Arc::clone(&worker_finished);
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        finished.notified().await;
                    }
                    Ok(())
                }
            })
            .await
        });
        tokio::task::yield_now().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "startup refresh is automatic"
        );
        signals.try_send(()).expect("arrival during sync");
        assert!(matches!(
            signals.try_send(()),
            Err(mpsc::error::TrySendError::Full(()))
        ));
        finished.notify_one();
        tokio::task::yield_now().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "pending arrival refreshes immediately"
        );
        token.cancel();
        assert!(matches!(
            worker.await.expect("worker"),
            WorkerOutcome::Cancelled
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn missing_idle_notifications_still_refresh_automatically() {
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let (_signals, receiver) = mpsc::channel(1);
        let worker = tokio::spawn(async move {
            refresh_on_signals("a", &worker_token, receiver, REFRESH_INTERVAL, |_| {
                worker_calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(()))
            })
            .await
        });
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(REFRESH_INTERVAL).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        token.cancel();
        assert!(matches!(
            worker.await.expect("worker"),
            WorkerOutcome::Cancelled
        ));
        tokio::time::advance(REFRESH_INTERVAL).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "paused workers stay stopped"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn transient_sync_failure_retries_without_a_manual_wake() {
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let (_signals, receiver) = mpsc::channel(1);
        let worker = tokio::spawn(async move {
            refresh_on_signals("a", &worker_token, receiver, REFRESH_INTERVAL, |_| {
                let attempt = worker_calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(if attempt == 0 {
                    Err(WatchError::from_sync(AppError::Protocol(
                        "UID changed during FETCH".into(),
                    )))
                } else {
                    Ok(())
                })
            })
            .await
        });
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(retry_delay(0, "a")).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        token.cancel();
        assert!(matches!(
            worker.await.expect("worker"),
            WorkerOutcome::Cancelled
        ));
    }

    #[tokio::test]
    async fn authentication_failure_stops_instead_of_retrying_credentials() {
        let (_signals, receiver) = mpsc::channel(1);
        let calls = AtomicUsize::new(0);
        let outcome = refresh_on_signals(
            "a",
            &CancellationToken::new(),
            receiver,
            REFRESH_INTERVAL,
            |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Err(WatchError::from_sync(AppError::Authentication)))
            },
        )
        .await;
        assert!(matches!(outcome, WorkerOutcome::Terminal));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
