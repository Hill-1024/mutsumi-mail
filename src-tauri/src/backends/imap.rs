use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use chrono::DateTime;
use imap_proto::{
    parser::parse_response, AttributeValue, Capability, MailboxDatum, MessageSection,
    NameAttribute, Response, ResponseCode, SectionPath, Status, UidSetMember,
};
use nom::Err as NomError;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, Instant};
use tokio_native_tls::{TlsConnector, TlsStream};

use super::incoming::{
    AppendMessageResult, IncomingConfig, IncomingError, IncomingMailBackend, IncomingMailbox,
    IncomingMailboxIndex, IncomingMailboxSnapshot, IncomingMessage, IncomingMessageFetch,
    RemoteMessageOperation, ServerCapabilities,
};
use crate::domain::capabilities::ProviderCapabilities;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const GREETING_LIMIT: usize = 8 * 1024;
const CONTROL_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const MAILBOX_INDEX_RESPONSE_LIMIT: usize = 32 * 1024 * 1024;
// Must stay at least as large as sync_service::MAILBOX_FETCH_LIMIT so the
// caller never mistakes an internally truncated page for a complete snapshot.
pub(crate) const MAX_FETCH_MESSAGES: u32 = 250;
const MAX_SYNC_BODY_BYTES: u64 = 25 * 1024 * 1024;
const MAX_BATCH_BODY_BYTES: u64 = 64 * 1024 * 1024;
const BATCH_BODY_RESPONSE_LIMIT: usize = MAX_BATCH_BODY_BYTES as usize + 2 * 1024 * 1024;
const MESSAGE_METADATA_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (DATE FROM TO CC BCC REPLY-TO SUBJECT MESSAGE-ID REFERENCES IN-REPLY-TO CONTENT-TYPE MIME-VERSION)])";

pub struct ImapIncomingBackend {
    pub config: IncomingConfig,
    // One backend instance represents one account operation. Keeping its authenticated
    // session alive avoids a burst of LOGIN commands while a sync walks every mailbox;
    // some providers throttle those reconnects and then leave later UID commands hanging.
    session: tokio::sync::Mutex<Option<CachedImapSession>>,
}

/// A dedicated authenticated IMAP session for server push. It is intentionally separate from
/// the bounded sync session so IDLE never blocks a foreground refresh or a queued mutation.
pub struct ImapIdleConnection {
    session: ImapSession<TlsStream<TcpStream>>,
}

impl ImapIncomingBackend {
    pub fn new(config: IncomingConfig) -> Self {
        Self {
            config,
            session: tokio::sync::Mutex::new(None),
        }
    }

    async fn authenticated_session(
        &self,
        secret: &str,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<CachedImapSession>>, IncomingError> {
        let mut cached = self.session.lock().await;
        if cached
            .as_ref()
            .is_none_or(|existing| existing.secret != secret)
        {
            cached.take();
            let (session, _) = authenticate(&self.config, secret).await?;
            *cached = Some(CachedImapSession {
                secret: secret.to_owned(),
                session,
            });
        }
        Ok(cached)
    }

    /// Opens one read-only INBOX selection and verifies that the server actually supports IDLE.
    /// `INBOX` is the one mailbox every IMAP server must expose; a notification there triggers an
    /// account-wide incremental sync, which also reconciles the remaining enabled folders.
    pub async fn open_idle_connection(
        &self,
        secret: &str,
    ) -> Result<ImapIdleConnection, IncomingError> {
        let (mut session, _) = authenticate(&self.config, secret).await?;
        let capabilities = read_capabilities(&mut session).await?;
        if !capabilities
            .iter()
            .any(|capability| capability.eq_ignore_ascii_case("IDLE"))
        {
            return Err(IncomingError::Unsupported(
                "IMAP server does not advertise IDLE".into(),
            ));
        }
        select_mailbox(&mut session, "INBOX").await?;
        Ok(ImapIdleConnection { session })
    }
}

impl ImapIdleConnection {
    /// Blocks until the selected mailbox changes or the caller's renewal deadline is reached.
    /// A `false` result is only an IDLE renewal; it never causes an unnecessary message fetch.
    pub async fn wait_for_change(&mut self, max_wait: Duration) -> Result<bool, IncomingError> {
        self.session.idle_until_change(max_wait).await
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
            // A caller still has to discover a selectable \Sent mailbox and
            // decide whether its provider already auto-saves sent messages.
            append_sent: false,
            partial_fetch: true,
            threading: true,
            smtp_utf8: false,
            ..Default::default()
        }
    }

    async fn test_connection(&self, secret: &str) -> Result<ServerCapabilities, IncomingError> {
        let (mut session, greeting) = authenticate(&self.config, secret).await?;
        let capability_names = read_capabilities(&mut session).await?;
        let _ = session.logout().await;
        Ok(ServerCapabilities {
            backend: "imap".into(),
            capabilities: capabilities_from_names(&capability_names),
            greeting: Some(greeting),
        })
    }

    async fn list_remote_mailboxes(
        &self,
        secret: &str,
    ) -> Result<Vec<IncomingMailbox>, IncomingError> {
        let mut cached = self.authenticated_session(secret).await?;
        let result = match cached.as_mut() {
            Some(cached) => list_mailboxes_on_session(&mut cached.session).await,
            None => Err(IncomingError::Protocol(
                "IMAP session cache was unexpectedly empty".into(),
            )),
        };
        if result.is_err() {
            cached.take();
        }
        result
    }

    async fn fetch_remote_messages(
        &self,
        secret: &str,
        mailbox: &str,
        since_uid: Option<u32>,
        limit: u32,
    ) -> Result<IncomingMailboxSnapshot, IncomingError> {
        let mut cached = self.authenticated_session(secret).await?;
        let result = match cached.as_mut() {
            Some(cached) => {
                fetch_messages_on_session(&mut cached.session, mailbox, since_uid, limit).await
            }
            None => Err(IncomingError::Protocol(
                "IMAP session cache was unexpectedly empty".into(),
            )),
        };
        if result.is_err() {
            cached.take();
        }
        result
    }

    async fn fetch_remote_messages_before(
        &self,
        secret: &str,
        mailbox: &str,
        before_uid: u32,
        limit: u32,
    ) -> Result<IncomingMailboxSnapshot, IncomingError> {
        let mut cached = self.authenticated_session(secret).await?;
        let result = match cached.as_mut() {
            Some(cached) => {
                fetch_messages_before_on_session(&mut cached.session, mailbox, before_uid, limit)
                    .await
            }
            None => Err(IncomingError::Protocol(
                "IMAP session cache was unexpectedly empty".into(),
            )),
        };
        if result.is_err() {
            cached.take();
        }
        result
    }

    async fn fetch_remote_message(
        &self,
        secret: &str,
        mailbox: &str,
        uid: u32,
    ) -> Result<Option<IncomingMessageFetch>, IncomingError> {
        let mut cached = self.authenticated_session(secret).await?;
        let result = match cached.as_mut() {
            Some(cached) => fetch_message_on_session(&mut cached.session, mailbox, uid).await,
            None => Err(IncomingError::Protocol(
                "IMAP session cache was unexpectedly empty".into(),
            )),
        };
        if result.is_err() {
            cached.take();
        }
        result
    }

    async fn fetch_remote_mailbox_index(
        &self,
        secret: &str,
        mailbox: &str,
    ) -> Result<IncomingMailboxIndex, IncomingError> {
        let mut cached = self.authenticated_session(secret).await?;
        let result = match cached.as_mut() {
            Some(cached) => fetch_mailbox_index_on_session(&mut cached.session, mailbox).await,
            None => Err(IncomingError::Protocol(
                "IMAP session cache was unexpectedly empty".into(),
            )),
        };
        if result.is_err() {
            cached.take();
        }
        result
    }

    async fn apply_remote_operation(
        &self,
        secret: &str,
        operation: &RemoteMessageOperation,
    ) -> Result<(), IncomingError> {
        let mut cached = self.authenticated_session(secret).await?;
        let result = match cached.as_mut() {
            Some(cached) => match read_capabilities(&mut cached.session).await {
                Ok(capability_names) => {
                    apply_remote_operation_on_session(
                        &mut cached.session,
                        &capability_names,
                        operation,
                    )
                    .await
                }
                Err(error) => Err(error),
            },
            None => Err(IncomingError::Protocol(
                "IMAP session cache was unexpectedly empty".into(),
            )),
        };
        if result.is_err() {
            cached.take();
        }
        result
    }

    async fn append_message(
        &self,
        secret: &str,
        mailbox: &str,
        raw_rfc822: &[u8],
        mark_seen: bool,
    ) -> Result<AppendMessageResult, IncomingError> {
        let mut cached = self.authenticated_session(secret).await?;
        let result = match cached.as_mut() {
            Some(cached) => {
                append_message_on_session(&mut cached.session, mailbox, raw_rfc822, mark_seen).await
            }
            None => Err(IncomingError::Protocol(
                "IMAP session cache was unexpectedly empty".into(),
            )),
        };
        if result.is_err() {
            cached.take();
        }
        result
    }
}

struct CachedImapSession {
    // Retained only for the lifetime of this backend so a caller cannot accidentally reuse an
    // authenticated connection after changing credentials. This type has no Debug output.
    secret: String,
    session: ImapSession<TlsStream<TcpStream>>,
}

struct CommandResult {
    responses: Vec<Response<'static>>,
    completion_code: Option<ResponseCode<'static>>,
}

struct ImapSession<S> {
    stream: S,
    read_buffer: Vec<u8>,
    next_tag: u32,
}

