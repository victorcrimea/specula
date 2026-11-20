//! The hosting configuration: how many matches the server runs at once,
//! their ports, and the limits shared by all of them.

use std::path::PathBuf;

/// Relative to the working directory, as `WEB_ROOT`.
pub const DEFAULT_ENGINE_DIR: &str = "engines";
/// The engine's default port is taken by any 0 A.D. hosted on this machine.
pub const DEFAULT_PORT_BASE: u16 = 20777;
pub const DEFAULT_SLOTS: u16 = 1;
pub const DEFAULT_MAX_OBSERVERS: u32 = 100;

pub struct Config {
    /// Matches hosted at once; 0 hosts none (`--observe`, or a feed
    /// started by hand).
    pub slots: u16,
    /// Slot `i` hosts its game on UDP port `port_base + i`.
    pub port_base: u16,
    /// Observer connections summed over every match; 0 refuses them all.
    pub max_observers: u32,
    pub admin_key: Option<String>,
    /// Absolute: the engine reads a relative XDG path as relative to $HOME
    /// (`Paths::XDG_Path`).
    pub engine_dir: PathBuf,
}

impl Config {
    /// The port of slot `index`; `parse_args` has checked it fits.
    pub fn port(&self, index: u16) -> u16 {
        self.port_base + index
    }

    /// One line for the startup log.
    pub fn summary(&self) -> String {
        let hosting = match self.slots {
            0 => "hosting off".to_string(),
            1 => format!("hosting 1 match on port {}", self.port_base),
            slots => format!(
                "hosting up to {slots} matches on ports {} to {}",
                self.port_base,
                self.port(slots - 1)
            ),
        };
        let admin = if self.admin_key.is_some() {
            "admin key set"
        } else {
            "admin key off (no --admin-key)"
        };
        format!(
            "{hosting}, at most {} observers, {admin}",
            self.max_observers
        )
    }
}
