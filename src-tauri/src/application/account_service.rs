use async_trait::async_trait;

use crate::app_state::AppState;
use crate::backends::{
    imap::ImapIncomingBackend,
    incoming::{IncomingConfig, IncomingError, IncomingMailBackend, ServerCapabilities},
    outgoing::{OutgoingConfig, OutgoingError, OutgoingMailBackend},
    smtp::SmtpOutgoingBackend,
};
use crate::domain::account::{CreateAccountInput, EndpointConfig};
use crate::domain::Account;
use crate::errors::AppError;
use crate::providers::registry::{
    detect_provider, provider_presets, EndpointPreset, ProviderPreset,
};

fn validate_endpoint(endpoint: &EndpointConfig, label: &str) -> Result<(), AppError> {
    if endpoint.host.trim().is_empty() || endpoint.port == 0 {
        return Err(AppError::InvalidConfiguration(format!(
            "{label}端点需要服务器和有效端口"
        )));
    }
    if !matches!(endpoint.tls_mode.as_str(), "implicit" | "starttls") {
        return Err(AppError::InvalidConfiguration(format!(
            "{label}端点 TLS 模式不受支持"
        )));
    }
    if endpoint.username.trim().is_empty() {
        return Err(AppError::InvalidConfiguration(format!(
            "{label}端点需要用户名"
        )));
    }
    Ok(())
}

fn endpoint_from_preset(endpoint: EndpointPreset, email: &str) -> EndpointConfig {
    EndpointConfig {
        protocol: endpoint.protocol,
        host: endpoint.host,
        port: endpoint.port,
        tls_mode: endpoint.tls_mode,
        auth_method: endpoint
            .auth_methods
            .into_iter()
            .next()
            .unwrap_or_else(|| "password".into()),
        username: endpoint.username.unwrap_or_else(|| email.to_owned()),
    }
}

fn resolve_account_input(
    mut input: CreateAccountInput,
) -> Result<(CreateAccountInput, ProviderPreset), AppError> {
    input.email = input.email.trim().to_owned();
    input.display_name = input.display_name.trim().to_owned();
    if input.email.is_empty() || !input.email.contains('@') {
        return Err(AppError::InvalidConfiguration("请输入完整邮箱地址".into()));
    }
    let preset = provider_presets()
        .into_iter()
        .find(|preset| preset.id == input.provider_id)
        .or_else(|| detect_provider(&input.email))
        .ok_or_else(|| AppError::InvalidConfiguration("未找到该域名的 Provider preset".into()))?;

    if input.incoming.is_none() {
        input.incoming = preset
            .incoming
            .clone()
            .map(|endpoint| endpoint_from_preset(endpoint, &input.email));
    }
    if input.outgoing.is_none() {
        input.outgoing = preset
            .outgoing
            .clone()
            .map(|endpoint| endpoint_from_preset(endpoint, &input.email));
    }
    input.provider_id.clone_from(&preset.id);

    if let Some(endpoint) = &input.incoming {
        validate_endpoint(endpoint, "收件")?;
    }
    if let Some(endpoint) = &input.outgoing {
        validate_endpoint(endpoint, "发件")?;
    }
    if input.incoming.is_none() && input.outgoing.is_none() {
        return Err(AppError::InvalidConfiguration(
            "账户至少需要一个收件或发件端点".into(),
        ));
    }

    Ok((input, preset))
}

fn incoming_config(endpoint: &EndpointConfig) -> IncomingConfig {
    IncomingConfig {
        protocol: endpoint.protocol.clone(),
        host: endpoint.host.clone(),
        port: endpoint.port,
        tls_mode: endpoint.tls_mode.clone(),
        auth_method: endpoint.auth_method.clone(),
        username: endpoint.username.clone(),
    }
}

fn outgoing_config(endpoint: &EndpointConfig) -> OutgoingConfig {
    OutgoingConfig {
        protocol: endpoint.protocol.clone(),
        host: endpoint.host.clone(),
        port: endpoint.port,
        tls_mode: endpoint.tls_mode.clone(),
        auth_method: endpoint.auth_method.clone(),
        username: endpoint.username.clone(),
    }
}

fn map_incoming_error(error: IncomingError) -> AppError {
    match error {
        IncomingError::Authentication => AppError::Authentication,
        IncomingError::Unsupported(message) => AppError::Capability(message),
        IncomingError::Tls(message) | IncomingError::Network(message) => AppError::Network(message),
        IncomingError::Protocol(message) => AppError::Protocol(message),
    }
}

