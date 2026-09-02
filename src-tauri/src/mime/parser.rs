#![allow(dead_code)] // Parser is ready for the first network-backed fetch slice.

use mail_parser::MessageParser;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub subject: Option<String>,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub in_reply_to: Option<String>,
    pub text: Option<String>,
    pub html_text: Option<String>,
    pub attachment_count: usize,
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
    let html_text = message
        .body_html(0)
        .map(|value| crate::mime::sanitizer::html_to_safe_text(value.as_ref()));
    Some(ParsedMessage {
        subject,
        message_id,
        references,
        in_reply_to,
        text,
        html_text,
        attachment_count: message.attachments().count(),
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
}
