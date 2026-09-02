#![allow(dead_code)] // Queue DTO is the stable shape used by future worker persistence.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueuedOperation {
    pub id: String,
    pub operation_type: String,
    pub payload_json: String,
    pub retry_count: u32,
}

impl QueuedOperation {
    pub fn is_idempotent(&self) -> bool {
        matches!(
            self.operation_type.as_str(),
            "set_flags" | "move" | "archive" | "trash"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::QueuedOperation;
    #[test]
    fn flag_operations_are_idempotent() {
        assert!(QueuedOperation {
            id: "x".into(),
            operation_type: "set_flags".into(),
            payload_json: "{}".into(),
            retry_count: 0
        }
        .is_idempotent());
    }
}