fn map_outgoing_error(error: OutgoingError) -> AppError {
    match error {
        OutgoingError::Authentication => AppError::Authentication,
        OutgoingError::Unsupported(message) => AppError::Capability(message),
        OutgoingError::AmbiguousSend => AppError::AmbiguousSend,
        OutgoingError::Tls(message) | OutgoingError::Network(message) => AppError::Network(message),
        OutgoingError::Rejected(message) => AppError::ServerRejected(message),
    }
}

#[async_trait]
trait AccountConnectionTester: Send + Sync {
    async fn test_incoming(
        &self,
        config: &IncomingConfig,
        secret: &str,
    ) -> Result<ServerCapabilities, AppError>;
    async fn test_outgoing(&self, config: &OutgoingConfig, secret: &str) -> Result<(), AppError>;
}

struct LiveConnectionTester;

#[async_trait]
impl AccountConnectionTester for LiveConnectionTester {
    async fn test_incoming(
        &self,
        config: &IncomingConfig,
        secret: &str,
    ) -> Result<ServerCapabilities, AppError> {
        ImapIncomingBackend::new(config.clone())
            .test_connection(secret)
            .await
            .map_err(map_incoming_error)
    }

    async fn test_outgoing(&self, config: &OutgoingConfig, secret: &str) -> Result<(), AppError> {
        SmtpOutgoingBackend::new(config.clone())
            .test_connection(secret)
            .await
            .map_err(map_outgoing_error)
    }
}

pub async fn create_account(
    state: &AppState,
    input: CreateAccountInput,
) -> Result<Account, AppError> {
    create_account_with_tester(state, input, &LiveConnectionTester).await
}

