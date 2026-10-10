use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// Brute-force lock for POST /api/auth, ported from the reference server:
/// after `max_failures` consecutive failures an identity is locked for
/// `lock_duration`; counters expire after `window` of inactivity; any success
/// clears the counter. Both the device id (X-Device-Id) and the client IP are
/// tracked so rotating device ids can't bypass the lock.
pub struct LoginLimiter {
    entries: HashMap<String, Entry>,
    max_failures: u32,
    lock_duration: Duration,
    window: Duration,
}

#[derive(Default)]
struct Entry {
    failures: u32,
    locked_until: Option<Instant>,
    last_seen: Option<Instant>,
}

pub struct RequestIdentity {
    pub ip: Option<SocketAddr>,
    pub device_id: Option<String>,
}

impl RequestIdentity {
    /// Identity keys: `ip:` always when known, `dev:` when the client sends it.
    fn keys(&self) -> Vec<String> {
        let mut keys = Vec::with_capacity(2);
        if let Some(addr) = self.ip {
            keys.push(format!("ip:{}", addr.ip()));
        }
        if let Some(dev) = &self.device_id {
            keys.push(format!("dev:{dev}"));
        }
        keys
    }
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_failures: 3,
            lock_duration: Duration::from_secs(15 * 60),
            window: Duration::from_secs(60 * 60),
        }
    }

    /// May a login attempt proceed? Returns `Err(remaining_lock)` when locked.
    pub fn allow(&mut self, id: &RequestIdentity) -> Result<(), Duration> {
        let now = Instant::now();
        let mut remaining = Duration::ZERO;
        for key in id.keys() {
            self.expire(&key, now);
            if let Some(e) = self.entries.get(&key) {
                if let Some(locked_until) = e.locked_until {
                    if locked_until > now {
                        remaining = remaining.max(locked_until - now);
                    }
                }
            }
        }
        if remaining.is_zero() {
            for key in id.keys() {
                self.entries.entry(key).or_default().last_seen = Some(now);
            }
            Ok(())
        } else {
            Err(remaining)
        }
    }

    pub fn record_failure(&mut self, id: &RequestIdentity) {
        let now = Instant::now();
        for key in id.keys() {
            let e = self.entries.entry(key).or_default();
            e.failures += 1;
            e.last_seen = Some(now);
            if e.failures >= self.max_failures {
                e.locked_until = Some(now + self.lock_duration);
            }
        }
    }

    pub fn record_success(&mut self, id: &RequestIdentity) {
        for key in id.keys() {
            if let Some(e) = self.entries.get_mut(&key) {
                e.failures = 0;
                e.locked_until = None;
                e.last_seen = Some(Instant::now());
            }
        }
    }

    /// Drop counters idle beyond the window, and expired locks.
    fn expire(&mut self, key: &str, now: Instant) {
        let stale = match self.entries.get(key) {
            Some(e) => {
                let idle = e
                    .last_seen
                    .map(|s| now.duration_since(s) > self.window)
                    .unwrap_or(true);
                let unlocked = e.locked_until.map(|u| u <= now).unwrap_or(true);
                idle && unlocked
            }
            None => false,
        };
        if stale {
            self.entries.remove(key);
        }
    }
}

impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new()
    }
}
