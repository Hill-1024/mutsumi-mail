#![allow(dead_code)] // Retry policy is exercised by the supervisor once network workers are enabled.

use std::time::Duration;

pub fn exponential_backoff(retry_count: u32) -> Duration {
    let capped = retry_count.min(8);
    Duration::from_secs(2_u64.saturating_pow(capped))
}

#[cfg(test)]
mod tests {
    use super::exponential_backoff;
    #[test]
    fn caps_backoff() {
        assert_eq!(exponential_backoff(20), std::time::Duration::from_secs(256));
    }
}
