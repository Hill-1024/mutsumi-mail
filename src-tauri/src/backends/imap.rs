use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use native_tls::TlsConnector;

use super::incoming::{IncomingConfig, IncomingError, IncomingMailBackend, ServerCapabilities};
use crate::domain::capabilities::ProviderCapabilities;

pub struct ImapIncomingBackend {
    pub config: IncomingConfig,
}

impl ImapIncomingBackend {
    pub fn new(config: IncomingConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl IncomingMailBackend for ImapIncomingBackend {
    fn backend_name(&self) -> &'static str {
        "imap"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            folders: true,
            flags: true,
            r#move: true,
            append: true,
            append_sent: true,
            partial_fetch: true,
            threading: true,
            smtp_utf8: false,
            ..Default::default()
        }
    }

    async fn test_connection(&self, secret: &str) -> Result<ServerCapabilities, IncomingError> {
        let config = self.config.clone();
        let secret = secret.to_owned();
        tokio::task::spawn_blocking(move || probe_imap(&config, &secret))
            .await
            .map_err(|error| IncomingError::Network(error.to_string()))?
    }
}

fn probe_imap(config: &IncomingConfig, secret: &str) -> Result<ServerCapabilities, IncomingError> {
    if config.host.trim().is_empty() || config.username.trim().is_empty() || secret.is_empty() {
        return Err(IncomingError::Protocol(
            "IMAP endpoint or credential is incomplete".into(),
        ));
    }
    if config
        .username
        .chars()
        .any(|character| character == '\r' || character == '\n')
        || secret
            .chars()
            .any(|character| character == '\r' || character == '\n')
    {
        return Err(IncomingError::Protocol(
            "IMAP credential contains an invalid line break".into(),
        ));
    }
    if config.protocol != "imap" {
        return Err(IncomingError::Unsupported(format!(
            "incoming protocol {} is not implemented by the IMAP backend",
            config.protocol
        )));
    }
    if config.auth_method != "password" {
        return Err(IncomingError::Unsupported(format!(
            "IMAP auth method {} needs its own token flow",
            config.auth_method
        )));
    }
    let address = format!("{}:{}", config.host, config.port);
    let socket = address
        .to_socket_addrs()
        .map_err(|error| IncomingError::Network(error.to_string()))?
        .next()
        .ok_or_else(|| IncomingError::Network("DNS returned no address".into()))?;
    let stream = TcpStream::connect_timeout(&socket, Duration::from_secs(10))
        .map_err(|error| IncomingError::Network(error.to_string()))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let connector = TlsConnector::new().map_err(|error| IncomingError::Tls(error.to_string()))?;
    if config.tls_mode == "implicit" {
        let mut tls = connector
            .connect(&config.host, stream)
            .map_err(|error| IncomingError::Tls(error.to_string()))?;
        let greeting = read_greeting(&mut tls)?;
        return authenticated_probe(&mut tls, &greeting, config, secret);
    }
    if config.tls_mode != "starttls" {
        return Err(IncomingError::Unsupported(format!(
            "IMAP TLS mode {} is not supported",
            config.tls_mode
        )));
    }
    let mut plain = stream;
    let greeting = read_greeting(&mut plain)?;
    write_imap_command(&mut plain, "a001 STARTTLS\r\n")?;
    let starttls_response = read_until_tag(&mut plain, "a001")?;
    if starttls_response.contains(" NO ") || starttls_response.contains(" BAD ") {
        return Err(IncomingError::Unsupported(
            "IMAP server rejected STARTTLS".into(),
        ));
    }
    let mut tls = connector
        .connect(&config.host, plain)
        .map_err(|error| IncomingError::Tls(error.to_string()))?;
    authenticated_probe(&mut tls, &greeting, config, secret)
}

fn read_greeting<S: Read>(stream: &mut S) -> Result<String, IncomingError> {
    let mut greeting = [0_u8; 4096];
    let read = stream
        .read(&mut greeting)
        .map_err(|error| IncomingError::Network(error.to_string()))?;
    let greeting_text = String::from_utf8_lossy(&greeting[..read]).to_string();
    if !greeting_text.starts_with('*') {
        return Err(IncomingError::Protocol("invalid IMAP greeting".into()));
    }
    Ok(greeting_text)
}

fn authenticated_probe<S: Read + Write>(
    stream: &mut S,
    greeting: &str,
    config: &IncomingConfig,
    secret: &str,
) -> Result<ServerCapabilities, IncomingError> {
    write_imap_command(stream, "a002 CAPABILITY\r\n")?;
    let capability_response = read_until_tag(stream, "a002")?;
    write_imap_command(
        stream,
        &format!(
            "a003 LOGIN {} {}\r\n",
            quote_atom(&config.username),
            quote_atom(secret)
        ),
    )?;
    let login_response = read_until_tag(stream, "a003")?;
    if login_response.contains(" NO ") || login_response.contains(" BAD ") {
        return Err(IncomingError::Authentication);
    }
    write_imap_command(stream, "a004 LOGOUT\r\n")?;
    Ok(ServerCapabilities {
        backend: "imap".into(),
        capabilities: parse_capabilities(&capability_response),
        greeting: Some(greeting.trim().to_string()),
    })
}

fn write_imap_command<S: Write>(stream: &mut S, command: &str) -> Result<(), IncomingError> {
    stream
        .write_all(command.as_bytes())
        .map_err(|error| IncomingError::Network(error.to_string()))
}

fn read_until_tag<S: Read>(stream: &mut S, tag: &str) -> Result<String, IncomingError> {
    let mut response = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let size = stream
            .read(&mut buffer)
            .map_err(|error| IncomingError::Network(error.to_string()))?;
        if size == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..size]);
        let text = String::from_utf8_lossy(&response);
        if text.lines().any(|line| line.starts_with(tag)) {
            return Ok(text.into_owned());
        }
        if response.len() > 128 * 1024 {
            return Err(IncomingError::Protocol(
                "IMAP response exceeded probe limit".into(),
            ));
        }
    }
    Ok(String::from_utf8_lossy(&response).into_owned())
}

fn quote_atom(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn parse_capabilities(response: &str) -> ProviderCapabilities {
    let uppercase = response.to_ascii_uppercase();
    ProviderCapabilities {
        folders: true,
        flags: true,
        r#move: uppercase.contains("MOVE"),
        append: uppercase.contains("APPEND"),
        append_sent: uppercase.contains("APPEND"),
        idle_push: uppercase.contains("IDLE"),
        keywords: uppercase.contains("KEYWORDS"),
        partial_fetch: true,
        threading: uppercase.contains("THREAD"),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_capabilities;

    #[test]
    fn maps_capability_tokens() {
        let capabilities = parse_capabilities("* CAPABILITY IMAP4rev1 IDLE MOVE CONDSTORE");
        assert!(capabilities.idle_push);
        assert!(capabilities.r#move);
    }
}
