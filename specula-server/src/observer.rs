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
//! The play host (`play.rs`) is run and noted the same way.

use std::io;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

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

pub fn supervise(host: &str, port: &str, socket: &str, hub: &Hub) {
    kill_recorded(socket);
    loop {
        match run(host, port, socket, hub) {
            Ok(outcome) => println!("observer {outcome}"),
            Err(error) => eprintln!("cannot run the observer: {error}"),
        }
        std::thread::sleep(RESTART_DELAY);
    }
}

fn run(host: &str, port: &str, socket: &str, hub: &Hub) -> io::Result<String> {
    let mut command = Command::new(OBSERVE_SCRIPT);
    command.arg(host).arg(port).arg(socket);
    run_engine(command, &format!("joining {host}:{port}"), socket, hub)
}

/// Runs an engine script until it exits or its feed goes quiet, and kills
/// its process group then; `what` names it in the log. The play host
/// (`play.rs`) runs through here too.
pub fn run_engine(mut command: Command, what: &str, socket: &str, hub: &Hub) -> io::Result<String> {
    let started = Instant::now();
    let mut child = command
        .stdin(Stdio::piped())
        // The engine writes its own logs; its stdout is a line per turn.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    println!("engine {} {what}", child.id());
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

fn kill_group(group: u32) {
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg("--")
        .arg(format!("-{group}"))
        .stderr(Stdio::null())
        .status();
}