async fn create_account_with_tester(
    state: &AppState,
    input: CreateAccountInput,
    tester: &dyn AccountConnectionTester,
) -> Result<Account, AppError> {
    let (input, preset) = resolve_account_input(input)?;
    let duplicate_exists = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .list_accounts()?
        .into_iter()
        .any(|account| {
            account.provider_id == input.provider_id
                && account.email.eq_ignore_ascii_case(input.email.trim())
        });
    if duplicate_exists {
        return Err(AppError::InvalidConfiguration("该邮箱账户已经添加".into()));
    }
    let incoming_secret = input
        .incoming_secret
        .as_deref()
        .unwrap_or(input.secret.as_str());
    let outgoing_secret = input
        .outgoing_secret
        .as_deref()
        .unwrap_or(input.secret.as_str());

    if input.incoming.is_some() && incoming_secret.is_empty() {
        return Err(AppError::InvalidConfiguration("收件端点需要凭据".into()));
    }
    if input.outgoing.is_some() && outgoing_secret.is_empty() {
        return Err(AppError::InvalidConfiguration("发件端点需要凭据".into()));
    }

    // Validation is all-or-nothing: neither SQLite nor the keyring is touched until
    // every configured endpoint has accepted the supplied credential.
    if let Some(endpoint) = &input.incoming {
        tester
            .test_incoming(&incoming_config(endpoint), incoming_secret)
            .await?;
    }
    if let Some(endpoint) = &input.outgoing {
        tester
            .test_outgoing(&outgoing_config(endpoint), outgoing_secret)
            .await?;
    }

    let id_hint = uuid::Uuid::new_v4().to_string();
    let incoming_ref = format!("account/{id_hint}/incoming");
    let incoming_enabled = input.incoming.is_some();
    let outgoing_enabled = input.outgoing.is_some();
    let shared_secret = incoming_enabled && outgoing_enabled && incoming_secret == outgoing_secret;
    let outgoing_ref = if shared_secret {
        incoming_ref.clone()
    } else {
        format!("account/{id_hint}/outgoing")
    };

    if incoming_enabled {
        state
            .secret_store
            .set(&incoming_ref, incoming_secret)
            .map_err(|error| AppError::SecretStore(error.to_string()))?;
    }
    if outgoing_enabled && !shared_secret {
        if let Err(error) = state.secret_store.set(&outgoing_ref, outgoing_secret) {
            if incoming_enabled {
                let _ = state.secret_store.delete(&incoming_ref);
            }
            return Err(AppError::SecretStore(error.to_string()));
        }
    }

    // The OS keyring and SQLite cannot share a real transaction. If the database
    // commit fails, delete the provisional keyring entries as compensation.
    let account_result = match state.database.lock() {
        Ok(mut database) => database.create_account(
            &input,
            &preset,
            &incoming_ref,
            &outgoing_ref,
            incoming_enabled,
            outgoing_enabled,
        ),
        Err(_) => Err(AppError::Internal("database lock poisoned".into())),
    };
    match account_result {
        Ok(account) => Ok(account),
        Err(error) => {
            if incoming_enabled {
                let _ = state.secret_store.delete(&incoming_ref);
            }
            if outgoing_enabled && !shared_secret {
                let _ = state.secret_store.delete(&outgoing_ref);
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::{create_account_with_tester, AccountConnectionTester};
    use crate::app_state::{AppState, SyncCoordinator};
    use crate::auth::secret_store::{SecretStore, SecretStoreError};
    use crate::backends::incoming::{IncomingConfig, ServerCapabilities};
    use crate::backends::outgoing::OutgoingConfig;
    use crate::domain::account::{CreateAccountInput, EndpointConfig};
    use crate::domain::capabilities::ProviderCapabilities;
    use crate::errors::{AppError, AppErrorDto};
    use crate::storage::database::Database;

    #[derive(Default)]
    struct MemorySecretStore {
        values: Mutex<HashMap<String, String>>,
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_on_set_number: Option<usize>,
        set_count: Mutex<usize>,
    }

    impl MemorySecretStore {
        fn with_events(events: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                events,
                ..Self::default()
            }
        }

        fn failing_on_set(events: Arc<Mutex<Vec<&'static str>>>, number: usize) -> Self {
            Self {
                events,
                fail_on_set_number: Some(number),
                ..Self::default()
            }
        }
    }

    impl SecretStore for MemorySecretStore {
        fn set(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError> {
            self.events.lock().expect("event lock").push("secret:set");
            let mut set_count = self.set_count.lock().expect("set count lock");
            *set_count += 1;
            if self.fail_on_set_number == Some(*set_count) {
                return Err(SecretStoreError::OperationFailed);
            }
            self.values
                .lock()
                .expect("secret lock")
                .insert(reference.to_owned(), secret.to_owned());
            Ok(())
        }

        fn get(&self, reference: &str) -> Result<String, SecretStoreError> {
            self.values
                .lock()
                .expect("secret lock")
                .get(reference)
                .cloned()
                .ok_or(SecretStoreError::NotFound)
        }

        fn delete(&self, reference: &str) -> Result<(), SecretStoreError> {
            self.events
                .lock()
                .expect("event lock")
                .push("secret:delete");
            self.values.lock().expect("secret lock").remove(reference);
            Ok(())
        }
    }

    struct StubConnectionTester {
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_incoming: bool,
        fail_outgoing: bool,
    }

    #[async_trait::async_trait]
    impl AccountConnectionTester for StubConnectionTester {
        async fn test_incoming(
            &self,
            _config: &IncomingConfig,
            _secret: &str,
        ) -> Result<ServerCapabilities, AppError> {
            self.events
                .lock()
                .expect("event lock")
                .push("probe:incoming");
            if self.fail_incoming {
                Err(AppError::Authentication)
            } else {
                Ok(ServerCapabilities {
                    backend: "imap".into(),
                    capabilities: ProviderCapabilities::default(),
                    greeting: None,
                })
            }
        }

        async fn test_outgoing(
            &self,
            _config: &OutgoingConfig,
            _secret: &str,
        ) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("event lock")
                .push("probe:outgoing");
            if self.fail_outgoing {
                Err(AppError::Authentication)
            } else {
                Ok(())
            }
        }
    }

    struct DuplicateRaceTester {
        state: Arc<AppState>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl AccountConnectionTester for DuplicateRaceTester {
        async fn test_incoming(
            &self,
            _config: &IncomingConfig,
            _secret: &str,
        ) -> Result<ServerCapabilities, AppError> {
            self.events
                .lock()
                .expect("event lock")
                .push("probe:incoming");
            Ok(ServerCapabilities {
                backend: "imap".into(),
                capabilities: ProviderCapabilities::default(),
                greeting: None,
            })
        }

        async fn test_outgoing(
            &self,
            _config: &OutgoingConfig,
            _secret: &str,
        ) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("event lock")
                .push("probe:outgoing");
            let preset = crate::providers::registry::provider_presets()
                .into_iter()
                .find(|preset| preset.id == "qq")
                .expect("QQ preset");
            self.state
                .database
                .lock()
                .expect("database lock")
                .create_account(
                    &qq_input(),
                    &preset,
                    "account/racing/incoming",
                    "account/racing/outgoing",
                    true,
                    true,
                )?;
            Ok(())
        }
    }

    fn qq_input() -> CreateAccountInput {
        CreateAccountInput {
            email: "person@qq.com".into(),
            display_name: "Person".into(),
            provider_id: "qq".into(),
            secret: "authorization-code".into(),
            incoming_secret: None,
            outgoing_secret: None,
            incoming: None,
            outgoing: None,
        }
    }

    fn outbound_only_input() -> CreateAccountInput {
        CreateAccountInput {
            email: "sender@example.com".into(),
            display_name: "Sender".into(),
            provider_id: "generic-smtp".into(),
            secret: "smtp-secret".into(),
            incoming_secret: None,
            outgoing_secret: None,
            incoming: None,
            outgoing: Some(EndpointConfig {
                protocol: "smtp".into(),
                host: "smtp.example.com".into(),
                port: 587,
                tls_mode: "starttls".into(),
                auth_method: "password".into(),
                username: "smtp-user".into(),
            }),
        }
    }

    fn state_with_store(store: Arc<dyn SecretStore>) -> AppState {
        AppState {
            database: Mutex::new(Database::open_in_memory().expect("database")),
            secret_store: store,
            sync: Arc::new(SyncCoordinator::new()),
        }
    }

    #[tokio::test]
    async fn incoming_auth_failure_persists_nothing() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(MemorySecretStore::with_events(events.clone()));
        let state = state_with_store(store.clone());
        let tester = StubConnectionTester {
            events: events.clone(),
            fail_incoming: true,
            fail_outgoing: false,
        };

        let error = create_account_with_tester(&state, qq_input(), &tester)
            .await
            .expect_err("incoming authentication must fail");

        assert!(matches!(error, AppError::Authentication));
        assert_eq!(*events.lock().expect("event lock"), ["probe:incoming"]);
        assert!(store.values.lock().expect("secret lock").is_empty());
        assert!(state
            .database
            .lock()
            .expect("database lock")
            .list_accounts()
            .expect("accounts")
            .is_empty());
    }

    #[tokio::test]
    async fn outgoing_auth_failure_after_incoming_success_persists_nothing() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(MemorySecretStore::with_events(events.clone()));
        let state = state_with_store(store.clone());
        let tester = StubConnectionTester {
            events: events.clone(),
            fail_incoming: false,
            fail_outgoing: true,
        };

        let error = create_account_with_tester(&state, qq_input(), &tester)
            .await
            .expect_err("outgoing authentication must fail");

        let ipc_error = AppErrorDto::from(error);
        assert_eq!(ipc_error.code, "authentication");
        assert!(!ipc_error.retryable);
        assert_eq!(
            *events.lock().expect("event lock"),
            ["probe:incoming", "probe:outgoing"]
        );
        assert!(store.values.lock().expect("secret lock").is_empty());
        assert!(state
            .database
            .lock()
            .expect("database lock")
            .list_accounts()
            .expect("accounts")
            .is_empty());
    }

    #[tokio::test]
    async fn successful_probes_happen_before_any_persistence() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(MemorySecretStore::with_events(events.clone()));
        let state = state_with_store(store.clone());
        let tester = StubConnectionTester {
            events: events.clone(),
            fail_incoming: false,
            fail_outgoing: false,
        };

        let account = create_account_with_tester(&state, qq_input(), &tester)
            .await
            .expect("account creation");

        assert_eq!(account.email, "person@qq.com");
        assert_eq!(
            *events.lock().expect("event lock"),
            ["probe:incoming", "probe:outgoing", "secret:set"]
        );
        assert_eq!(store.values.lock().expect("secret lock").len(), 1);
        let (incoming_ref, outgoing_ref) = state
            .database
            .lock()
            .expect("database lock")
            .account_secret_refs(&account.id)
            .expect("secret refs");
        assert_eq!(incoming_ref, outgoing_ref);
        assert_eq!(
            state
                .database
                .lock()
                .expect("database lock")
                .list_accounts()
                .expect("accounts")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn outbound_only_account_validates_only_its_configured_endpoint() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(MemorySecretStore::with_events(events.clone()));
        let state = state_with_store(store.clone());
        let tester = StubConnectionTester {
            events: events.clone(),
            fail_incoming: false,
            fail_outgoing: false,
        };

        let account = create_account_with_tester(&state, outbound_only_input(), &tester)
            .await
            .expect("outbound-only account creation");

        assert!(!account.incoming_configured);
        assert!(account.outgoing_configured);
        assert_eq!(
            *events.lock().expect("event lock"),
            ["probe:outgoing", "secret:set"]
        );
        assert_eq!(store.values.lock().expect("secret lock").len(), 1);
        let outgoing = state
            .database
            .lock()
            .expect("database lock")
            .outgoing_config(&account.id)
            .expect("outgoing configuration");
        assert_eq!(outgoing.host, "smtp.example.com");
        assert_eq!(outgoing.port, 587);
    }

    #[tokio::test]
    async fn duplicate_account_is_rejected_before_network_or_secret_store_access() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(MemorySecretStore::with_events(events.clone()));
        let state = state_with_store(store.clone());
        let tester = StubConnectionTester {
            events: events.clone(),
            fail_incoming: false,
            fail_outgoing: false,
        };
        create_account_with_tester(&state, qq_input(), &tester)
            .await
            .expect("first account creation");
        events.lock().expect("event lock").clear();

        let mut duplicate = qq_input();
        duplicate.email = "PERSON@qq.com".into();
        let error = create_account_with_tester(&state, duplicate, &tester)
            .await
            .expect_err("duplicate account must be rejected");

        assert!(matches!(error, AppError::InvalidConfiguration(_)));
        assert!(events.lock().expect("event lock").is_empty());
        assert_eq!(store.values.lock().expect("secret lock").len(), 1);
        assert_eq!(
            state
                .database
                .lock()
                .expect("database lock")
                .list_accounts()
                .expect("accounts")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn duplicate_race_at_database_commit_removes_provisional_secrets() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(MemorySecretStore::with_events(events.clone()));
        let state = Arc::new(state_with_store(store.clone()));
        let tester = DuplicateRaceTester {
            state: state.clone(),
            events: events.clone(),
        };

        let error = create_account_with_tester(&state, qq_input(), &tester)
            .await
            .expect_err("database duplicate check must win the race");

        assert!(matches!(error, AppError::InvalidConfiguration(_)));
        assert_eq!(
            *events.lock().expect("event lock"),
            [
                "probe:incoming",
                "probe:outgoing",
                "secret:set",
                "secret:delete"
            ]
        );
        assert!(store.values.lock().expect("secret lock").is_empty());
        assert_eq!(
            state
                .database
                .lock()
                .expect("database lock")
                .list_accounts()
                .expect("accounts")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn partial_secret_store_failure_rolls_back_first_secret_and_account() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(MemorySecretStore::failing_on_set(events.clone(), 2));
        let state = state_with_store(store.clone());
        let tester = StubConnectionTester {
            events: events.clone(),
            fail_incoming: false,
            fail_outgoing: false,
        };

        let mut input = qq_input();
        input.outgoing_secret = Some("different-outgoing-code".into());
        let error = create_account_with_tester(&state, input, &tester)
            .await
            .expect_err("second keyring write must fail");

        assert!(matches!(error, AppError::SecretStore(_)));
        assert_eq!(
            *events.lock().expect("event lock"),
            [
                "probe:incoming",
                "probe:outgoing",
                "secret:set",
                "secret:set",
                "secret:delete"
            ]
        );
        assert!(store.values.lock().expect("secret lock").is_empty());
        assert!(state
            .database
            .lock()
            .expect("database lock")
            .list_accounts()
            .expect("accounts")
            .is_empty());
    }

    #[tokio::test]
    async fn distinct_incoming_and_outgoing_credentials_use_distinct_keyring_items() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(MemorySecretStore::with_events(events.clone()));
        let state = state_with_store(store.clone());
        let tester = StubConnectionTester {
            events: events.clone(),
            fail_incoming: false,
            fail_outgoing: false,
        };
        let mut input = qq_input();
        input.outgoing_secret = Some("different-outgoing-code".into());

        let account = create_account_with_tester(&state, input, &tester)
            .await
            .expect("account creation");

        assert_eq!(store.values.lock().expect("secret lock").len(), 2);
        let (incoming_ref, outgoing_ref) = state
            .database
            .lock()
            .expect("database lock")
            .account_secret_refs(&account.id)
            .expect("secret refs");
        assert_ne!(incoming_ref, outgoing_ref);
    }
}
