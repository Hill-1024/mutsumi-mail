#![allow(dead_code)] // Threading policy is shared by IMAP/JMAP adapters as they land.

use crate::domain::message::normalize_subject;

pub fn thread_key(subject: &str, references: &[String], in_reply_to: Option<&str>) -> String {
    if let Some(reference) = references.last() {
        return format!("ref:{reference}");
    }
    if let Some(reference) = in_reply_to {
        return format!("ref:{reference}");
    }
    format!("subject:{}", normalize_subject(subject))
}