impl<S> ImapSession<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    fn new(stream: S) -> Self {
        Self {
            stream,
            read_buffer: Vec::with_capacity(8 * 1024),
            next_tag: 1,
        }
    }

    fn next_command_tag(&mut self) -> Result<String, IncomingError> {
        let tag = format!("A{:04}", self.next_tag);
        self.next_tag = self
            .next_tag
            .checked_add(1)
            .ok_or_else(|| IncomingError::Protocol("IMAP command tag exhausted".into()))?;
        Ok(tag)
    }

    async fn execute(
        &mut self,
        command: &str,
        response_limit: usize,
        authentication_command: bool,
    ) -> Result<CommandResult, IncomingError> {
        let tag = self.next_command_tag()?;
        let wire = format!("{tag} {command}\r\n");
        timeout(COMMAND_TIMEOUT, async {
            self.stream.write_all(wire.as_bytes()).await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| IncomingError::Network(format!("IMAP {} timed out", command_name(command))))?
        .map_err(|error| IncomingError::Network(error.to_string()))?;

        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut responses = Vec::new();
        let mut response_bytes = 0_usize;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IncomingError::Network(format!(
                    "IMAP {} timed out",
                    command_name(command)
                )));
            }
            let remaining_bytes = response_limit.checked_sub(response_bytes).ok_or_else(|| {
                IncomingError::Protocol("IMAP response exceeded the allowed size".into())
            })?;
            if remaining_bytes == 0 {
                return Err(IncomingError::Protocol(
                    "IMAP response exceeded the allowed size".into(),
                ));
            }
            let (response, consumed) = timeout(remaining, self.read_response(remaining_bytes))
                .await
                .map_err(|_| {
                    IncomingError::Network(format!("IMAP {} timed out", command_name(command)))
                })??;
            response_bytes = response_bytes.checked_add(consumed).ok_or_else(|| {
                IncomingError::Protocol("IMAP response exceeded the allowed size".into())
            })?;
            match response {
                Response::Done {
                    tag: response_tag,
                    status,
                    code,
                    information,
                } => {
                    if response_tag.0 != tag {
                        return Err(IncomingError::Protocol(format!(
                            "IMAP returned unexpected tagged response {} while waiting for {tag}",
                            response_tag.0
                        )));
                    }
                    return match status {
                        Status::Ok => Ok(CommandResult {
                            responses,
                            completion_code: code,
                        }),
                        Status::No | Status::Bad if authentication_command => {
                            Err(IncomingError::Authentication)
                        }
                        Status::No | Status::Bad => Err(IncomingError::Protocol(
                            information
                                .map(|value| value.into_owned())
                                .unwrap_or_else(|| {
                                    format!("IMAP {} was rejected", command_name(command))
                                }),
                        )),
                        Status::Bye | Status::PreAuth => Err(IncomingError::Protocol(format!(
                            "unexpected IMAP completion status for {}",
                            command_name(command)
                        ))),
                    };
                }
                Response::Data {
                    status: Status::Bye,
                    information,
                    ..
                } => {
                    return Err(IncomingError::Protocol(
                        information
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|| "IMAP server closed the session".into()),
                    ));
                }
                Response::Continue { .. } => {
                    return Err(IncomingError::Protocol(format!(
                        "unexpected IMAP continuation during {}",
                        command_name(command)
                    )));
                }
                other => responses.push(other),
            }
        }
    }

    /// Implements the RFC 2177 IDLE handshake. The caller gives IDLE a finite renewal deadline
    /// (typically below the server's 29-minute limit); renewal is connection maintenance only,
    /// while a matching unsolicited mailbox response returns `true` and triggers a delta sync.
    async fn idle_until_change(&mut self, max_wait: Duration) -> Result<bool, IncomingError> {
        let tag = self.next_command_tag()?;
        let wire = format!("{tag} IDLE\r\n");
        timeout(COMMAND_TIMEOUT, async {
            self.stream.write_all(wire.as_bytes()).await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| IncomingError::Network("IMAP IDLE command timed out".into()))?
        .map_err(|error| IncomingError::Network(error.to_string()))?;

        let continuation_deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut changed = false;
        loop {
            let remaining = continuation_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IncomingError::Network(
                    "IMAP IDLE continuation timed out".into(),
                ));
            }
            let (response, _) = timeout(remaining, self.read_response(CONTROL_RESPONSE_LIMIT))
                .await
                .map_err(|_| IncomingError::Network("IMAP IDLE continuation timed out".into()))??;
            match response {
                Response::Continue { .. } => break,
                Response::Done {
                    tag: response_tag,
                    status,
                    information,
                    ..
                } => {
                    return idle_completion(
                        &tag,
                        response_tag.0.as_str(),
                        status,
                        information,
                        changed,
                    )
                }
                Response::Data {
                    status: Status::Bye,
                    information,
                    ..
                } => {
                    return Err(IncomingError::Network(
                        information
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|| "IMAP server closed the IDLE session".into()),
                    ));
                }
                response => changed |= response_indicates_mailbox_change(&response),
            }
        }

        if changed {
            return self.finish_idle(&tag, true).await;
        }

        let idle_deadline = Instant::now() + max_wait;
        loop {
            let remaining = idle_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return self.finish_idle(&tag, false).await;
            }
            let (response, _) = timeout(remaining, self.read_response(CONTROL_RESPONSE_LIMIT))
                .await
                .map_err(|_| IncomingError::Network("IMAP IDLE response timed out".into()))??;
            match response {
                Response::Done {
                    tag: response_tag,
                    status,
                    information,
                    ..
                } => {
                    return idle_completion(
                        &tag,
                        response_tag.0.as_str(),
                        status,
                        information,
                        changed,
                    )
                }
                Response::Data {
                    status: Status::Bye,
                    information,
                    ..
                } => {
                    return Err(IncomingError::Network(
                        information
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|| "IMAP server closed the IDLE session".into()),
                    ));
                }
                Response::Continue { .. } => {
                    return Err(IncomingError::Protocol(
                        "unexpected second IMAP IDLE continuation".into(),
                    ));
                }
                response if response_indicates_mailbox_change(&response) => {
                    changed = true;
                    return self.finish_idle(&tag, changed).await;
                }
                // Providers commonly send an unsolicited `* OK` keepalive while idling. It is
                // not a mailbox mutation, so keep the same socket blocked instead of syncing.
                _ => {}
            }
        }
    }

    async fn finish_idle(&mut self, tag: &str, mut changed: bool) -> Result<bool, IncomingError> {
        timeout(COMMAND_TIMEOUT, async {
            self.stream.write_all(b"DONE\r\n").await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| IncomingError::Network("IMAP IDLE termination timed out".into()))?
        .map_err(|error| IncomingError::Network(error.to_string()))?;

        let deadline = Instant::now() + COMMAND_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IncomingError::Network(
                    "IMAP IDLE termination timed out".into(),
                ));
            }
            let (response, _) = timeout(remaining, self.read_response(CONTROL_RESPONSE_LIMIT))
                .await
                .map_err(|_| IncomingError::Network("IMAP IDLE termination timed out".into()))??;
            match response {
                Response::Done {
                    tag: response_tag,
                    status,
                    information,
                    ..
                } => {
                    return idle_completion(
                        tag,
                        response_tag.0.as_str(),
                        status,
                        information,
                        changed,
                    )
                }
                Response::Data {
                    status: Status::Bye,
                    information,
                    ..
                } => {
                    return Err(IncomingError::Network(
                        information
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|| "IMAP server closed the IDLE session".into()),
                    ));
                }
                Response::Continue { .. } => {
                    return Err(IncomingError::Protocol(
                        "unexpected IMAP continuation while ending IDLE".into(),
                    ));
                }
                response => changed |= response_indicates_mailbox_change(&response),
            }
        }
    }

    /// Executes APPEND using a synchronizing literal. The message slice is
    /// written byte-for-byte only after the server sends a `+` continuation;
    /// the trailing CRLF terminates the IMAP command and is not part of the
    /// declared literal length.
    async fn append_literal(
        &mut self,
        mailbox: &str,
        raw_rfc822: &[u8],
        mark_seen: bool,
    ) -> Result<CommandResult, IncomingError> {
        let tag = self.next_command_tag()?;
        let flags = if mark_seen { " (\\Seen)" } else { "" };
        let prefix = format!(
            "{tag} APPEND {}{flags} {{{}}}\r\n",
            quote_imap(mailbox)?,
            raw_rfc822.len()
        );
        timeout(COMMAND_TIMEOUT, async {
            self.stream.write_all(prefix.as_bytes()).await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| IncomingError::Network("IMAP APPEND continuation timed out".into()))?
        .map_err(|error| IncomingError::Network(error.to_string()))?;

        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut responses = Vec::new();
        let mut response_bytes = 0_usize;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IncomingError::Network(
                    "IMAP APPEND continuation timed out".into(),
                ));
            }
            let remaining_bytes = CONTROL_RESPONSE_LIMIT
                .checked_sub(response_bytes)
                .ok_or_else(|| {
                    IncomingError::Protocol("IMAP response exceeded the allowed size".into())
                })?;
            if remaining_bytes == 0 {
                return Err(IncomingError::Protocol(
                    "IMAP response exceeded the allowed size".into(),
                ));
            }
            let (response, consumed) = timeout(remaining, self.read_response(remaining_bytes))
                .await
                .map_err(|_| {
                    IncomingError::Network("IMAP APPEND continuation timed out".into())
                })??;
            response_bytes = response_bytes.checked_add(consumed).ok_or_else(|| {
                IncomingError::Protocol("IMAP response exceeded the allowed size".into())
            })?;
            match response {
                Response::Continue { .. } => break,
                Response::Done {
                    tag: response_tag,
                    status,
                    information,
                    ..
                } => {
                    if response_tag.0 != tag {
                        return Err(IncomingError::Protocol(format!(
                            "IMAP returned unexpected tagged response {} while waiting for {tag}",
                            response_tag.0
                        )));
                    }
                    return Err(IncomingError::Protocol(match status {
                        Status::Ok => {
                            "IMAP APPEND completed without requesting the declared literal".into()
                        }
                        _ => information
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|| "IMAP APPEND was rejected".into()),
                    }));
                }
                Response::Data {
                    status: Status::Bye,
                    information,
                    ..
                } => {
                    return Err(IncomingError::Protocol(
                        information
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|| "IMAP server closed the APPEND session".into()),
                    ));
                }
                other => responses.push(other),
            }
        }

        timeout(COMMAND_TIMEOUT, async {
            self.stream.write_all(raw_rfc822).await?;
            self.stream.write_all(b"\r\n").await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| IncomingError::Network("IMAP APPEND upload timed out".into()))?
        .map_err(|error| IncomingError::Network(error.to_string()))?;

        let completion_deadline = Instant::now() + COMMAND_TIMEOUT;
        loop {
            let remaining = completion_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IncomingError::Network(
                    "IMAP APPEND completion timed out".into(),
                ));
            }
            let remaining_bytes = CONTROL_RESPONSE_LIMIT
                .checked_sub(response_bytes)
                .ok_or_else(|| {
                    IncomingError::Protocol("IMAP response exceeded the allowed size".into())
                })?;
            if remaining_bytes == 0 {
                return Err(IncomingError::Protocol(
                    "IMAP response exceeded the allowed size".into(),
                ));
            }
            let (response, consumed) = timeout(remaining, self.read_response(remaining_bytes))
                .await
                .map_err(|_| IncomingError::Network("IMAP APPEND completion timed out".into()))??;
            response_bytes = response_bytes.checked_add(consumed).ok_or_else(|| {
                IncomingError::Protocol("IMAP response exceeded the allowed size".into())
            })?;
            match response {
                Response::Done {
                    tag: response_tag,
                    status,
                    code,
                    information,
                } => {
                    if response_tag.0 != tag {
                        return Err(IncomingError::Protocol(format!(
                            "IMAP returned unexpected tagged response {} while waiting for {tag}",
                            response_tag.0
                        )));
                    }
                    return match status {
                        Status::Ok => Ok(CommandResult {
                            responses,
                            completion_code: code,
                        }),
                        Status::No | Status::Bad => Err(IncomingError::Protocol(
                            information
                                .map(|value| value.into_owned())
                                .unwrap_or_else(|| "IMAP APPEND was rejected".into()),
                        )),
                        Status::Bye | Status::PreAuth => Err(IncomingError::Protocol(
                            "unexpected IMAP APPEND completion status".into(),
                        )),
                    };
                }
                Response::Data {
                    status: Status::Bye,
                    information,
                    ..
                } => {
                    return Err(IncomingError::Protocol(
                        information
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|| "IMAP server closed the APPEND session".into()),
                    ));
                }
                Response::Continue { .. } => {
                    return Err(IncomingError::Protocol(
                        "unexpected second IMAP APPEND continuation".into(),
                    ));
                }
                other => responses.push(other),
            }
        }
    }

    async fn read_response(
        &mut self,
        response_limit: usize,
    ) -> Result<(Response<'static>, usize), IncomingError> {
        loop {
            match parse_response(&self.read_buffer) {
                Ok((remaining, response)) => {
                    let consumed = self.read_buffer.len() - remaining.len();
                    if consumed == 0 {
                        return Err(IncomingError::Protocol(
                            "IMAP parser made no progress".into(),
                        ));
                    }
                    if consumed > response_limit {
                        return Err(IncomingError::Protocol(
                            "IMAP response exceeded the allowed size".into(),
                        ));
                    }
                    let response = response.into_owned();
                    self.read_buffer.drain(..consumed);
                    return Ok((response, consumed));
                }
                Err(NomError::Incomplete(_)) => {}
                Err(NomError::Error(_)) | Err(NomError::Failure(_)) => {
                    return Err(IncomingError::Protocol(
                        "IMAP server returned a malformed response".into(),
                    ));
                }
            }

            if self.read_buffer.len() >= response_limit {
                return Err(IncomingError::Protocol(
                    "IMAP response exceeded the allowed size".into(),
                ));
            }
            let mut chunk = [0_u8; 8 * 1024];
            let allowed = chunk
                .len()
                .min(response_limit.saturating_sub(self.read_buffer.len()));
            let read = self
                .stream
                .read(&mut chunk[..allowed])
                .await
                .map_err(|error| IncomingError::Network(error.to_string()))?;
            if read == 0 {
                return Err(IncomingError::Protocol(
                    "IMAP connection ended before the tagged command response".into(),
                ));
            }
            self.read_buffer.extend_from_slice(&chunk[..read]);
        }
    }

    async fn logout(&mut self) -> Result<(), IncomingError> {
        self.execute("LOGOUT", CONTROL_RESPONSE_LIMIT, false)
            .await
            .map(|_| ())
    }

    fn into_inner(self) -> Result<S, IncomingError> {
        if self.read_buffer.is_empty() {
            Ok(self.stream)
        } else {
            Err(IncomingError::Protocol(
                "unexpected plaintext bytes after STARTTLS response".into(),
            ))
        }
    }
}

