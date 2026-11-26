//! Has the engine convert COLLADA to its own PMD/PSA and textures to its
//! cached DDS, so the skeleton mapping in art/skeletons, the converter's
//! axis and bone handling and the textures.xml settings stay the engine's.
//! `-archivebuild` converts only what is under the directory it is given,
//! so a staging copy of just the needed files plus art/skeletons keeps it
//! to a fraction of a second for one unit.
//!
//! A texture the converter cannot decode fails an `ENSURE` in
//! `CArchiveBuilder::Build`. The engine then asks "(C)ontinue, (S)uppress,
//! ..." on stdin, and on EOF `udbg_launch_debugger` forks; without gdb the
//! fork carries on as a second engine and asserts again, a fork bomb that
//! once took all of a 43 GB machine. So stdin answers "(E)xit", which
//! aborts, and the builder runs in its own process group that a watchdog
//! measures and kills as a whole.

use std::fs;
use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

/// The engine's error dialog on a terminal-less run.
const ERROR_PROMPT: &str = "(C)ontinue, (S)uppress";
/// Far above a whole-game build's log, far below a looping prompt's.
const MAX_LOG_BYTES: u64 = 256 << 20;
/// A whole-game build stays well under this; a runaway is killed before
/// it takes the machine down.
const MAX_RESIDENT_BYTES: u64 = 8 << 30;
const POLL: Duration = Duration::from_millis(200);
/// How much of the log an error quotes.
const LOG_TAIL_BYTES: u64 = 4096;

/// Returns the directory the converted files were unpacked into and the
/// sources the engine did not convert; each source is there under
/// `cached_path`. Sources that are not converted, such as textures.xml,
/// are only staged. Each source, and art/skeletons, is taken from the
/// first of `mod_dirs` that has it, as the engine's VFS lets a mod
/// override the ones it depends on.
pub fn run(
    mod_dirs: &[PathBuf],
    pyrogenesis: &Path,
    work_dir: &Path,
    sources: &[String],
) -> Result<(PathBuf, Vec<String>), String> {
    let stage = work_dir.join("stage");
    let archive = work_dir.join("models.zip");
    let converted = work_dir.join("converted");
    for directory in [&stage, &converted] {
        if directory.exists() {
            fs::remove_dir_all(directory)
                .map_err(|error| format!("cannot clear {}: {error}", directory.display()))?;
        }
    }
    if archive.exists() {
        fs::remove_file(&archive)
            .map_err(|error| format!("cannot remove {}: {error}", archive.display()))?;
    }

    let find = |path: &str| {
        mod_dirs
            .iter()
            .map(|dir| dir.join(path))
            .find(|path| path.exists())
            .ok_or_else(|| format!("{path}: in none of the mods"))
    };
    copy_directory(&find("art/skeletons")?, &stage.join("art/skeletons"))?;
    for source in sources {
        let target = stage.join(source);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        fs::copy(find(source)?, &target).map_err(|error| format!("{source}: {error}"))?;
    }

    // The engine resolves relative paths against its own directories.
    let absolute = |path: &Path| {
        std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let log_path = work_dir.join("archivebuild.log");
    let log = File::create(&log_path)
        .map_err(|error| format!("cannot create {}: {error}", log_path.display()))?;
    let log_err = log
        .try_clone()
        .map_err(|error| format!("{}: {error}", log_path.display()))?;
    let mut child = Command::new(pyrogenesis)
        .arg(format!("-archivebuild={}", absolute(&stage)?.display()))
        .arg(format!(
            "-archivebuild-output={}",
            absolute(&archive)?.display()
        ))
        .stdin(Stdio::piped())
        .stdout(log)
        .stderr(log_err)
        .process_group(0)
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", pyrogenesis.display()))?;
    // Answers every prompt with exit; dropping the pipe closes it.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&b"e\n".repeat(64));
    }
    let result = watch(&mut child, &log_path);
    // Whatever the builder forked must not outlive it.
    kill_group(child.id());
    let status = result?;
    if !status.success() || !archive.exists() {
        return Err(format!(
            "the archive builder failed ({status}):\n{}",
            log_tail(&log_path)
        ));
    }

    let status = Command::new("unzip")
        .arg("-q")
        .arg(&archive)
        .arg("-d")
        .arg(&converted)
        .status()
        .map_err(|error| format!("cannot run unzip: {error}"))?;
    if !status.success() {
        return Err(format!("unzip failed ({status})"));
    }

    // The builder drops a file it cannot convert instead of failing.
    let missing = sources
        .iter()
        .filter(|source| is_converted(source) && !cached_path(&converted, source).exists())
        .cloned()
        .collect();
    Ok((converted, missing))
}

