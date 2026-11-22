//! How often flares may be sent, matching vanilla's receipt limit
//! (`g_FlareRateLimitScope` and `g_FlareRateLimitMaximumFlares` in
//! `gui/session/messages.js`). The server checks it as well, since a
//! client's own limit is no defence.

use std::time::Duration;

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