fn idle_completion(
    expected_tag: &str,
    response_tag: &str,
    status: Status,
    information: Option<std::borrow::Cow<'static, str>>,
    changed: bool,
) -> Result<bool, IncomingError> {
    if response_tag != expected_tag {
        return Err(IncomingError::Protocol(format!(
            "IMAP returned unexpected tagged response {response_tag} while waiting for {expected_tag}"
        )));
    }
    match status {
        Status::Ok => Ok(changed),
        Status::No | Status::Bad => Err(IncomingError::Protocol(
            information
                .map(|value| value.into_owned())
                .unwrap_or_else(|| "IMAP IDLE was rejected".into()),
        )),
        Status::Bye | Status::PreAuth => Err(IncomingError::Network(
            "IMAP server ended the IDLE session".into(),
        )),
    }
}

fn response_indicates_mailbox_change(response: &Response<'_>) -> bool {
    matches!(
        response,
        Response::Expunge(_)
            | Response::Vanished { .. }
            | Response::Fetch(_, _)
            | Response::MailboxData(MailboxDatum::Exists(_) | MailboxDatum::Recent(_))
            | Response::Data {
                code: Some(
                    ResponseCode::UidNext(_)
                        | ResponseCode::UidValidity(_)
                        | ResponseCode::Unseen(_)
                ),
                ..
            }
    )
}

fn command_name(command: &str) -> String {
    let mut parts = command.split_ascii_whitespace();
    match (parts.next(), parts.next()) {
        (Some("UID"), Some(operation)) => format!("UID {operation}"),
        (Some(operation), _) => operation.to_owned(),
        (None, _) => "command".into(),
    }
}

async fn authenticate(
    config: &IncomingConfig,
    secret: &str,
) -> Result<(ImapSession<TlsStream<TcpStream>>, String), IncomingError> {
    validate_config(config, secret)?;
    let (mut session, greeting) = connect(config).await?;
    let login = format!(
        "LOGIN {} {}",
        quote_imap(&config.username)?,
        quote_imap(secret)?
    );
    session
        .execute(&login, CONTROL_RESPONSE_LIMIT, true)
        .await?;
    Ok((session, greeting))
}

fn validate_config(config: &IncomingConfig, secret: &str) -> Result<(), IncomingError> {
    if config.host.trim().is_empty() || config.username.trim().is_empty() || secret.is_empty() {
        return Err(IncomingError::Protocol(
            "IMAP endpoint or credential is incomplete".into(),
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
    if config.host.chars().any(char::is_control)
        || [config.username.as_str(), secret].into_iter().any(|value| {
            value
                .as_bytes()
                .iter()
                .any(|byte| matches!(byte, b'\r' | b'\n' | b'\0'))
        })
    {
        return Err(IncomingError::Protocol(
            "IMAP endpoint or credential contains an invalid control character".into(),
        ));
    }
    Ok(())
}

fn quote_imap(value: &str) -> Result<String, IncomingError> {
    if value
        .as_bytes()
        .iter()
        .any(|byte| matches!(byte, b'\r' | b'\n' | b'\0'))
    {
        return Err(IncomingError::Protocol(
            "IMAP value contains an invalid control character".into(),
        ));
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

async fn connect(
    config: &IncomingConfig,
) -> Result<(ImapSession<TlsStream<TcpStream>>, String), IncomingError> {
    let tcp = timeout(
        CONNECT_TIMEOUT,
        TcpStream::connect((config.host.as_str(), config.port)),
    )
    .await
    .map_err(|_| IncomingError::Network("IMAP TCP connection timed out".into()))?
    .map_err(|error| IncomingError::Network(error.to_string()))?;
    let connector = native_tls::TlsConnector::new()
        .map(TlsConnector::from)
        .map_err(|error| IncomingError::Tls(error.to_string()))?;

    match config.tls_mode.as_str() {
        "implicit" => {
            let mut tls = timeout(CONNECT_TIMEOUT, connector.connect(&config.host, tcp))
                .await
                .map_err(|_| IncomingError::Tls("IMAP TLS handshake timed out".into()))?
                .map_err(|error| IncomingError::Tls(error.to_string()))?;
            let greeting = read_greeting(&mut tls).await?;
            Ok((ImapSession::new(tls), greeting))
        }
        "starttls" => {
            let mut tcp = tcp;
            let greeting = read_greeting(&mut tcp).await?;
            let mut plain = ImapSession::new(tcp);
            plain
                .execute("STARTTLS", CONTROL_RESPONSE_LIMIT, false)
                .await?;
            let tcp = plain.into_inner()?;
            let tls = timeout(CONNECT_TIMEOUT, connector.connect(&config.host, tcp))
                .await
                .map_err(|_| IncomingError::Tls("IMAP TLS handshake timed out".into()))?
                .map_err(|error| IncomingError::Tls(error.to_string()))?;
            Ok((ImapSession::new(tls), greeting))
        }
        mode => Err(IncomingError::Unsupported(format!(
            "IMAP TLS mode {mode} is not supported"
        ))),
    }
}

async fn read_greeting<S>(stream: &mut S) -> Result<String, IncomingError>
where
    S: AsyncRead + Unpin,
{
    let line = timeout(COMMAND_TIMEOUT, read_crlf_line(stream))
        .await
        .map_err(|_| IncomingError::Network("IMAP greeting timed out".into()))??;
    parse_greeting_line(&line)
}

async fn read_crlf_line<S>(stream: &mut S) -> Result<Vec<u8>, IncomingError>
where
    S: AsyncRead + Unpin,
{
    let mut line = Vec::with_capacity(128);
    loop {
        let mut byte = [0_u8; 1];
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|error| IncomingError::Network(error.to_string()))?;
        if read == 0 {
            return Err(IncomingError::Protocol(
                "IMAP connection ended before a complete greeting".into(),
            ));
        }
        line.push(byte[0]);
        if line.len() > GREETING_LIMIT {
            return Err(IncomingError::Protocol(
                "IMAP greeting exceeded the allowed size".into(),
            ));
        }
        if line.ends_with(b"\r\n") {
            return Ok(line);
        }
    }
}

fn parse_greeting_line(line: &[u8]) -> Result<String, IncomingError> {
    if !line.ends_with(b"\r\n") {
        return Err(IncomingError::Protocol(
            "IMAP greeting did not end with CRLF".into(),
        ));
    }
    let greeting = std::str::from_utf8(&line[..line.len() - 2])
        .map_err(|_| IncomingError::Protocol("IMAP greeting was not valid UTF-8".into()))?;
    let status = greeting
        .split_ascii_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();
    match status.as_str() {
        "* OK" => Ok(greeting.to_owned()),
        "* BYE" => Err(IncomingError::Protocol(format!(
            "IMAP server rejected the connection: {greeting}"
        ))),
        "* PREAUTH" => Err(IncomingError::Unsupported(
            "IMAP PREAUTH sessions are not supported for credential validation".into(),
        )),
        _ => Err(IncomingError::Protocol("invalid IMAP greeting".into())),
    }
}

async fn read_capabilities<S>(session: &mut ImapSession<S>) -> Result<Vec<String>, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let result = session
        .execute("CAPABILITY", CONTROL_RESPONSE_LIMIT, false)
        .await?;
    let mut names = Vec::new();
    for response in result.responses {
        if let Response::Capabilities(capabilities) = response {
            names.extend(capabilities.iter().map(capability_name));
        }
    }
    if let Some(ResponseCode::Capabilities(capabilities)) = result.completion_code {
        names.extend(capabilities.iter().map(capability_name));
    }
    names.sort_unstable();
    names.dedup();
    if names
        .iter()
        .any(|name| name.eq_ignore_ascii_case("IMAP4REV1") || name == "IMAP4REV2")
    {
        Ok(names)
    } else {
        Err(IncomingError::Protocol(
            "IMAP CAPABILITY omitted IMAP4rev1/IMAP4rev2".into(),
        ))
    }
}

async fn list_mailboxes_on_session<S>(
    session: &mut ImapSession<S>,
) -> Result<Vec<IncomingMailbox>, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let result = session
        .execute("LIST \"\" \"*\"", CONTROL_RESPONSE_LIMIT, false)
        .await?;
    let mut mailboxes = result
        .responses
        .into_iter()
        .filter_map(|response| match response {
            Response::MailboxData(MailboxDatum::List {
                name_attributes,
                delimiter,
                name,
            }) => Some(mailbox_from_parts(name_attributes, delimiter, name)),
            _ => None,
        })
        .collect::<Vec<_>>();
    mailboxes.sort_by_key(|mailbox| {
        (
            mailbox.special_role.as_deref() != Some("inbox"),
            mailbox.display_name.to_lowercase(),
        )
    });
    Ok(mailboxes)
}

fn mailbox_from_parts(
    name_attributes: Vec<NameAttribute<'static>>,
    delimiter: Option<std::borrow::Cow<'static, str>>,
    name: std::borrow::Cow<'static, str>,
) -> IncomingMailbox {
    let special_role = if name.eq_ignore_ascii_case("INBOX") {
        Some("inbox".to_owned())
    } else {
        name_attributes.iter().find_map(special_role)
    };
    IncomingMailbox {
        remote_id: name.to_string(),
        display_name: decode_modified_utf7(&name),
        delimiter: delimiter.map(|value| value.into_owned()),
        selectable: !name_attributes
            .iter()
            .any(|attribute| matches!(attribute, NameAttribute::NoSelect)),
        attributes: name_attributes.iter().map(name_attribute).collect(),
        special_role,
    }
}

struct SelectedMailbox {
    uid_validity: Option<u32>,
    total_count: u32,
}

async fn select_mailbox<S>(
    session: &mut ImapSession<S>,
    mailbox: &str,
) -> Result<SelectedMailbox, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    select_mailbox_with_mode(session, mailbox, true).await
}

async fn select_mailbox_read_write<S>(
    session: &mut ImapSession<S>,
    mailbox: &str,
) -> Result<SelectedMailbox, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    select_mailbox_with_mode(session, mailbox, false).await
}

