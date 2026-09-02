use lettre::address::{Address, Envelope};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};

use super::outgoing::{OutgoingConfig, OutgoingError, OutgoingMailBackend, SendResult};
use crate::domain::capabilities::ProviderCapabilities;

pub struct SmtpOutgoingBackend {
    pub config: OutgoingConfig,
}

impl SmtpOutgoingBackend {
    pub fn new(config: OutgoingConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl OutgoingMailBackend for SmtpOutgoingBackend {
    fn backend_name(&self) -> &'static str {
        "smtp"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            append_sent: true,
            smtp_utf8: true,
            ..Default::default()
        }
    }

    async fn test_connection(&self, secret: &str) -> Result<(), OutgoingError> {
        let config = self.config.clone();
        let secret = secret.to_owned();
        tokio::task::spawn_blocking(move || test_smtp(&config, &secret))
            .await
            .map_err(|error| OutgoingError::Network(error.to_string()))?
    }

    async fn send_mime(
        &self,
        secret: &str,
        mime: Vec<u8>,
        envelope_from: &str,
        recipients: &[String],
    ) -> Result<SendResult, OutgoingError> {
        let config = self.config.clone();
        let secret = secret.to_owned();
        let envelope_from = envelope_from.to_owned();
        let recipients = recipients.to_vec();
        tokio::task::spawn_blocking(move || {
            send_smtp(&config, &secret, mime, &envelope_from, &recipients)
        })
        .await
        .map_err(|error| OutgoingError::Network(error.to_string()))?
    }
}

fn transport(config: &OutgoingConfig, secret: &str) -> Result<SmtpTransport, OutgoingError> {
    if config.host.trim().is_empty() || config.username.trim().is_empty() || secret.is_empty() {
        return Err(OutgoingError::Rejected(
            "SMTP endpoint or credential is incomplete".into(),
        ));
    }
    if config.protocol != "smtp" {
        return Err(OutgoingError::Unsupported(format!(
            "outgoing protocol {} is not implemented by the SMTP backend",
            config.protocol
        )));
    }
    if config.auth_method != "password" && config.auth_method != "api-token" {
        return Err(OutgoingError::Unsupported(format!(
            "SMTP auth method {} needs its own token flow",
            config.auth_method
        )));
    }
    if !matches!(config.tls_mode.as_str(), "implicit" | "starttls") {
        return Err(OutgoingError::Unsupported(
            "only implicit TLS and STARTTLS are allowed".into(),
        ));
    }
    let credentials = Credentials::new(config.username.clone(), secret.to_owned());
    let builder = if config.tls_mode == "starttls" {
        SmtpTransport::starttls_relay(&config.host)
    } else {
        SmtpTransport::relay(&config.host)
    }
    .map_err(|error| OutgoingError::Tls(error.to_string()))?;
    Ok(builder.port(config.port).credentials(credentials).build())
}

fn test_smtp(config: &OutgoingConfig, secret: &str) -> Result<(), OutgoingError> {
    let transport = transport(config, secret)?;
    transport
        .test_connection()
        .map(|_| ())
        .map_err(|error| classify_smtp_error(&error.to_string()))
}

#[allow(dead_code)]
fn send_smtp(
    config: &OutgoingConfig,
    secret: &str,
    mime: Vec<u8>,
    envelope_from: &str,
    recipients: &[String],
) -> Result<SendResult, OutgoingError> {
    let sender = envelope_from
        .parse::<Address>()
        .map_err(|error| OutgoingError::Rejected(format!("invalid sender: {error}")))?;
    let recipients = recipients
        .iter()
        .map(|recipient| {
            recipient
                .parse::<Address>()
                .map_err(|error| OutgoingError::Rejected(format!("invalid recipient: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let envelope = Envelope::new(Some(sender), recipients)
        .map_err(|error| OutgoingError::Rejected(format!("invalid SMTP envelope: {error}")))?;
    let transport = transport(config, secret)?;
    match transport.send_raw(&envelope, &mime) {
        Ok(_) => Ok(SendResult::Sent { remote_id: None }),
        Err(error) => {
            let text = error.to_string();
            if text.contains("connection") {
                Ok(SendResult::OutcomeUnknown)
            } else {
                Err(classify_smtp_error(&text))
            }
        }
    }
}

fn classify_smtp_error(text: &str) -> OutgoingError {
    let lower = text.to_ascii_lowercase();
    if lower.contains("auth") || lower.contains("credential") || lower.contains("535") {
        OutgoingError::Authentication
    } else if lower.contains("timed out") || lower.contains("timeout") {
        OutgoingError::Network("SMTP connection timed out".into())
    } else {
        OutgoingError::Rejected("SMTP server rejected the operation".into())
    }
}
