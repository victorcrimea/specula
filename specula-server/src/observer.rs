//! Keeps an observer joined to the game at HOST:PORT. A match ends when the
//! host goes away (the demo restarts its host every 2 h) or a player wins,
//! and the observer then exits, so it is started again and again; whatever
//! game the host runs next is the next match.
//!
//! The engine answers a failed assertion with a prompt on stdin that, on
//! EOF, forks a copy of itself (see `specula-tools/src/convert.rs`), so
//! stdin answers "(E)xit" and the observer runs in its own process group,
//! which is killed as a whole.
//!
//! That group outlives a server that dies without cleaning up, and the
//! orphan would join the next game too and feed this server's socket, so
//! the group is noted in `<socket>.observer` and killed on the next start.
//! The play host (`play.rs`) is run and noted the same way, one pid file per
//! match socket.
//!
//! Each run writes its logs and cache under the engine dir (`EngineDirs`):
//! the crash log and `system_info.txt` have fixed names, so engines sharing
//! a log dir would overwrite each other's, and the engine rewrites cache
//! files in place, so a second engine could read one half-written.

use std::io;
use std::io::Write;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use crate::registry::MatchId;
use crate::view::Hub;

/// Relative to the working directory, like the web root.
const OBSERVE_SCRIPT: &str = "scripts/observe.sh";
/// Until the feed connects. The engine's join loop ignores a refused or
/// dropped connection, so this is what notices the host is not up yet.
/// The feed connects as the game starts, before a late joiner downloads
/// the game state, so a long-running game does not need longer.
const JOIN_LIMIT: Duration = Duration::from_secs(30);
/// Between turns once the feed is up. Long enough to download and catch up
/// on a game that has run for hours; a game paused through the feed sends
/// keepalives instead of turns.
const SILENCE_LIMIT: Duration = Duration::from_secs(120);
/// The restarted host refuses connections for a while; each failed join
/// costs the engine's startup.
const RESTART_DELAY: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_secs(1);

/// Run log dirs kept under `<engine dir>/logs`, newest first.
const KEEP_LOGS: usize = 20;

/// Where one engine run writes. Each run has its own log dir, so a crash
/// log or `system_info.txt` is never another engine's; each slot (and the
/// observer) has its own cache, because the engine rewrites cache files in
/// place and a second engine could read one half-written.
pub struct EngineDirs {
    state: PathBuf,
    cache: PathBuf,
    /// Set for a hosted match, whose settings file lives here.
    config: Option<PathBuf>,
}

impl EngineDirs {
    /// `root` is `Config::engine_dir`; `run` names the log dir and `cache`
    /// the cache dir ("slot-0", "observe").
    pub fn new(root: &Path, run: MatchId, cache: &str) -> Self {
        EngineDirs {
            state: root.join("logs").join(run.to_string()),
            cache: root.join("cache").join(cache),
            config: None,
        }
    }

    /// Gives the run a config dir of its own, which the engine reads the
    /// setup's settings from (M20); the `--observe` engine keeps the user's.
    /// It is under the run's log dir, so `prune_logs` removes it with them.
    pub fn with_settings(mut self) -> Self {
        self.config = Some(self.state.join("config"));
        self
    }

    /// Where the engine finds the settings as `config/matchsettings.mp.json`
    /// in its VFS: `XDG_CONFIG_HOME/0ad/config/` is mounted there
    /// (`Paths.cpp`, `GameSetup.cpp`), and it is one of the few files the
    /// GUI compartment may read (`JSInterface_VFS.cpp`).
    pub fn settings_file(&self) -> Option<PathBuf> {
        Some(
            self.config
                .as_ref()?
                .join("0ad")
                .join("config")
                .join("matchsettings.mp.json"),
        )
    }

    /// For the log line.
    pub fn logs(&self) -> PathBuf {
        self.state.join("0ad").join("log")
    }
}

/// Deletes run log dirs beyond the newest `KEEP_LOGS`, never one in
/// `live`. Only dirs named by a UUID are touched, so nothing else a user
/// put there is lost.
pub fn prune_logs(root: &Path, live: &[MatchId]) {
    let Ok(entries) = std::fs::read_dir(root.join("logs")) else {
        return;
    };
    let mut runs: Vec<(std::time::SystemTime, MatchId, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let id = MatchId::parse(entry.file_name().to_str()?)?;
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, id, entry.path()))
        })
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.0));
    for (_, id, path) in runs.into_iter().skip(KEEP_LOGS) {
        if live.contains(&id) {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(&path) {
            eprintln!("cannot remove engine logs {}: {error}", path.display());
        }
    }
}

