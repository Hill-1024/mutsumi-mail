use serde::{Deserialize, Serialize};

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SyncCursor {
    Imap {
        uid_validity: u32,
        last_uid: u32,
        highest_modseq: Option<u64>,
    },
    Pop3 {
        uidls: Vec<String>,
    },
    Gmail {
        history_id: String,
    },
    Graph {
        delta_link: String,
    },
    Jmap {
        state: String,
    },
}

#[allow(dead_code)]
impl SyncCursor {
    pub fn reset_for_uid_validity(&self, uid_validity: u32) -> Self {
        match self {
            Self::Imap { .. } => Self::Imap {
                uid_validity,
                last_uid: 0,
                highest_modseq: None,
            },
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SyncCursor;

    #[test]
    fn uid_validity_reset_does_not_reuse_old_uid() {
        let cursor = SyncCursor::Imap {
            uid_validity: 4,
            last_uid: 120,
            highest_modseq: Some(9),
        };
        assert_eq!(
            cursor.reset_for_uid_validity(5),
            SyncCursor::Imap {
                uid_validity: 5,
                last_uid: 0,
                highest_modseq: None
            }
        );
    }
}