/// `CCacheLoader::ArchiveCachePath` with the extension of the manager
/// that converts the source.
pub fn cached_path(converted: &Path, source: &str) -> PathBuf {
    let kind = if source.starts_with("art/animation/") {
        "psa"
    } else if source.starts_with("art/textures/") {
        "dds"
    } else {
        "pmd"
    };
    converted.join(format!("{source}.cached.{kind}"))
}

/// What `CArchiveBuilder::Run` converts rather than copies: DAE under
/// art/meshes and art/animation, and textures under art/textures.
/// Both compare extensions case-sensitively.
fn is_converted(source: &str) -> bool {
    if source.starts_with("art/textures/") {
        [".png", ".dds", ".tga", ".bmp"]
            .iter()
            .any(|extension| source.ends_with(extension))
    } else {
        source.ends_with(".dae")
    }
}

/// Waits for the builder, killing it on the engine's error prompt, a
/// runaway log or runaway memory.
fn watch(child: &mut Child, log_path: &Path) -> Result<std::process::ExitStatus, String> {
    let mut checked: u64 = 0;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for the archive builder: {error}"))?
        {
            return Ok(status);
        }
        let size = fs::metadata(log_path).map_or(0, |metadata| metadata.len());
        // Read only what was added, from a little before so a prompt split
        // across two reads is still seen.
        let start = checked.saturating_sub(ERROR_PROMPT.len() as u64);
        let prompted = read_from(log_path, start).contains(ERROR_PROMPT);
        checked = size;
        let resident = group_resident_bytes(child.id());
        let reason = if prompted {
            Some("stopped at an engine assertion".to_string())
        } else if size > MAX_LOG_BYTES {
            Some(format!("its log passed {} MB", MAX_LOG_BYTES >> 20))
        } else if resident > MAX_RESIDENT_BYTES {
            Some(format!(
                "it passed {} GB of memory",
                MAX_RESIDENT_BYTES >> 30
            ))
        } else {
            None
        };
        if let Some(reason) = reason {
            kill_group(child.id());
            let _ = child.wait();
            return Err(format!(
                "the archive builder {reason} and was killed:\n{}",
                log_tail(log_path)
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// The resident memory of every process in the group `group`, from
/// /proc; processes that cannot be read count as 0.
pub fn group_resident_bytes(group: u32) -> u64 {
    let Ok(entries) = fs::read_dir("/proc") else {
        return 0;
    };
    let page = 4096;
    entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path().join("stat")).ok())
        .filter_map(|stat| {
            // The command name may hold spaces; the fields after it do not.
            let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
            // State, ppid, pgrp, ... rss is the 22nd after the name.
            let in_group = fields.get(2)?.parse::<u32>().ok()? == group;
            let pages: u64 = fields.get(21)?.parse().ok()?;
            in_group.then_some(pages * page)
        })
        .sum()
}

/// SIGKILL to the whole process group.
pub fn kill_group(group: u32) {
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg("--")
        .arg(format!("-{group}"))
        .stderr(Stdio::null())
        .status();
}

pub fn read_from(path: &Path, start: u64) -> String {
    let mut text = Vec::new();
    if let Ok(mut file) = File::open(path)
        && file.seek(SeekFrom::Start(start)).is_ok()
    {
        let _ = file.take(MAX_LOG_BYTES).read_to_end(&mut text);
    }
    String::from_utf8_lossy(&text).into_owned()
}

pub fn log_tail(path: &Path) -> String {
    let size = fs::metadata(path).map_or(0, |metadata| metadata.len());
    read_from(path, size.saturating_sub(LOG_TAIL_BYTES))
}

fn copy_directory(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|error| format!("cannot create {}: {error}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            copy_directory(&path, &to.join(entry.file_name()))?;
        } else {
            fs::copy(&path, to.join(entry.file_name()))
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(())
}