async fn select_mailbox_with_mode<S>(
    session: &mut ImapSession<S>,
    mailbox: &str,
    read_only: bool,
) -> Result<SelectedMailbox, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let command = if read_only { "EXAMINE" } else { "SELECT" };
    let result = session
        .execute(
            &format!("{command} {}", quote_imap(mailbox)?),
            CONTROL_RESPONSE_LIMIT,
            false,
        )
        .await?;
    let mut selected = SelectedMailbox {
        uid_validity: None,
        total_count: 0,
    };
    for response in result.responses {
        match response {
            Response::MailboxData(MailboxDatum::Exists(count)) => selected.total_count = count,
            Response::Data {
                code: Some(ResponseCode::UidValidity(value)),
                ..
            } => selected.uid_validity = Some(value),
            _ => {}
        }
    }
    if let Some(ResponseCode::UidValidity(value)) = result.completion_code {
        selected.uid_validity = Some(value);
    }
    Ok(selected)
}

async fn search_uids<S>(
    session: &mut ImapSession<S>,
    criteria: &str,
) -> Result<Vec<u32>, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    search_uids_with_limit(session, criteria, CONTROL_RESPONSE_LIMIT).await
}

async fn search_uids_with_limit<S>(
    session: &mut ImapSession<S>,
    criteria: &str,
    response_limit: usize,
) -> Result<Vec<u32>, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let result = session
        .execute(&format!("UID SEARCH {criteria}"), response_limit, false)
        .await?;
    let mut saw_search_response = false;
    let mut uids = Vec::new();
    for response in result.responses {
        if let Response::MailboxData(MailboxDatum::Search(values)) = response {
            saw_search_response = true;
            uids.extend(values);
        }
    }
    if !saw_search_response {
        return Err(IncomingError::Protocol(
            "IMAP UID SEARCH completed without a SEARCH response".into(),
        ));
    }
    if uids.contains(&0) {
        return Err(IncomingError::Protocol(
            "IMAP UID SEARCH returned the invalid UID 0".into(),
        ));
    }
    uids.sort_unstable();
    uids.dedup();
    Ok(uids)
}

async fn fetch_mailbox_index_on_session<S>(
    session: &mut ImapSession<S>,
    mailbox: &str,
) -> Result<IncomingMailboxIndex, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let selected = select_mailbox(session, mailbox).await?;
    if selected.total_count == 0 {
        return Ok(IncomingMailboxIndex {
            remote_id: mailbox.to_owned(),
            uid_validity: selected.uid_validity,
            total_count: 0,
            all_uids: Vec::new(),
            unseen_uids: Vec::new(),
            flagged_uids: Vec::new(),
        });
    }
    // Fetch UID and flags together so one tagged response is the authority for both
    // membership and flag state. Use message sequence numbers for the full selected mailbox;
    // the UID is still explicitly returned, while avoiding provider-specific UID FETCH stalls.
    // Four separate SEARCH commands created race windows and triggered throttling when a sync
    // walked several folders in quick succession.
    let result = session
        .execute("FETCH 1:* (UID FLAGS)", MAILBOX_INDEX_RESPONSE_LIMIT, false)
        .await?;
    let mut entries = Vec::new();
    for response in result.responses {
        let Response::Fetch(_, attributes) = response else {
            continue;
        };
        let mut uid = None;
        let mut flags = Vec::new();
        for attribute in attributes {
            match attribute {
                AttributeValue::Uid(value) => uid = Some(value),
                AttributeValue::Flags(values) => {
                    flags = values.into_iter().map(|value| value.into_owned()).collect()
                }
                _ => {}
            }
        }
        let uid = uid.ok_or_else(|| {
            IncomingError::Protocol("IMAP mailbox index response omitted UID".into())
        })?;
        if uid == 0 {
            return Err(IncomingError::Protocol(
                "IMAP mailbox index returned the invalid UID 0".into(),
            ));
        }
        entries.push((uid, flags));
    }
    entries.sort_unstable_by_key(|(uid, _)| *uid);
    if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(IncomingError::Protocol(
            "IMAP mailbox index returned a duplicate UID".into(),
        ));
    }
    let all_uids = entries.iter().map(|(uid, _)| *uid).collect::<Vec<_>>();
    let unseen_uids = entries
        .iter()
        .filter(|(_, flags)| !flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen")))
        .map(|(uid, _)| *uid)
        .collect::<Vec<_>>();
    let flagged_uids = entries
        .iter()
        .filter(|(_, flags)| {
            flags
                .iter()
                .any(|flag| flag.eq_ignore_ascii_case("\\Flagged"))
        })
        .map(|(uid, _)| *uid)
        .collect::<Vec<_>>();
    let total_count = u32::try_from(all_uids.len()).map_err(|_| {
        IncomingError::Protocol("IMAP mailbox UID index exceeded the supported count".into())
    })?;

    Ok(IncomingMailboxIndex {
        remote_id: mailbox.to_owned(),
        uid_validity: selected.uid_validity,
        total_count,
        all_uids,
        unseen_uids,
        flagged_uids,
    })
}

async fn fetch_messages_on_session<S>(
    session: &mut ImapSession<S>,
    mailbox: &str,
    since_uid: Option<u32>,
    limit: u32,
) -> Result<IncomingMailboxSnapshot, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let selected = select_mailbox(session, mailbox).await?;
    let unread_count =
        u32::try_from(search_uids(session, "UNSEEN").await?.len()).unwrap_or(u32::MAX);
    let retained = usize::try_from(limit.min(MAX_FETCH_MESSAGES)).unwrap_or_default();
    let (mut uids, coverage_complete) = if retained == 0 || since_uid == Some(u32::MAX) {
        (Vec::new(), false)
    } else {
        let criteria = since_uid
            .map(|uid| format!("UID {}:*", uid + 1))
            .unwrap_or_else(|| "ALL".into());
        let uids = search_uids(session, &criteria)
            .await?
            .into_iter()
            .filter(|uid| since_uid.is_none_or(|previous| *uid > previous))
            .collect::<Vec<_>>();
        let coverage_complete = since_uid.is_none() && uids.len() <= retained;
        (uids, coverage_complete)
    };
    if uids.len() > retained {
        if since_uid.is_some() {
            uids.truncate(retained);
        } else {
            uids.drain(..uids.len() - retained);
        }
    }
    if uids.is_empty() {
        return Ok(IncomingMailboxSnapshot {
            remote_id: mailbox.to_owned(),
            uid_validity: selected.uid_validity,
            total_count: selected.total_count,
            unread_count,
            coverage_complete,
            messages: Vec::new(),
        });
    }

    let uid_set = numeric_set(&uids);
    let metadata = session
        .execute(
            &format!("UID FETCH {uid_set} {MESSAGE_METADATA_ITEMS}"),
            CONTROL_RESPONSE_LIMIT,
            false,
        )
        .await?;
    let requested = uids.iter().copied().collect::<HashSet<_>>();
    let mut messages = metadata
        .responses
        .into_iter()
        .filter_map(|response| match response {
            Response::Fetch(sequence, attributes) => Some(message_from_fetch(sequence, attributes)),
            _ => None,
        })
        .collect::<Result<Vec<_>, IncomingError>>()?;
    if messages
        .iter()
        .any(|message| !requested.contains(&message.uid))
    {
        return Err(IncomingError::Protocol(
            "IMAP UID FETCH returned an unrequested UID".into(),
        ));
    }
    let fetched_uids = messages
        .iter()
        .map(|message| message.uid)
        .collect::<HashSet<_>>();
    if fetched_uids.len() != messages.len() {
        return Err(IncomingError::Protocol(
            "IMAP UID FETCH returned a duplicate UID".into(),
        ));
    }
    if fetched_uids != requested {
        return Err(IncomingError::Protocol(
            "IMAP UID FETCH omitted a requested UID".into(),
        ));
    }
    messages.sort_by_key(|message| std::cmp::Reverse(message.uid));

    let mut body_budget = MAX_BATCH_BODY_BYTES;
    let body_uids = messages
        .iter()
        .filter_map(|message| {
            let size = u64::from(message.size_bytes?);
            // Initial sync stays metadata-first for large messages. The reader can fetch one
            // selected message on demand without making background sync allocate
            // the same amount for every item in a page.
            if size > MAX_SYNC_BODY_BYTES || size > body_budget {
                return None;
            }
            body_budget -= size;
            Some(message.uid)
        })
        .collect::<Vec<_>>();
    if !body_uids.is_empty() {
        let body_uid_set = numeric_set(&body_uids);
        let body_result = session
            .execute(
                &format!("UID FETCH {body_uid_set} (UID BODY.PEEK[])"),
                BATCH_BODY_RESPONSE_LIMIT,
                false,
            )
            .await?;
        let requested_bodies = body_uids.iter().copied().collect::<HashSet<_>>();
        let mut bodies = HashMap::new();
        let mut actual_body_bytes = 0_u64;
        for response in body_result.responses {
            let Response::Fetch(_, attributes) = response else {
                continue;
            };
            let (uid, body) = body_from_fetch(attributes)?;
            if !requested_bodies.contains(&uid) {
                return Err(IncomingError::Protocol(
                    "IMAP body FETCH returned an unrequested UID".into(),
                ));
            }
            if let Some(body) = body {
                let body_size = u64::try_from(body.len()).unwrap_or(u64::MAX);
                if body_size <= MAX_SYNC_BODY_BYTES
                    && actual_body_bytes.saturating_add(body_size) <= MAX_BATCH_BODY_BYTES
                {
                    bodies.insert(uid, body);
                    actual_body_bytes += body_size;
                }
            }
        }
        for message in &mut messages {
            message.raw_rfc822 = bodies.remove(&message.uid);
        }
    }

    Ok(IncomingMailboxSnapshot {
        remote_id: mailbox.to_owned(),
        uid_validity: selected.uid_validity,
        total_count: selected.total_count,
        unread_count,
        coverage_complete,
        messages,
    })
}

