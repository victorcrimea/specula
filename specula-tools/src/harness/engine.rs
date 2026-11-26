//! One match in its own engine, made safe the way `convert.rs` makes the
//! archive builder: "e" on stdin, its own process group and a watchdog.

use crate::setup::Setup;
use serde_json::Value;
use specula_tools::convert::group_resident_bytes;
use specula_tools::convert::kill_group;
use specula_tools::convert::log_tail;
use specula_tools::convert::read_from;
use std::fs;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// The engine's error dialog on a terminal-less run.
const ERROR_PROMPT: &str = "(C)ontinue, (S)uppress";
const RESULT_PREFIX: &str = "SPECULA-HARNESS ";
const POLL: Duration = Duration::from_millis(200);
/// How long a finished engine gets to write its replay and exit.
const EXIT_GRACE: Duration = Duration::from_secs(60);
/// A runaway is killed before it takes the machine down.
const MAX_RESIDENT_BYTES: u64 = 12 << 30;
const MAX_LOG_BYTES: u64 = 256 << 20;

pub struct Options {
    /// Game seconds.
    pub time_limit: f64,
    pub wall_limit: Duration,
    pub work_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub keep: bool,
}

#[derive(Clone)]
pub struct PlayerResult {
    pub id: u64,
    pub state: String,
    pub economy: f64,
    pub military: f64,
    pub exploration: f64,
    pub total: f64,
}

#[derive(Clone)]
pub struct MatchResult {
    pub time: f64,
    pub timed_out: bool,
    pub players: Vec<PlayerResult>,
    pub wall: Duration,
    pub peak_resident: u64,
}

/// The first match to run fills the shared cache; the others wait for it,
/// as the engine rewrites cache files in place and a second engine could
/// read one half-written.
static WARMING: Mutex<()> = Mutex::new(());

pub fn run_match(setup: &Setup, options: &Options) -> Result<MatchResult, String> {
    let marker = options.cache_dir.join(".warm");
    if marker.exists() {
        return run(setup, options);
    }
    let _warming = WARMING.lock().unwrap_or_else(|e| e.into_inner());
    let result = run(setup, options);
    if !marker.exists() {
        let _ = fs::create_dir_all(&options.cache_dir);
        let _ = fs::write(&marker, "");
    }
    result
}

fn run(setup: &Setup, options: &Options) -> Result<MatchResult, String> {
    let absolute = |path: &Path| {
        std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let work = absolute(&options.work_dir)?;
    let cache = absolute(&options.cache_dir)?;
    let config = work.join("config");
    let settings_dir = config.join("0ad").join("config");
    let state = work.join("state");
    let data = work.join("data");
    for directory in [&settings_dir, &state, &data, &cache] {
        fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    }
    fs::write(
        settings_dir.join("matchsettings.json"),
        setup.value.to_string(),
    )
    .map_err(|error| format!("cannot write the settings: {error}"))?;

    let pyrogenesis = match std::env::var_os("PYROGENESIS") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from("../0ad-specula/binaries/system/pyrogenesis"),
    };
    let pyrogenesis = absolute(&pyrogenesis)?;
    let stdout_path = work.join("stdout.log");
    let stderr_path = work.join("stderr.log");
    let open = |path: &Path| {
        File::create(path).map_err(|error| format!("cannot create {}: {error}", path.display()))
    };
    let mut child = Command::new(&pyrogenesis)
        .arg(format!("-autostart={}", setup.autostart_map()?))
        .arg("-autostart-specula-match")
        .arg("-autostart-specula-settings=config/matchsettings.json")
        .arg("-autostart-nonvisual")
        .arg("-autostart-player=-1")
        .arg("-unique-logs")
        .env("XDG_DATA_HOME", &data)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CACHE_HOME", &cache)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(open(&stderr_path)?)
        .process_group(0)
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", pyrogenesis.display()))?;
    let started = Instant::now();
    // Answers every prompt with exit; dropping the pipe closes it.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&b"e\n".repeat(64));
    }

    // The engine prints a line per turn too; only the last result line is
    // kept, the rest goes to the log.
    let last: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stdout_log = open(&stdout_path)?;
    let reader = {
        let last = Arc::clone(&last);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(json) = line.strip_prefix(RESULT_PREFIX)
                    && let Ok(value) = serde_json::from_str(json)
                {
                    *last.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
                }
                let _ = writeln!(stdout_log, "{line}");
            }
        })
    };

    let outcome = watch(
        &mut child,
        &last,
        &stderr_path,
        &stdout_path,
        options,
        started,
    );
    // Whatever the engine forked must not outlive it.
    kill_group(child.id());
    let _ = child.wait();
    let _ = reader.join();
    let (value, timed_out, peak_resident) = match outcome {
        Ok(found) => found,
        Err(error) => {
            // The work dir stays, as the error names its logs.
            return Err(format!("{error} (logs in {})", work.display()));
        }
    };
    let result = MatchResult {
        time: value["time"].as_f64().unwrap_or(0.0),
        timed_out,
        players: value["players"]
            .as_array()
            .map(|players| players.iter().map(player_result).collect())
            .unwrap_or_default(),
        wall: started.elapsed(),
        peak_resident,
    };
    if !options.keep {
        let _ = fs::remove_dir_all(&work);
    }
    Ok(result)
}

