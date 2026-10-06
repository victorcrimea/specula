//! Has the engine convert COLLADA to its own PMD/PSA, so the skeleton
//! mapping in art/skeletons and the converter's axis and bone handling stay
//! the engine's. `-archivebuild` converts only what is under the directory
//! it is given, so a staging copy of just the needed files plus
//! art/skeletons keeps it to a fraction of a second.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

/// Returns the directory the converted files were unpacked into; each
/// source `x.dae` is there as `x.dae.cached.pmd` or `x.dae.cached.psa`
/// (`CCacheLoader::ArchiveCachePath`).
pub fn run(
    mod_dir: &Path,
    pyrogenesis: &Path,
    work_dir: &Path,
    sources: &[String],
) -> Result<PathBuf, String> {
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

    copy_directory(&mod_dir.join("art/skeletons"), &stage.join("art/skeletons"))?;
    for source in sources {
        let target = stage.join(source);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        fs::copy(mod_dir.join(source), &target).map_err(|error| format!("{source}: {error}"))?;
    }

    // The engine resolves relative paths against its own directories.
    let absolute = |path: &Path| {
        std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let output = Command::new(pyrogenesis)
        .arg(format!("-archivebuild={}", absolute(&stage)?.display()))
        .arg(format!(
            "-archivebuild-output={}",
            absolute(&archive)?.display()
        ))
        .output()
        .map_err(|error| format!("cannot run {}: {error}", pyrogenesis.display()))?;
    if !output.status.success() || !archive.exists() {
        return Err(format!(
            "the archive builder failed ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
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
    let missing: Vec<&String> = sources
        .iter()
        .filter(|source| !cached_path(&converted, source).exists())
        .collect();
    if !missing.is_empty() {
        return Err(format!("the engine did not convert: {missing:?}"));
    }
    Ok(converted)
}

pub fn cached_path(converted: &Path, source: &str) -> PathBuf {
    let kind = if source.starts_with("art/animation/") {
        "psa"
    } else {
        "pmd"
    };
    converted.join(format!("{source}.cached.{kind}"))
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