async fn fetch_message_on_session<S>(
    session: &mut ImapSession<S>,
    mailbox: &str,
    uid: u32,
) -> Result<Option<IncomingMessageFetch>, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let selected = select_mailbox(session, mailbox).await?;
    let metadata = session
        .execute(
            &format!("UID FETCH {uid} {MESSAGE_METADATA_ITEMS}"),
            CONTROL_RESPONSE_LIMIT,
            false,
        )
        .await?;
    let mut messages = metadata
        .responses
        .into_iter()
        .filter_map(|response| match response {
            Response::Fetch(sequence, attributes) => Some(message_from_fetch(sequence, attributes)),
            _ => None,
        })
        .collect::<Result<Vec<_>, IncomingError>>()?;
    if messages.len() > 1 || messages.first().is_some_and(|message| message.uid != uid) {
        return Err(IncomingError::Protocol(
            "IMAP single-message FETCH returned an unexpected UID".into(),
        ));
    }
    let Some(mut message) = messages.pop() else {
        return Ok(None);
    };
    let body_result = session
        .execute(
            &format!("UID FETCH {uid} (UID BODY.PEEK[])"),
            usize::MAX,
            false,
        )
        .await?;
    for response in body_result.responses {
        let Response::Fetch(_, attributes) = response else {
            continue;
        };
        let (response_uid, body) = body_from_fetch(attributes)?;
        if response_uid != uid {
            return Err(IncomingError::Protocol(
                "IMAP single-message body FETCH returned an unexpected UID".into(),
            ));
        }
        if let Some(body) = body {
            message.raw_rfc822 = Some(body);
        }
    }
    Ok(Some(IncomingMessageFetch {
        remote_id: mailbox.to_owned(),
        uid_validity: selected.uid_validity,
        message,
    }))
}

/// Fetches an older page as metadata-only. Full bodies are intentionally left
/// to the reader's lazy hydration path so historical backfill cannot download
/// another batch-sized body budget on every page.
async fn fetch_messages_before_on_session<S>(
    session: &mut ImapSession<S>,
    mailbox: &str,
    before_uid: u32,
    limit: u32,
) -> Result<IncomingMailboxSnapshot, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let selected = select_mailbox(session, mailbox).await?;
    let unread_count =
        u32::try_from(search_uids(session, "UNSEEN").await?.len()).unwrap_or(u32::MAX);
    let retained = usize::try_from(limit.min(MAX_FETCH_MESSAGES)).unwrap_or_default();
    if before_uid <= 1 || retained == 0 {
        return Ok(IncomingMailboxSnapshot {
            remote_id: mailbox.to_owned(),
            uid_validity: selected.uid_validity,
            total_count: selected.total_count,
            unread_count,
            coverage_complete: false,
            messages: Vec::new(),
        });
    }

    let mut uids = search_uids(session, &format!("UID 1:{}", before_uid - 1)).await?;
    if uids.len() > retained {
        uids.drain(..uids.len() - retained);
    }
    if uids.is_empty() {
        return Ok(IncomingMailboxSnapshot {
            remote_id: mailbox.to_owned(),
            uid_validity: selected.uid_validity,
            total_count: selected.total_count,
            unread_count,
            coverage_complete: false,
            messages: Vec::new(),
        });
    }

    let requested = uids.iter().copied().collect::<HashSet<_>>();
    let metadata = session
        .execute(
            &format!("UID FETCH {} {MESSAGE_METADATA_ITEMS}", numeric_set(&uids)),
            CONTROL_RESPONSE_LIMIT,
            false,
        )
        .await?;
    let mut messages = metadata
        .responses
        .into_iter()
        .filter_map(|response| match response {
            Response::Fetch(sequence, attributes) => Some(message_from_fetch(sequence, attributes)),
            _ => None,
        })
        .collect::<Result<Vec<_>, IncomingError>>()?;
    if messages
        .iter()
        .any(|message| !requested.contains(&message.uid))
    {
        return Err(IncomingError::Protocol(
            "IMAP historical UID FETCH returned an unrequested UID".into(),
        ));
    }
    let fetched_uids = messages
        .iter()
        .map(|message| message.uid)
        .collect::<HashSet<_>>();
    if fetched_uids.len() != messages.len() {
        return Err(IncomingError::Protocol(
            "IMAP historical UID FETCH returned a duplicate UID".into(),
        ));
    }
    if fetched_uids != requested {
        return Err(IncomingError::Protocol(
            "IMAP historical UID FETCH omitted a requested UID".into(),
        ));
    }
    messages.sort_by_key(|message| std::cmp::Reverse(message.uid));
    Ok(IncomingMailboxSnapshot {
        remote_id: mailbox.to_owned(),
        uid_validity: selected.uid_validity,
        total_count: selected.total_count,
        unread_count,
        coverage_complete: false,
        messages,
    })
}