fn player_result(value: &Value) -> PlayerResult {
    let number = |key: &str| value[key].as_f64().unwrap_or(0.0);
    PlayerResult {
        id: value["id"].as_u64().unwrap_or(0),
        state: value["state"].as_str().unwrap_or("").to_string(),
        economy: number("economy"),
        military: number("military"),
        exploration: number("exploration"),
        total: number("total"),
    }
}

/// Waits for a final line, or stops the engine at the game time limit.
/// Returns the result, whether it timed out, and the peak memory.
fn watch(
    child: &mut std::process::Child,
    last: &Mutex<Option<Value>>,
    stderr_path: &Path,
    stdout_path: &Path,
    options: &Options,
    started: Instant,
) -> Result<(Value, bool, u64), String> {
    let mut checked: u64 = 0;
    let mut peak = 0;
    let mut finished_at: Option<Instant> = None;
    loop {
        let line = last.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let exited = child
            .try_wait()
            .map_err(|error| format!("cannot wait for the engine: {error}"))?;
        if let Some(line) = &line {
            if line["final"].as_bool() == Some(true) {
                // The engine quits by itself once the game is decided.
                let at = *finished_at.get_or_insert_with(Instant::now);
                if exited.is_some() || at.elapsed() > EXIT_GRACE {
                    return Ok((line.clone(), false, peak));
                }
            } else if line["time"].as_f64().unwrap_or(0.0) >= options.time_limit {
                return Ok((line.clone(), true, peak));
            }
        }
        if let Some(status) = exited {
            return Err(format!(
                "the engine exited ({status}) without a result:\n{}{}",
                log_tail(stderr_path),
                log_tail(stdout_path)
            ));
        }
        if started.elapsed() > options.wall_limit {
            return Err(format!(
                "the engine ran over {} s of wall time and was killed",
                options.wall_limit.as_secs()
            ));
        }
        let size = fs::metadata(stderr_path).map_or(0, |metadata| metadata.len());
        let start = checked.saturating_sub(ERROR_PROMPT.len() as u64);
        let prompted = read_from(stderr_path, start).contains(ERROR_PROMPT);
        checked = size;
        if prompted {
            return Err(format!(
                "the engine stopped at an assertion and was killed:\n{}",
                log_tail(stderr_path)
            ));
        }
        let logs = size + fs::metadata(stdout_path).map_or(0, |metadata| metadata.len());
        if logs > MAX_LOG_BYTES {
            return Err(format!("its logs passed {} MB", MAX_LOG_BYTES >> 20));
        }
        let resident = group_resident_bytes(child.id());
        peak = peak.max(resident);
        if resident > MAX_RESIDENT_BYTES {
            return Err(format!(
                "the engine passed {} GB of memory and was killed",
                MAX_RESIDENT_BYTES >> 30
            ));
        }
        std::thread::sleep(POLL);
    }
}
