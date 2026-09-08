// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use crate::error::Result;
use std::future::Future;
use std::time::Duration;

const MAX_ATTEMPTS: u32 = 3;
const BASE_DELAY_MS: u64 = 250;

/// Retries a fallible async operation up to `MAX_ATTEMPTS` times with exponential backoff
/// (250ms, 500ms, ...). Intended only for transient outbound HTTP calls (SugarWOD/Hevy).
/// AWS SDK calls already retry internally.
pub async fn retry_with_backoff<F, Fut, T>(mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) if attempt < MAX_ATTEMPTS => {
                let delay = BASE_DELAY_MS * 2u64.pow(attempt - 1);
                tracing::warn!(attempt, %err, "retrying after transient error");
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SwhError;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn succeeds_after_transient_failures() {
        let attempts = AtomicU32::new(0);
        let result = retry_with_backoff(|| {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(SwhError::SugarWodFetch("transient".into()))
                } else {
                    Ok(42)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let attempts = AtomicU32::new(0);
        let result: Result<()> = retry_with_backoff(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            async move { Err(SwhError::SugarWodFetch("persistent".into())) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), MAX_ATTEMPTS);
    }
}
