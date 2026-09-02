use serde::{Deserialize, Serialize};

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingOperationState {
    Pending,
    Sending,
    Succeeded,
    Failed,
    Conflicted,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboxState {
    Queued,
    Sending,
    Sent,
    Failed,
    OutcomeUnknown,
    Cancelled,
}

#[allow(dead_code)]
impl OutboxState {
    pub fn can_retry(&self) -> bool {
        matches!(self, Self::Failed | Self::OutcomeUnknown)
    }
}

#[cfg(test)]
mod tests {
    use super::{OutboxState, PendingOperationState};

    #[test]
    fn outcome_unknown_is_not_silently_sent() {
        assert!(OutboxState::OutcomeUnknown.can_retry());
        assert_ne!(
            PendingOperationState::Failed,
            PendingOperationState::Succeeded
        );
    }
}
