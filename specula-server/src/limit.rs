//! A sliding-window rate limit per key, shared by flares and chat. A request
//! passes only when every key it is charged to is under its maximum, and
//! then counts against all of them, so a refused request never spends a
//! shared budget.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::Hash;
use std::time::Duration;
use std::time::Instant;

pub struct Limiter<K> {
    scope: Duration,
    sent: HashMap<K, VecDeque<Instant>>,
}

impl<K: Clone + Eq + Hash> Limiter<K> {
    pub fn new(scope: Duration) -> Self {
        Limiter {
            scope,
            sent: HashMap::new(),
        }
    }

    /// `keys` pairs each key with its maximum within the scope.
    pub fn allow(&mut self, keys: &[(K, usize)], now: Instant) -> bool {
        let scope = self.scope;
        self.sent.retain(|_, times| {
            while times
                .front()
                .is_some_and(|at| now.duration_since(*at) > scope)
            {
                times.pop_front();
            }
            !times.is_empty()
        });
        if keys.iter().any(|(key, maximum)| {
            self.sent
                .get(key)
                .is_some_and(|times| times.len() >= *maximum)
        }) {
            return false;
        }
        for (key, _) in keys {
            self.sent.entry(key.clone()).or_default().push_back(now);
        }
        true
    }
}