async fn apply_remote_operation_on_session<S>(
    session: &mut ImapSession<S>,
    capability_names: &[String],
    operation: &RemoteMessageOperation,
) -> Result<(), IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let has_capability = |expected: &str| {
        capability_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(expected))
    };
    match operation {
        RemoteMessageOperation::SetFlags {
            mailbox_remote_id,
            uid,
            expected_uid_validity,
            is_read,
            is_starred,
        } => {
            validate_uid(*uid)?;
            let selected = select_mailbox_read_write(session, mailbox_remote_id).await?;
            validate_uid_validity(selected.uid_validity, *expected_uid_validity)?;
            let additions = [
                is_read.filter(|value| *value).map(|_| "\\Seen"),
                is_starred.filter(|value| *value).map(|_| "\\Flagged"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            let removals = [
                is_read.filter(|value| !*value).map(|_| "\\Seen"),
                is_starred.filter(|value| !*value).map(|_| "\\Flagged"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if !additions.is_empty() {
                session
                    .execute(
                        &format!("UID STORE {uid} +FLAGS.SILENT ({})", additions.join(" ")),
                        CONTROL_RESPONSE_LIMIT,
                        false,
                    )
                    .await?;
            }
            if !removals.is_empty() {
                session
                    .execute(
                        &format!("UID STORE {uid} -FLAGS.SILENT ({})", removals.join(" ")),
                        CONTROL_RESPONSE_LIMIT,
                        false,
                    )
                    .await?;
            }
            Ok(())
        }
        RemoteMessageOperation::Move {
            source_mailbox_remote_id,
            target_mailbox_remote_id,
            uid,
            expected_uid_validity,
        } => {
            let supports_move = has_capability("MOVE");
            let supports_uidplus = has_capability("UIDPLUS");
            if !supports_move && !supports_uidplus {
                return Err(IncomingError::Unsupported(
                    "safe IMAP move requires MOVE or UIDPLUS".into(),
                ));
            }
            validate_uid(*uid)?;
            let selected = select_mailbox_read_write(session, source_mailbox_remote_id).await?;
            validate_uid_validity(selected.uid_validity, *expected_uid_validity)?;
            if source_mailbox_remote_id == target_mailbox_remote_id {
                return Ok(());
            }
            let target = quote_imap(target_mailbox_remote_id)?;
            if supports_move {
                session
                    .execute(
                        &format!("UID MOVE {uid} {target}"),
                        CONTROL_RESPONSE_LIMIT,
                        false,
                    )
                    .await?;
            } else {
                // UIDPLUS makes the final expunge UID-scoped. If a later step
                // fails, the copied destination and original source remain
                // recoverable; never issue a mailbox-wide EXPUNGE fallback.
                session
                    .execute(
                        &format!("UID COPY {uid} {target}"),
                        CONTROL_RESPONSE_LIMIT,
                        false,
                    )
                    .await?;
                session
                    .execute(
                        &format!("UID STORE {uid} +FLAGS.SILENT (\\Deleted)"),
                        CONTROL_RESPONSE_LIMIT,
                        false,
                    )
                    .await?;
                session
                    .execute(&format!("UID EXPUNGE {uid}"), CONTROL_RESPONSE_LIMIT, false)
                    .await?;
            }
            Ok(())
        }
        RemoteMessageOperation::DeletePermanently {
            mailbox_remote_id,
            uid,
            expected_uid_validity,
        } => {
            if !has_capability("UIDPLUS") {
                return Err(IncomingError::Unsupported(
                    "safe per-message expunge requires IMAP UIDPLUS".into(),
                ));
            }
            validate_uid(*uid)?;
            let selected = select_mailbox_read_write(session, mailbox_remote_id).await?;
            validate_uid_validity(selected.uid_validity, *expected_uid_validity)?;
            session
                .execute(
                    &format!("UID STORE {uid} +FLAGS.SILENT (\\Deleted)"),
                    CONTROL_RESPONSE_LIMIT,
                    false,
                )
                .await?;
            session
                .execute(&format!("UID EXPUNGE {uid}"), CONTROL_RESPONSE_LIMIT, false)
                .await?;
            Ok(())
        }
    }
}

fn validate_uid(uid: u32) -> Result<(), IncomingError> {
    if uid == 0 {
        Err(IncomingError::Protocol(
            "IMAP message operation requires a non-zero UID".into(),
        ))
    } else {
        Ok(())
    }
}

fn validate_uid_validity(actual: Option<u32>, expected: Option<u32>) -> Result<(), IncomingError> {
    match (actual, expected) {
        (Some(actual), Some(expected)) if actual == expected => Ok(()),
        (Some(_), Some(_)) => Err(IncomingError::Protocol(
            "IMAP mailbox UIDVALIDITY changed before the queued operation was applied".into(),
        )),
        _ => Err(IncomingError::Protocol(
            "safe IMAP mutation requires UIDVALIDITY on both the local instance and server".into(),
        )),
    }
}

async fn append_message_on_session<S>(
    session: &mut ImapSession<S>,
    mailbox: &str,
    raw_rfc822: &[u8],
    mark_seen: bool,
) -> Result<AppendMessageResult, IncomingError>
where
    S: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let result = session
        .append_literal(mailbox, raw_rfc822, mark_seen)
        .await?;
    match result.completion_code {
        Some(ResponseCode::AppendUid(uid_validity, uid_set)) => {
            let uid = single_uid(&uid_set).ok_or_else(|| {
                IncomingError::Protocol("IMAP APPENDUID did not contain exactly one UID".into())
            })?;
            Ok(AppendMessageResult {
                uid_validity: Some(uid_validity),
                uid: Some(uid),
            })
        }
        _ => Ok(AppendMessageResult {
            uid_validity: None,
            uid: None,
        }),
    }
}

fn single_uid(uid_set: &[UidSetMember]) -> Option<u32> {
    match uid_set {
        [UidSetMember::Uid(uid)] => Some(*uid),
        [UidSetMember::UidRange(range)] if range.start() == range.end() => Some(*range.start()),
        _ => None,
    }
}

fn message_from_fetch(
    sequence: u32,
    attributes: Vec<AttributeValue<'static>>,
) -> Result<IncomingMessage, IncomingError> {
    let mut uid = None;
    let mut flags = Vec::new();
    let mut internal_date = None;
    let mut size_bytes = None;
    let mut raw_headers = None;
    for attribute in attributes {
        match attribute {
            AttributeValue::Uid(value) => uid = Some(value),
            AttributeValue::Flags(values) => {
                flags = values.into_iter().map(|value| value.into_owned()).collect()
            }
            AttributeValue::InternalDate(value) => {
                internal_date = parse_internal_date(&value);
            }
            AttributeValue::Rfc822Size(value) => size_bytes = Some(value),
            AttributeValue::BodySection {
                section: Some(SectionPath::Full(MessageSection::Header)),
                data: Some(value),
                ..
            } => raw_headers = Some(value.into_owned()),
            _ => {}
        }
    }
    Ok(IncomingMessage {
        sequence,
        uid: uid
            .ok_or_else(|| IncomingError::Protocol("IMAP UID FETCH response omitted UID".into()))?,
        flags,
        internal_date,
        size_bytes,
        raw_headers,
        raw_rfc822: None,
    })
}

fn body_from_fetch(
    attributes: Vec<AttributeValue<'static>>,
) -> Result<(u32, Option<Vec<u8>>), IncomingError> {
    let mut uid = None;
    let mut body = None;
    for attribute in attributes {
        match attribute {
            AttributeValue::Uid(value) => uid = Some(value),
            AttributeValue::BodySection {
                section: None,
                data: Some(value),
                ..
            }
            | AttributeValue::Rfc822(Some(value)) => body = Some(value.into_owned()),
            _ => {}
        }
    }
    Ok((
        uid.ok_or_else(|| IncomingError::Protocol("IMAP body FETCH response omitted UID".into()))?,
        body,
    ))
}

fn parse_internal_date(value: &str) -> Option<String> {
    DateTime::parse_from_str(value, "%d-%b-%Y %H:%M:%S %z")
        .ok()
        .map(|date| date.to_rfc3339())
}

fn numeric_set(values: &[u32]) -> String {
    values
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn capability_name(capability: &Capability<'_>) -> String {
    match capability {
        Capability::Imap4rev1 => "IMAP4REV1".into(),
        Capability::Auth(method) => format!("AUTH={method}"),
        Capability::Atom(name) => name.to_ascii_uppercase(),
    }
}

fn capabilities_from_names(names: &[String]) -> ProviderCapabilities {
    let contains = |value: &str| names.iter().any(|name| name.eq_ignore_ascii_case(value));
    ProviderCapabilities {
        folders: true,
        flags: true,
        r#move: contains("MOVE"),
        append: true,
        append_sent: false,
        idle_push: contains("IDLE"),
        keywords: contains("KEYWORDS"),
        partial_fetch: true,
        threading: names.iter().any(|name| name.starts_with("THREAD=")),
        ..Default::default()
    }
}

fn name_attribute(attribute: &NameAttribute<'_>) -> String {
    match attribute {
        NameAttribute::NoInferiors => "\\Noinferiors".into(),
        NameAttribute::NoSelect => "\\Noselect".into(),
        NameAttribute::Marked => "\\Marked".into(),
        NameAttribute::Unmarked => "\\Unmarked".into(),
        NameAttribute::All => "\\All".into(),
        NameAttribute::Archive => "\\Archive".into(),
        NameAttribute::Drafts => "\\Drafts".into(),
        NameAttribute::Flagged => "\\Flagged".into(),
        NameAttribute::Junk => "\\Junk".into(),
        NameAttribute::Sent => "\\Sent".into(),
        NameAttribute::Trash => "\\Trash".into(),
        NameAttribute::Extension(value) => value.to_string(),
        _ => "\\Unknown".into(),
    }
}

fn special_role(attribute: &NameAttribute<'_>) -> Option<String> {
    let role = match attribute {
        NameAttribute::All => "all",
        NameAttribute::Archive => "archive",
        NameAttribute::Drafts => "drafts",
        NameAttribute::Flagged => "starred",
        NameAttribute::Junk => "junk",
        NameAttribute::Sent => "sent",
        NameAttribute::Trash => "trash",
        _ => return None,
    };
    Some(role.into())
}

fn decode_modified_utf7(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_start) = value[cursor..].find('&') {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let Some(relative_end) = value[start + 1..].find('-') else {
            return value.to_owned();
        };
        let end = start + 1 + relative_end;
        let encoded = &value[start + 1..end];
        if encoded.is_empty() {
            output.push('&');
        } else {
            let mut standard = encoded.replace(',', "/");
            standard.extend(std::iter::repeat_n('=', (4 - standard.len() % 4) % 4));
            let Ok(bytes) = STANDARD.decode(standard) else {
                return value.to_owned();
            };
            if bytes.len() % 2 != 0 {
                return value.to_owned();
            }
            let utf16 = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            let Ok(decoded) = String::from_utf16(&utf16) else {
                return value.to_owned();
            };
            output.push_str(&decoded);
        }
        cursor = end + 1;
    }
    output.push_str(&value[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    use super::*;

    #[derive(Debug, Clone)]
    struct MockStream {
        input: Arc<Mutex<Cursor<Vec<u8>>>>,
        output: Arc<Mutex<Vec<u8>>>,
    }

    impl MockStream {
        fn new(input: impl Into<Vec<u8>>) -> Self {
            Self {
                input: Arc::new(Mutex::new(Cursor::new(input.into()))),
                output: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl AsyncRead for MockStream {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let mut input = self.input.lock().expect("mock input lock");
            let position = usize::try_from(input.position()).unwrap_or(usize::MAX);
            let bytes = input.get_ref();
            if position >= bytes.len() {
                return Poll::Ready(Ok(()));
            }
            let count = buffer.remaining().min(bytes.len() - position);
            buffer.put_slice(&bytes[position..position + count]);
            input.set_position(u64::try_from(position + count).unwrap_or(u64::MAX));
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for MockStream {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.output
                .lock()
                .expect("mock output lock")
                .extend_from_slice(buffer);
            Poll::Ready(Ok(buffer.len()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn validates_greeting_status_and_crlf() {
        assert_eq!(
            parse_greeting_line(b"* OK server ready\r\n").expect("valid greeting"),
            "* OK server ready"
        );
        assert!(matches!(
            parse_greeting_line(b"* BYE unavailable\r\n"),
            Err(IncomingError::Protocol(_))
        ));
        assert!(matches!(
            parse_greeting_line(b"* OK only-lf\n"),
            Err(IncomingError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn login_requires_matching_tagged_ok() {
        let mut missing = ImapSession::new(MockStream::new(Vec::new()));
        assert!(matches!(
            missing
                .execute(
                    "LOGIN \"alice@example.com\" \"secret\"",
                    CONTROL_RESPONSE_LIMIT,
                    true
                )
                .await,
            Err(IncomingError::Protocol(_))
        ));

        let mut rejected = ImapSession::new(MockStream::new(
            b"A0001 NO invalid credentials\r\n".to_vec(),
        ));
        assert!(matches!(
            rejected
                .execute(
                    "LOGIN \"alice@example.com\" \"wrong\"",
                    CONTROL_RESPONSE_LIMIT,
                    true
                )
                .await,
            Err(IncomingError::Authentication)
        ));
    }

    #[tokio::test]
    async fn every_command_requires_its_own_tagged_ok() {
        let mut missing_starttls = ImapSession::new(MockStream::new(Vec::new()));
        assert!(matches!(
            missing_starttls
                .execute("STARTTLS", CONTROL_RESPONSE_LIMIT, false)
                .await,
            Err(IncomingError::Protocol(_))
        ));

        let mut rejected_starttls =
            ImapSession::new(MockStream::new(b"A0001 NO TLS unavailable\r\n".to_vec()));
        assert!(matches!(
            rejected_starttls
                .execute("STARTTLS", CONTROL_RESPONSE_LIMIT, false)
                .await,
            Err(IncomingError::Protocol(_))
        ));

        let mut missing_capability =
            ImapSession::new(MockStream::new(b"* CAPABILITY IMAP4rev1 IDLE\r\n".to_vec()));
        assert!(matches!(
            read_capabilities(&mut missing_capability).await,
            Err(IncomingError::Protocol(_))
        ));

        let mut wrong_tag =
            ImapSession::new(MockStream::new(b"A9999 OK unrelated command\r\n".to_vec()));
        assert!(matches!(
            wrong_tag
                .execute("CAPABILITY", CONTROL_RESPONSE_LIMIT, false)
                .await,
            Err(IncomingError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn idle_wakes_only_for_a_mailbox_change_and_ends_cleanly() {
        let stream =
            MockStream::new(b"+ idling\r\n* 3 EXISTS\r\nA0001 OK IDLE terminated\r\n".to_vec());
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);

        assert!(session
            .idle_until_change(Duration::from_secs(1))
            .await
            .expect("IDLE mailbox event"));
        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert_eq!(commands, "A0001 IDLE\r\nDONE\r\n");
    }

    #[tokio::test]
    async fn idle_ignores_keepalive_without_triggering_a_sync() {
        let stream = MockStream::new(
            b"+ idling\r\n* OK Still here\r\nA0001 OK server ended IDLE\r\n".to_vec(),
        );
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);

        assert!(!session
            .idle_until_change(Duration::from_secs(1))
            .await
            .expect("IDLE keepalive"));
        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert_eq!(commands, "A0001 IDLE\r\n");
    }

    #[tokio::test]
    async fn parses_list_uid_flags_headers_and_rfc822_literal_without_network() {
        let headers = b"From: Sender <sender@example.com>\r\nSubject: Test\r\n\r\n";
        let raw = b"From: Sender <sender@example.com>\r\nSubject: Test\r\n\r\nBody";
        let responses = format!(
            "A0001 OK logged in\r\n\
             * LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
             * LIST (\\Sent) \"/\" \"Sent\"\r\n\
             A0002 OK list complete\r\n\
             * FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)\r\n\
             * 2 EXISTS\r\n\
             * 0 RECENT\r\n\
             * OK [UIDVALIDITY 44] valid\r\n\
             A0003 OK [READ-ONLY] examine complete\r\n\
             * SEARCH 42\r\n\
             A0004 OK search complete\r\n\
             * SEARCH 41 42\r\n\
             A0005 OK search complete\r\n\
             * 2 FETCH (UID 42 FLAGS (\\Seen \\Flagged) INTERNALDATE \"03-Sep-2026 12:00:00 +0800\" RFC822.SIZE {} BODY[HEADER] {{{}}}\r\n",
            raw.len(),
            headers.len()
        );
        let mut response_bytes = responses.into_bytes();
        response_bytes.extend_from_slice(headers);
        response_bytes.extend_from_slice(b")\r\nA0006 OK fetch complete\r\n");
        response_bytes.extend_from_slice(
            format!("* 2 FETCH (UID 42 BODY[] {{{}}}\r\n", raw.len()).as_bytes(),
        );
        response_bytes.extend_from_slice(raw);
        response_bytes.extend_from_slice(b")\r\nA0007 OK fetch complete\r\n");
        let stream = MockStream::new(response_bytes);
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        session
            .execute(
                "LOGIN \"alice@example.com\" \"secret\"",
                CONTROL_RESPONSE_LIMIT,
                true,
            )
            .await
            .expect("login");

        let mailboxes = list_mailboxes_on_session(&mut session)
            .await
            .expect("mailboxes");
        assert_eq!(mailboxes.len(), 2);
        assert_eq!(mailboxes[0].special_role.as_deref(), Some("inbox"));
        assert_eq!(mailboxes[1].special_role.as_deref(), Some("sent"));

        let snapshot = fetch_messages_on_session(&mut session, "INBOX", None, 1)
            .await
            .expect("snapshot");
        assert_eq!(snapshot.uid_validity, Some(44));
        assert_eq!(snapshot.total_count, 2);
        assert_eq!(snapshot.unread_count, 1);
        assert!(!snapshot.coverage_complete);
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.messages[0].uid, 42);
        assert_eq!(snapshot.messages[0].flags, ["\\Seen", "\\Flagged"]);
        assert_eq!(
            snapshot.messages[0].raw_headers.as_deref(),
            Some(&headers[..])
        );
        assert_eq!(snapshot.messages[0].raw_rfc822.as_deref(), Some(&raw[..]));

        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert!(commands.contains("A0002 LIST \"\" \"*\"\r\n"));
        assert!(commands.contains("A0005 UID SEARCH ALL\r\n"));
        assert!(commands.contains("A0007 UID FETCH 42 (UID BODY.PEEK[])\r\n"));
    }

    #[tokio::test]
    async fn incremental_search_keeps_oldest_page_after_cursor() {
        let responses = b"* 4 EXISTS\r\n\
                          * OK [UIDVALIDITY 9] valid\r\n\
                          A0001 OK examine\r\n\
                          * SEARCH\r\n\
                          A0002 OK unseen\r\n\
                          * SEARCH 11 12 13\r\n\
                          A0003 OK search\r\n\
                          * 2 FETCH (UID 11 FLAGS () RFC822.SIZE 30000000)\r\n\
                          * 3 FETCH (UID 12 FLAGS () RFC822.SIZE 30000000)\r\n\
                          A0004 OK fetch\r\n"
            .to_vec();
        let mut session = ImapSession::new(MockStream::new(responses));
        let snapshot = fetch_messages_on_session(&mut session, "INBOX", Some(10), 2)
            .await
            .expect("incremental snapshot");
        let mut uids = snapshot
            .messages
            .iter()
            .map(|message| message.uid)
            .collect::<Vec<_>>();
        uids.sort_unstable();
        assert_eq!(uids, [11, 12]);
        assert!(!snapshot.coverage_complete);
    }

    #[tokio::test]
    async fn full_snapshot_coverage_requires_an_untruncated_complete_fetch() {
        let raced_arrival = b"* 1 EXISTS\r\n\
                              * OK [UIDVALIDITY 9] valid\r\n\
                              A0001 OK examine\r\n\
                              * SEARCH\r\n\
                              A0002 OK unseen\r\n\
                              * SEARCH 1 2\r\n\
                              A0003 OK all\r\n\
                              * 2 FETCH (UID 2 FLAGS () RFC822.SIZE 30000000)\r\n\
                              A0004 OK fetch\r\n"
            .to_vec();
        let mut session = ImapSession::new(MockStream::new(raced_arrival));
        let snapshot = fetch_messages_on_session(&mut session, "INBOX", None, 1)
            .await
            .expect("truncated full snapshot");
        assert!(!snapshot.coverage_complete);

        let complete = b"* 1 EXISTS\r\n\
                         * OK [UIDVALIDITY 9] valid\r\n\
                         A0001 OK examine\r\n\
                         * SEARCH\r\n\
                         A0002 OK unseen\r\n\
                         * SEARCH 7\r\n\
                         A0003 OK all\r\n\
                         * 1 FETCH (UID 7 FLAGS () RFC822.SIZE 30000000)\r\n\
                         A0004 OK fetch\r\n"
            .to_vec();
        let mut session = ImapSession::new(MockStream::new(complete));
        let snapshot = fetch_messages_on_session(&mut session, "INBOX", None, 2)
            .await
            .expect("complete full snapshot");
        assert!(snapshot.coverage_complete);

        let omitted_uid = b"* 2 EXISTS\r\n\
                            * OK [UIDVALIDITY 9] valid\r\n\
                            A0001 OK examine\r\n\
                            * SEARCH\r\n\
                            A0002 OK unseen\r\n\
                            * SEARCH 7 8\r\n\
                            A0003 OK all\r\n\
                            * 2 FETCH (UID 8 FLAGS () RFC822.SIZE 30000000)\r\n\
                            A0004 OK fetch\r\n"
            .to_vec();
        let mut session = ImapSession::new(MockStream::new(omitted_uid));
        assert!(matches!(
            fetch_messages_on_session(&mut session, "INBOX", None, 2).await,
            Err(IncomingError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn mailbox_index_uses_one_uid_flags_fetch_as_authority() {
        let responses = b"* 3 EXISTS\r\n\
                          * OK [UIDVALIDITY 22] valid\r\n\
                          A0001 OK examine\r\n\
                          * 1 FETCH (UID 4 FLAGS (\\Seen \\Flagged))\r\n\
                          * 2 FETCH (UID 2 FLAGS ())\r\n\
                          * 3 FETCH (UID 9 FLAGS (\\Seen))\r\n\
                          A0002 OK flags complete\r\n"
            .to_vec();
        let stream = MockStream::new(responses);
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        let index = fetch_mailbox_index_on_session(&mut session, "INBOX")
            .await
            .expect("mailbox index");
        assert_eq!(index.remote_id, "INBOX");
        assert_eq!(index.uid_validity, Some(22));
        assert_eq!(index.total_count, 3);
        assert_eq!(index.all_uids, [2, 4, 9]);
        assert_eq!(index.unseen_uids, [2]);
        assert_eq!(index.flagged_uids, [4]);

        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert!(commands.contains("A0002 FETCH 1:* (UID FLAGS)\r\n"));
        assert_eq!(commands.matches("FETCH 1:*").count(), 1);

        let duplicate = b"* 2 EXISTS\r\n\
                          * OK [UIDVALIDITY 22] valid\r\n\
                          A0001 OK examine\r\n\
                          * 1 FETCH (UID 2 FLAGS ())\r\n\
                          * 2 FETCH (UID 2 FLAGS (\\Seen))\r\n\
                          A0002 OK flags complete\r\n"
            .to_vec();
        let mut session = ImapSession::new(MockStream::new(duplicate));
        assert!(matches!(
            fetch_mailbox_index_on_session(&mut session, "INBOX").await,
            Err(IncomingError::Protocol(_))
        ));

        let empty_stream = MockStream::new(
            b"* 0 EXISTS\r\n* OK [UIDVALIDITY 23] valid\r\nA0001 OK examine\r\n".to_vec(),
        );
        let empty_writes = Arc::clone(&empty_stream.output);
        let mut empty_session = ImapSession::new(empty_stream);
        let empty = fetch_mailbox_index_on_session(&mut empty_session, "Empty")
            .await
            .expect("empty mailbox index");
        assert!(empty.all_uids.is_empty());
        assert!(
            !String::from_utf8(empty_writes.lock().expect("mock output lock").clone())
                .expect("utf-8 commands")
                .contains("FETCH")
        );
    }

    #[tokio::test]
    async fn historical_fetch_pages_backwards_without_downloading_bodies() {
        let responses = b"* 6 EXISTS\r\n\
                          * OK [UIDVALIDITY 12] valid\r\n\
                          A0001 OK examine\r\n\
                          * SEARCH 2 4\r\n\
                          A0002 OK unseen\r\n\
                          * SEARCH 1 2 3 4\r\n\
                          A0003 OK search\r\n\
                          * 3 FETCH (UID 3 FLAGS () RFC822.SIZE 30000000)\r\n\
                          * 4 FETCH (UID 4 FLAGS (\\Seen) RFC822.SIZE 30000000)\r\n\
                          A0004 OK fetch\r\n"
            .to_vec();
        let stream = MockStream::new(responses);
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        let snapshot = fetch_messages_before_on_session(&mut session, "INBOX", 5, 2)
            .await
            .expect("historical page");
        assert_eq!(snapshot.uid_validity, Some(12));
        assert!(!snapshot.coverage_complete);
        assert_eq!(
            snapshot
                .messages
                .iter()
                .map(|message| message.uid)
                .collect::<Vec<_>>(),
            [4, 3]
        );
        assert!(snapshot
            .messages
            .iter()
            .all(|message| message.raw_rfc822.is_none()));
        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert!(commands.contains("A0003 UID SEARCH UID 1:4\r\n"));
        assert!(commands.contains("A0004 UID FETCH 3,4 "));
        assert!(!commands.contains("(UID BODY.PEEK[])"));
    }

    #[tokio::test]
    async fn exact_message_fetch_returns_mailbox_identity_and_rejects_wrong_uid() {
        let raw = b"Subject: Exact\r\n\r\nBody";
        let mut response = format!(
            "* 1 EXISTS\r\n* OK [UIDVALIDITY 51] valid\r\nA0001 OK examine\r\n\
             * 1 FETCH (UID 7 FLAGS () RFC822.SIZE {})\r\nA0002 OK metadata\r\n\
             * 1 FETCH (UID 7 BODY[] {{{}}}\r\n",
            101 * 1024 * 1024, // Server metadata must not impose a client-side cutoff.
            raw.len()
        )
        .into_bytes();
        response.extend_from_slice(raw);
        response.extend_from_slice(b")\r\nA0003 OK body\r\n");
        let mut session = ImapSession::new(MockStream::new(response));
        let fetched = fetch_message_on_session(&mut session, "INBOX", 7)
            .await
            .expect("single fetch")
            .expect("message");
        assert_eq!(fetched.remote_id, "INBOX");
        assert_eq!(fetched.uid_validity, Some(51));
        assert_eq!(fetched.message.raw_rfc822.as_deref(), Some(&raw[..]));

        let wrong_uid = b"* 1 EXISTS\r\n* OK [UIDVALIDITY 51] valid\r\nA0001 OK examine\r\n\
                          * 1 FETCH (UID 8 FLAGS () RFC822.SIZE 20)\r\nA0002 OK metadata\r\n";
        let mut session = ImapSession::new(MockStream::new(wrong_uid.to_vec()));
        assert!(matches!(
            fetch_message_on_session(&mut session, "INBOX", 7).await,
            Err(IncomingError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn remote_mutations_validate_uidvalidity_and_use_uid_scoped_commands() {
        let responses =
            b"* 1 EXISTS\r\n* OK [UIDVALIDITY 9] valid\r\nA0001 OK [READ-WRITE] selected\r\n\
                          A0002 OK stored\r\nA0003 OK stored\r\n"
                .to_vec();
        let stream = MockStream::new(responses);
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        apply_remote_operation_on_session(
            &mut session,
            &["IMAP4REV1".into()],
            &RemoteMessageOperation::SetFlags {
                mailbox_remote_id: "INBOX".into(),
                uid: 42,
                expected_uid_validity: Some(9),
                is_read: Some(true),
                is_starred: Some(false),
            },
        )
        .await
        .expect("set flags");
        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert!(commands.contains("A0001 SELECT \"INBOX\"\r\n"));
        assert!(commands.contains("A0002 UID STORE 42 +FLAGS.SILENT (\\Seen)\r\n"));
        assert!(commands.contains("A0003 UID STORE 42 -FLAGS.SILENT (\\Flagged)\r\n"));

        let mismatch = b"* 1 EXISTS\r\n* OK [UIDVALIDITY 10] valid\r\nA0001 OK selected\r\n";
        let stream = MockStream::new(mismatch.to_vec());
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        assert!(matches!(
            apply_remote_operation_on_session(
                &mut session,
                &["IMAP4REV1".into()],
                &RemoteMessageOperation::SetFlags {
                    mailbox_remote_id: "INBOX".into(),
                    uid: 42,
                    expected_uid_validity: Some(9),
                    is_read: Some(true),
                    is_starred: None,
                },
            )
            .await,
            Err(IncomingError::Protocol(_))
        ));
        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert!(!commands.contains("UID STORE"));
    }

    #[tokio::test]
    async fn move_uses_move_or_uidplus_fallback_and_never_bare_expunge() {
        let mut unsupported_move = ImapSession::new(MockStream::new(Vec::new()));
        assert!(matches!(
            apply_remote_operation_on_session(
                &mut unsupported_move,
                &["IMAP4REV1".into()],
                &RemoteMessageOperation::Move {
                    source_mailbox_remote_id: "INBOX".into(),
                    target_mailbox_remote_id: "Archive".into(),
                    uid: 3,
                    expected_uid_validity: Some(8),
                },
            )
            .await,
            Err(IncomingError::Unsupported(_))
        ));

        let direct_responses = b"* 1 EXISTS\r\n* OK [UIDVALIDITY 8] valid\r\nA0001 OK selected\r\n\
              A0002 OK moved\r\n"
            .to_vec();
        let stream = MockStream::new(direct_responses);
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        apply_remote_operation_on_session(
            &mut session,
            &["IMAP4REV1".into(), "MOVE".into()],
            &RemoteMessageOperation::Move {
                source_mailbox_remote_id: "INBOX".into(),
                target_mailbox_remote_id: "Archive".into(),
                uid: 3,
                expected_uid_validity: Some(8),
            },
        )
        .await
        .expect("native UID MOVE");
        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert!(commands.contains("A0002 UID MOVE 3 \"Archive\"\r\n"));
        assert!(!commands.contains("UID COPY"));

        let fallback_responses =
            b"* 1 EXISTS\r\n* OK [UIDVALIDITY 8] valid\r\nA0001 OK selected\r\n\
              A0002 OK [COPYUID 8 3 31] copied\r\n\
              A0003 OK marked deleted\r\n\
              A0004 OK expunged\r\n"
                .to_vec();
        let stream = MockStream::new(fallback_responses);
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        apply_remote_operation_on_session(
            &mut session,
            &["IMAP4REV1".into(), "UIDPLUS".into()],
            &RemoteMessageOperation::Move {
                source_mailbox_remote_id: "INBOX".into(),
                target_mailbox_remote_id: "Archive".into(),
                uid: 3,
                expected_uid_validity: Some(8),
            },
        )
        .await
        .expect("UIDPLUS move fallback");
        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert!(commands.contains("A0002 UID COPY 3 \"Archive\"\r\n"));
        assert!(commands.contains("A0003 UID STORE 3 +FLAGS.SILENT (\\Deleted)\r\n"));
        assert!(commands.contains("A0004 UID EXPUNGE 3\r\n"));
        assert!(!commands.contains("\r\nA0004 EXPUNGE\r\n"));
    }

    #[tokio::test]
    async fn permanent_delete_requires_uidplus_and_uses_uid_scoped_expunge() {
        let mut unsupported_delete = ImapSession::new(MockStream::new(Vec::new()));
        assert!(matches!(
            apply_remote_operation_on_session(
                &mut unsupported_delete,
                &["IMAP4REV1".into()],
                &RemoteMessageOperation::DeletePermanently {
                    mailbox_remote_id: "Trash".into(),
                    uid: 3,
                    expected_uid_validity: Some(8),
                },
            )
            .await,
            Err(IncomingError::Unsupported(_))
        ));

        let responses = b"* 1 EXISTS\r\n* OK [UIDVALIDITY 8] valid\r\nA0001 OK selected\r\n\
                          A0002 OK deleted flag\r\nA0003 OK expunged\r\n"
            .to_vec();
        let stream = MockStream::new(responses);
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        apply_remote_operation_on_session(
            &mut session,
            &["IMAP4REV1".into(), "UIDPLUS".into()],
            &RemoteMessageOperation::DeletePermanently {
                mailbox_remote_id: "Trash".into(),
                uid: 3,
                expected_uid_validity: Some(8),
            },
        )
        .await
        .expect("UID-scoped permanent delete");
        let commands = String::from_utf8(writes.lock().expect("mock output lock").clone())
            .expect("utf-8 commands");
        assert!(commands.contains("UID STORE 3 +FLAGS.SILENT (\\Deleted)"));
        assert!(commands.contains("UID EXPUNGE 3"));
        assert!(!commands.contains("\r\nA0003 EXPUNGE"));
    }

    #[tokio::test]
    async fn append_waits_for_continuation_preserves_bytes_and_requires_tagged_ok() {
        let raw = b"From: sender@example.com\r\nSubject: Sent\r\n\r\nExact body";
        let stream = MockStream::new(
            b"+ ready for literal\r\nA0001 OK [APPENDUID 77 88] saved\r\n".to_vec(),
        );
        let writes = Arc::clone(&stream.output);
        let mut session = ImapSession::new(stream);
        let appended = append_message_on_session(&mut session, "Sent Items", raw, true)
            .await
            .expect("append");
        assert_eq!(appended.uid_validity, Some(77));
        assert_eq!(appended.uid, Some(88));
        let mut expected =
            format!("A0001 APPEND \"Sent Items\" (\\Seen) {{{}}}\r\n", raw.len()).into_bytes();
        expected.extend_from_slice(raw);
        expected.extend_from_slice(b"\r\n");
        assert_eq!(*writes.lock().expect("mock output lock"), expected);

        let mut missing_completion =
            ImapSession::new(MockStream::new(b"+ ready for literal\r\n".to_vec()));
        assert!(matches!(
            append_message_on_session(&mut missing_completion, "Sent", raw, false).await,
            Err(IncomingError::Protocol(_))
        ));

        let stream = MockStream::new(b"A0001 OK no continuation\r\n".to_vec());
        let writes = Arc::clone(&stream.output);
        let mut no_continuation = ImapSession::new(stream);
        assert!(matches!(
            append_message_on_session(&mut no_continuation, "Sent", raw, false).await,
            Err(IncomingError::Protocol(_))
        ));
        assert!(!writes
            .lock()
            .expect("mock output lock")
            .windows(raw.len())
            .any(|window| window == raw));
    }

    #[test]
    fn quotes_command_values_and_rejects_injection() {
        assert_eq!(quote_imap("a\\b\"c").expect("quoted"), "\"a\\\\b\\\"c\"");
        assert!(quote_imap("INBOX\r\nA9999 LOGOUT").is_err());
    }

    #[test]
    fn decodes_modified_utf7_without_panicking_on_invalid_input() {
        assert_eq!(decode_modified_utf7("&U,BTFw-/&ZeVnLIqe-"), "台北/日本語");
        assert_eq!(decode_modified_utf7("R&-D &- QA"), "R&D & QA");
        assert_eq!(decode_modified_utf7("broken &%%%-"), "broken &%%%-");
    }

    #[test]
    fn maps_capability_tokens_without_claiming_sent_auto_save() {
        let capabilities = capabilities_from_names(&[
            "IMAP4REV1".into(),
            "IDLE".into(),
            "MOVE".into(),
            "THREAD=REFERENCES".into(),
        ]);
        assert!(capabilities.idle_push);
        assert!(capabilities.r#move);
        assert!(capabilities.threading);
        assert!(capabilities.append);
        assert!(!capabilities.append_sent);
    }
}
