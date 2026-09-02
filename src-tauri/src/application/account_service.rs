use crate::app_state::AppState;
use crate::domain::account::CreateAccountInput;
use crate::domain::Account;
use crate::errors::AppError;
use crate::providers::registry::{detect_provider, provider_presets};

fn validate_endpoint(
    endpoint: &crate::domain::account::EndpointConfig,
    label: &str,
) -> Result<(), AppError> {
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

pub fn create_account(state: &AppState, input: CreateAccountInput) -> Result<Account, AppError> {
    if input.email.trim().is_empty() || !input.email.contains('@') {
        return Err(AppError::InvalidConfiguration("请输入完整邮箱地址".into()));
    }
    let preset = provider_presets()
        .into_iter()
        .find(|preset| preset.id == input.provider_id)
        .or_else(|| detect_provider(&input.email))
        .ok_or_else(|| AppError::InvalidConfiguration("未找到该域名的 Provider preset".into()))?;
    if let Some(endpoint) = &input.incoming {
        validate_endpoint(endpoint, "收件")?;
    }
    if let Some(endpoint) = &input.outgoing {
        validate_endpoint(endpoint, "发件")?;
    }
    let id_hint = uuid::Uuid::new_v4().to_string();
    let incoming_ref = format!("account/{id_hint}/incoming");
    let outgoing_ref = format!("account/{id_hint}/outgoing");
    let incoming_enabled = input.incoming.is_some() || preset.incoming.is_some();
    let outgoing_enabled = input.outgoing.is_some() || preset.outgoing.is_some();
    if input.incoming.is_none()
        && preset
            .incoming
            .as_ref()
            .is_some_and(|endpoint| endpoint.host.trim().is_empty())
    {
        return Err(AppError::InvalidConfiguration(
            "请填写收件服务器端点".into(),
        ));
    }
    if input.outgoing.is_none()
        && preset
            .outgoing
            .as_ref()
            .is_some_and(|endpoint| endpoint.host.trim().is_empty())
    {
        return Err(AppError::InvalidConfiguration(
            "请填写发件服务器端点".into(),
        ));
    }
    if (incoming_enabled
        && input
            .incoming_secret
            .as_deref()
            .unwrap_or(&input.secret)
            .is_empty())
        || (outgoing_enabled
            && input
                .outgoing_secret
                .as_deref()
                .unwrap_or(&input.secret)
                .is_empty())
    {
        return Err(AppError::InvalidConfiguration(
            "收发端点至少需要一个凭据".into(),
        ));
    }
    if incoming_enabled {
        if let Err(error) = state.secret_store.set(
            &incoming_ref,
            input.incoming_secret.as_deref().unwrap_or(&input.secret),
        ) {
            return Err(AppError::SecretStore(error.to_string()));
        }
    }
    if outgoing_enabled {
        if let Err(error) = state.secret_store.set(
            &outgoing_ref,
            input.outgoing_secret.as_deref().unwrap_or(&input.secret),
        ) {
            if incoming_enabled {
                let _ = state.secret_store.delete(&incoming_ref);
            }
            return Err(AppError::SecretStore(error.to_string()));
        }
    }
    let account_result = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .create_account(
            &input,
            &preset,
            &incoming_ref,
            &outgoing_ref,
            incoming_enabled,
            outgoing_enabled,
        );
    match account_result {
        Ok(account) => Ok(account),
        Err(error) => {
            if incoming_enabled {
                let _ = state.secret_store.delete(&incoming_ref);
            }
            if outgoing_enabled {
                let _ = state.secret_store.delete(&outgoing_ref);
            }
            Err(error)
        }
    }
}
