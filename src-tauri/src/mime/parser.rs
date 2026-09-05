#![allow(dead_code)] // Parser is ready for the first network-backed fetch slice.

use mail_parser::{MessageParser, MimeHeaders};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub subject: Option<String>,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub in_reply_to: Option<String>,
    pub text: Option<String>,
    pub html_text: Option<String>,
    pub preview: String,
    pub attachment_count: usize,
    pub attachments: Vec<ParsedAttachment>,
}

pub fn parse_rfc822(raw: &[u8]) -> Option<ParsedMessage> {
    let message = MessageParser::default().parse(raw)?;
    let subject = message.subject().map(|value| value.to_string());
    let message_id = message.message_id().map(|value| value.to_string());
    let references = message
        .references()
        .as_text_list()
        .map(|values| values.iter().map(ToString::to_string).collect())
        .or_else(|| {
            message
                .references()
                .as_text()
                .map(|value| vec![value.to_string()])
        })
        .unwrap_or_default();
    let in_reply_to = message.in_reply_to().as_text().map(ToString::to_string);
    let text = message.body_text(0).map(|value| value.into_owned());
    // The web layer performs a strict DOM allow-list pass before rendering. Keeping the HTML
    // here preserves layout while the CSP remains the final network/script boundary.
    let html_text = message.body_html(0).map(|value| value.into_owned());
    let preview = message
        .body_preview(240)
        .map(|value| value.into_owned())
        .unwrap_or_default();
    let attachments = message
        .attachments()
        .filter(|part| !part.is_message())
        .enumerate()
        .map(|(index, part)| {
            let content_type = part
                .content_type()
                .map(|kind| {
                    format!(
                        "{}/{}",
                        kind.ctype(),
                        kind.subtype().unwrap_or("octet-stream")
                    )
                })
                .unwrap_or_else(|| "application/octet-stream".into());
            ParsedAttachment {
                filename: part
                    .attachment_name()
                    .filter(|name| !name.trim().is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("attachment-{}", index + 1)),
                content_type,
                bytes: part.contents().to_vec(),
            }
        })
        .collect::<Vec<_>>();
    Some(ParsedMessage {
        subject,
        message_id,
        references,
        in_reply_to,
        text,
        html_text,
        preview,
        attachment_count: attachments.len(),
        attachments,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_rfc822;

    #[test]
    fn parses_multipart_like_plain_message_without_manual_header_splitting() {
        let raw = b"From: sender@example.com\r\nSubject: Hello\r\nMessage-ID: <abc@example.com>\r\n\r\nHello body";
        let parsed = parse_rfc822(raw).expect("parsed");
        assert_eq!(parsed.subject.as_deref(), Some("Hello"));
        assert_eq!(parsed.text.as_deref(), Some("Hello body"));
    }

    #[test]
    fn preserves_html_and_decodes_attachment_bytes() {
        let raw = b"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/html\r\n\r\n<p>Hello</p>\r\n--x\r\nContent-Type: text/plain; name=note.txt\r\nContent-Disposition: attachment; filename=note.txt\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--x--\r\n";
        let parsed = parse_rfc822(raw).expect("parsed MIME message");
        assert_eq!(parsed.html_text.as_deref(), Some("<p>Hello</p>"));
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].filename, "note.txt");
        assert_eq!(parsed.attachments[0].bytes, b"hello");
    }
}
