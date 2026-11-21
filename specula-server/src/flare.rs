//! How often flares may be sent, matching vanilla's receipt limit
//! (`g_FlareRateLimitScope` and `g_FlareRateLimitMaximumFlares` in
//! `gui/session/messages.js`). The server checks it as well, since a
//! client's own limit is no defence.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

pub const SCOPE: Duration = Duration::from_secs(6);
pub const MAXIMUM: usize = 16;

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Sender {
    Seat(u32),
    Visitor(String),
    /// Shared by every observer of a match: a per-cookie limit alone is not
    /// reliable.
    Observers,
}

#[derive(Default)]
pub struct Limiter {
    sent: HashMap<Sender, VecDeque<Instant>>,
}

impl Limiter {
    pub fn allow(&mut self, keys: &[Sender], now: Instant) -> bool {
        self.sent.retain(|_, times| {
            while times
                .front()
                .is_some_and(|at| now.duration_since(*at) > SCOPE)
            {
                times.pop_front();
            }
            !times.is_empty()
        });
        if keys.iter().any(|key| {
            self.sent
                .get(key)
                .is_some_and(|times| times.len() >= MAXIMUM)
        }) {
            return false;
        }
        for key in keys {
            self.sent.entry(key.clone()).or_default().push_back(now);
        }
        true
    }
}