pub fn supervise(host: &str, port: &str, socket: &str, engine_dir: &Path, hub: &Hub) {
    kill_recorded(socket);
    loop {
        match run(host, port, socket, engine_dir, hub) {
            Ok(outcome) => println!("observer {outcome}"),
            Err(error) => eprintln!("cannot run the observer: {error}"),
        }
        std::thread::sleep(RESTART_DELAY);
    }
}

fn run(host: &str, port: &str, socket: &str, engine_dir: &Path, hub: &Hub) -> io::Result<String> {
    let run_id = MatchId::new_v4();
    prune_logs(engine_dir, &[run_id]);
    let dirs = EngineDirs::new(engine_dir, run_id, "observe");
    let mut command = Command::new(OBSERVE_SCRIPT);
    command.arg(host).arg(port).arg(socket);
    run_engine(
        command,
        &format!("joining {host}:{port}"),
        socket,
        &dirs,
        hub,
    )
}

/// Runs an engine script until it exits or its feed goes quiet, and kills
/// its process group then; `what` names it in the log. The play host
/// (`play.rs`) runs through here too.
pub fn run_engine(
    mut command: Command,
    what: &str,
    socket: &str,
    dirs: &EngineDirs,
    hub: &Hub,
) -> io::Result<String> {
    std::fs::create_dir_all(&dirs.state)?;
    std::fs::create_dir_all(&dirs.cache)?;
    command
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CACHE_HOME", &dirs.cache);
    if let Some(config) = &dirs.config {
        // A per-run config dir, as the file's only reader is the engine.
        std::fs::create_dir_all(config)?;
        command.env("XDG_CONFIG_HOME", config);
    }
    let started = Instant::now();
    let mut child = command
        .stdin(Stdio::piped())
        // The engine writes its own logs; its stdout is a line per turn.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    println!(
        "engine {} {what}, logs in {}",
        child.id(),
        dirs.logs().display()
    );
    if let Err(error) = std::fs::write(pid_path(socket), child.id().to_string()) {
        eprintln!("cannot note the observer in {}: {error}", pid_path(socket));
    }
    // Answers every prompt with exit; dropping the pipe closes it.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&b"e\n".repeat(64));
    }
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(format!("exited ({status})")),
            Ok(None) => {}
            Err(error) => break Err(error),
        }
        // Covers both a join that never reaches the game and a game that
        // stopped sending turns without the engine noticing.
        let (active, limit) = match hub.feed_activity().filter(|at| *at > started) {
            Some(at) => (at, SILENCE_LIMIT),
            None => (started, JOIN_LIMIT),
        };
        if active.elapsed() > limit {
            break Ok(format!("silent for {} s, killed", limit.as_secs()));
        }
        std::thread::sleep(POLL);
    };
    // Whatever the observer forked must not outlive it.
    kill_group(child.id());
    let _ = child.wait();
    let _ = std::fs::remove_file(pid_path(socket));
    outcome
}

fn pid_path(socket: &str) -> String {
    format!("{socket}.observer")
}

/// Kills the observer noted for this socket, if it is still running.
pub fn kill_recorded(socket: &str) {
    let path = pid_path(socket);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    if let Ok(pid) = text.trim().parse::<u32>() {
        // The pid may belong to something else by now.
        let is_engine = std::fs::read(format!("/proc/{pid}/cmdline"))
            .is_ok_and(|cmdline| cmdline.windows(11).any(|word| word == b"pyrogenesis"));
        if is_engine {
            println!("killing observer {pid}");
            kill_group(pid);
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// Kills the engines and removes the sockets of matches `<socket_base>.<uuid>`
/// that a dead server left. They are named by UUIDs it can no longer list.
pub fn kill_stale(socket_base: &str) {
    let base = std::path::Path::new(socket_base);
    let (Some(dir), Some(name)) = (base.parent(), base.file_name()) else {
        return;
    };
    let dir = if dir.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        dir
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = file.strip_prefix(&prefix) else {
            continue;
        };
        let (id, pid_file) = match rest.strip_suffix(".observer") {
            Some(id) => (id, true),
            None => (rest, false),
        };
        if MatchId::parse(id).is_none() {
            continue;
        }
        let path = entry.path();
        if pid_file {
            kill_recorded(path.to_string_lossy().trim_end_matches(".observer"));
        } else if entry.file_type().is_ok_and(|kind| kind.is_socket()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn kill_group(group: u32) {
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg("--")
        .arg(format!("-{group}"))
        .stderr(Stdio::null())
        .status();
}
