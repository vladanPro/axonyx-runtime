//! Bounded, process-local admission control before expensive login work.
//! Keys must be selected by trusted server policy, not blindly from proxy headers.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use thiserror::Error;

const MAX_KEY_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AxLoginThrottleError {
    #[error("invalid login throttle configuration")]
    InvalidConfiguration,
    #[error("invalid login throttle key")]
    InvalidKey,
    #[error("login attempts are limited")]
    Limited { retry_after: Duration },
    #[error("login throttle is unavailable")]
    Unavailable,
}

struct Bucket {
    start: Instant,
    attempts: u32,
}

pub struct AxLoginThrottle {
    attempts: u32,
    window: Duration,
    max_keys: usize,
    buckets: Mutex<HashMap<[u8; 32], Bucket>>,
}

impl AxLoginThrottle {
    pub fn new(
        attempts: u32,
        window: Duration,
        max_keys: usize,
    ) -> Result<Self, AxLoginThrottleError> {
        if attempts == 0 || window.is_zero() || max_keys == 0 {
            return Err(AxLoginThrottleError::InvalidConfiguration);
        }
        Ok(Self {
            attempts,
            window,
            max_keys,
            buckets: Mutex::new(HashMap::new()),
        })
    }

    /// Counts every admitted attempt, including successful logins. No password
    /// or session values should ever be used as keys. Capacity exhaustion fails closed.
    pub fn try_acquire(&self, key: &str) -> Result<(), AxLoginThrottleError> {
        self.acquire_at(key, Instant::now())
    }

    fn acquire_at(&self, key: &str, now: Instant) -> Result<(), AxLoginThrottleError> {
        if key.is_empty() || key.len() > MAX_KEY_BYTES {
            return Err(AxLoginThrottleError::InvalidKey);
        }
        let key: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        let mut buckets = self
            .buckets
            .lock()
            .map_err(|_| AxLoginThrottleError::Unavailable)?;
        buckets.retain(|_, bucket| now.saturating_duration_since(bucket.start) < self.window);
        if let Some(bucket) = buckets.get_mut(&key) {
            if bucket.attempts >= self.attempts {
                return Err(AxLoginThrottleError::Limited {
                    retry_after: self
                        .window
                        .saturating_sub(now.saturating_duration_since(bucket.start)),
                });
            }
            bucket.attempts += 1;
            return Ok(());
        }
        if buckets.len() >= self.max_keys {
            let retry_after = buckets
                .values()
                .map(|bucket| {
                    self.window
                        .saturating_sub(now.saturating_duration_since(bucket.start))
                })
                .min()
                .unwrap_or(self.window);
            return Err(AxLoginThrottleError::Limited { retry_after });
        }
        buckets.insert(
            key,
            Bucket {
                start: now,
                attempts: 1,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_counts_attempts_without_extending_on_rejection() {
        let throttle = AxLoginThrottle::new(2, Duration::from_secs(10), 4).unwrap();
        let now = Instant::now();
        assert_eq!(throttle.acquire_at("one", now), Ok(()));
        assert_eq!(throttle.acquire_at("one", now), Ok(()));
        assert_eq!(
            throttle.acquire_at("one", now + Duration::from_secs(3)),
            Err(AxLoginThrottleError::Limited {
                retry_after: Duration::from_secs(7)
            })
        );
        assert_eq!(throttle.acquire_at("two", now), Ok(()));
        assert_eq!(
            throttle.acquire_at("one", now + Duration::from_secs(10)),
            Ok(())
        );
    }

    #[test]
    fn bounded_capacity_fails_closed_and_reclaims_expired_keys() {
        let throttle = AxLoginThrottle::new(2, Duration::from_secs(10), 1).unwrap();
        let now = Instant::now();
        throttle.acquire_at("one", now).unwrap();
        assert!(matches!(
            throttle.acquire_at("two", now),
            Err(AxLoginThrottleError::Limited { .. })
        ));
        assert_eq!(throttle.buckets.lock().unwrap().len(), 1);
        assert_eq!(throttle.acquire_at("one", now), Ok(()));
        assert_eq!(
            throttle.acquire_at("two", now + Duration::from_secs(10)),
            Ok(())
        );
        assert_eq!(throttle.buckets.lock().unwrap().len(), 1);
    }

    #[test]
    fn validates_configuration_and_keys() {
        assert!(AxLoginThrottle::new(0, Duration::from_secs(1), 1).is_err());
        assert!(AxLoginThrottle::new(1, Duration::ZERO, 1).is_err());
        assert!(AxLoginThrottle::new(1, Duration::from_secs(1), 0).is_err());
        let throttle = AxLoginThrottle::new(1, Duration::from_secs(1), 1).unwrap();
        assert_eq!(
            throttle.try_acquire(""),
            Err(AxLoginThrottleError::InvalidKey)
        );
        assert_eq!(
            throttle.try_acquire(&"x".repeat(MAX_KEY_BYTES + 1)),
            Err(AxLoginThrottleError::InvalidKey)
        );
        assert!(throttle.buckets.lock().unwrap().is_empty());
    }

    #[test]
    fn concurrent_requests_cannot_exceed_the_budget() {
        let throttle =
            std::sync::Arc::new(AxLoginThrottle::new(5, Duration::from_secs(60), 1).unwrap());
        let threads = (0..32)
            .map(|_| {
                let throttle = throttle.clone();
                std::thread::spawn(move || throttle.try_acquire("same-key").is_ok())
            })
            .collect::<Vec<_>>();
        let allowed = threads
            .into_iter()
            .map(|thread| usize::from(thread.join().unwrap()))
            .sum::<usize>();
        assert_eq!(allowed, 5);
    }

    #[test]
    fn poisoned_state_fails_closed() {
        let throttle =
            std::sync::Arc::new(AxLoginThrottle::new(5, Duration::from_secs(60), 1).unwrap());
        let other = throttle.clone();
        let _ = std::thread::spawn(move || {
            let _guard = other.buckets.lock().unwrap();
            panic!("test lock poisoning");
        })
        .join();
        assert_eq!(
            throttle.try_acquire("same-key"),
            Err(AxLoginThrottleError::Unavailable)
        );
    }
}
