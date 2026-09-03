use lettre::address::{Address, Envelope};
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::Error as SmtpError;
use lettre::{SmtpTransport, Transport};
use std::time::Duration;

use super::outgoing::{OutgoingConfig, OutgoingError, OutgoingMailBackend, SendResult};
use crate::domain::capabilities::ProviderCapabilities;

pub struct SmtpOutgoingBackend {
    pub config: OutgoingConfig,
}

impl SmtpOutgoingBackend {
    pub fn new(config: OutgoingConfig) -> Self {
        Self { config }
    }

    pub fn validate_configuration(&self, secret: &str) -> Result<(), OutgoingError> {
        transport(&self.config, secret).map(|_| ())
    }
}

#[async_trait::async_trait]
impl OutgoingMailBackend for SmtpOutgoingBackend {
    fn backend_name(&self) -> &'static str {
        "smtp"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            // SMTP transports a message; saving a copy requires the incoming backend's
            // APPEND support and is not performed by this backend.
            append_sent: false,
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
        // A blocking worker failure does not tell us whether it happened before or after
        // the server accepted DATA. Keep the result ambiguous to avoid an automatic duplicate.
        .map_err(|_error| OutgoingError::AmbiguousSend)?
    }
}

fn transport(config: &OutgoingConfig, secret: &str) -> Result<SmtpTransport, OutgoingError> {
    if secret.is_empty() {
        return Err(OutgoingError::Authentication);
    }
    if config.host.trim().is_empty() || config.port == 0 || config.username.trim().is_empty() {
        return Err(OutgoingError::Unsupported(
            "SMTP 发件服务器配置不完整".into(),
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
    Ok(builder
        .port(config.port)
        .credentials(credentials)
        .timeout(Some(Duration::from_secs(20)))
        .build())
}

fn test_smtp(config: &OutgoingConfig, secret: &str) -> Result<(), OutgoingError> {
    let transport = transport(config, secret)?;
    let connected = transport
        .test_connection()
        .map_err(|error| classify_smtp_error(&error))?;
    ensure_connection_confirmed(connected)
}

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
    let mut envelope_recipients = Vec::new();
    for recipient in recipients {
        let address = recipient
            .parse::<Mailbox>()
            .map(|mailbox| mailbox.email)
            .map_err(|error| OutgoingError::Rejected(format!("invalid recipient: {error}")))?;
        if !envelope_recipients.contains(&address) {
            envelope_recipients.push(address);
        }
    }
    if envelope_recipients.is_empty() {
        return Err(OutgoingError::Rejected(
            "at least one recipient is required".into(),
        ));
    }
    let envelope = Envelope::new(Some(sender), envelope_recipients)
        .map_err(|error| OutgoingError::Rejected(format!("invalid SMTP envelope: {error}")))?;
    let transport = transport(config, secret)?;
    let connected = transport
        .test_connection()
        .map_err(|error| classify_smtp_error(&error))?;
    ensure_connection_confirmed(connected)?;
    match transport.send_raw(&envelope, &mime) {
        Ok(_) => Ok(SendResult::Sent { remote_id: None }),
        Err(error) => Err(classify_send_error(&error)),
    }
}

fn ensure_connection_confirmed(connected: bool) -> Result<(), OutgoingError> {
    if connected {
        Ok(())
    } else {
        Err(OutgoingError::Rejected(
            "SMTP server did not confirm the connection".into(),
        ))
    }
}

fn classify_smtp_error(error: &SmtpError) -> OutgoingError {
    let text = error.to_string();
    let lower = text.to_ascii_lowercase();
    let status = error.status().map(|code| code.to_string());
    if is_authentication_failure(status.as_deref(), &lower) {
        OutgoingError::Authentication
    } else if error.is_tls() {
        OutgoingError::Tls("SMTP TLS negotiation failed".into())
    } else if error.is_timeout() || lower.contains("timed out") || lower.contains("timeout") {
        OutgoingError::Network("SMTP connection timed out".into())
    } else if error.is_transient() {
        OutgoingError::Network(match status {
            Some(status) => format!("SMTP server temporarily unavailable ({status})"),
            None => "SMTP server temporarily unavailable".into(),
        })
    } else if error.is_permanent() || error.is_client() {
        OutgoingError::Rejected(match status {
            Some(status) => format!("SMTP server rejected the operation ({status})"),
            None => "SMTP server rejected the operation".into(),
        })
    } else {
        OutgoingError::Network("SMTP connection failed".into())
    }
}

fn classify_send_error(error: &SmtpError) -> OutgoingError {
    if error.is_permanent() || error.is_transient() || error.is_client() || error.is_tls() {
        return classify_smtp_error(error);
    }

    // lettre does not expose which SMTP command failed. Once send_raw starts, a
    // timeout/connection loss may happen after DATA was accepted, so treating it as a
    // normal retryable network failure could duplicate mail.
    OutgoingError::AmbiguousSend
}

fn is_authentication_failure(status: Option<&str>, lower_message: &str) -> bool {
    matches!(status, Some("530" | "534" | "535"))
        || lower_message.contains("auth")
        || lower_message.contains("credential")
}

#[cfg(test)]
mod tests {
    use super::{ensure_connection_confirmed, is_authentication_failure, SmtpOutgoingBackend};
    use crate::backends::outgoing::{OutgoingConfig, OutgoingError};

    fn config() -> OutgoingConfig {
        OutgoingConfig {
            protocol: "smtp".into(),
            host: "smtp.example.com".into(),
            port: 465,
            tls_mode: "implicit".into(),
            auth_method: "password".into(),
            username: "sender@example.com".into(),
        }
    }

    #[test]
    fn negative_noop_result_is_not_a_success() {
        assert!(matches!(
            ensure_connection_confirmed(false),
            Err(OutgoingError::Rejected(_))
        ));
        assert!(ensure_connection_confirmed(true).is_ok());
    }

    #[test]
    fn deterministic_configuration_errors_are_rejected_before_network_io() {
        let mut invalid = config();
        invalid.tls_mode = "none".into();
        assert!(matches!(
            SmtpOutgoingBackend::new(invalid).validate_configuration("secret"),
            Err(OutgoingError::Unsupported(_))
        ));

        assert!(matches!(
            SmtpOutgoingBackend::new(config()).validate_configuration(""),
            Err(OutgoingError::Authentication)
        ));

        let mut invalid_port = config();
        invalid_port.port = 0;
        assert!(matches!(
            SmtpOutgoingBackend::new(invalid_port).validate_configuration("secret"),
            Err(OutgoingError::Unsupported(_))
        ));
    }

    #[test]
    fn common_authentication_statuses_are_classified_without_message_matching() {
        for status in ["530", "534", "535"] {
            assert!(is_authentication_failure(Some(status), "permanent error"));
        }
        assert!(!is_authentication_failure(
            Some("550"),
            "mailbox unavailable"
        ));
    }
}
