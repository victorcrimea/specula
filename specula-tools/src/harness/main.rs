//! Judges an AI against another: one test is two 1v1 matches of the same
//! setup with the start positions swapped (see TEST.md).
//!
//! usage:
//!   harness SETUP.json [options]      one test, JSON on stdout
//!   harness DIR [options]             every *.json in DIR, with --jobs
//!   harness pairs TEMPLATE.json DIR   a setup per civ pair and mirror
//!   harness maps TEMPLATE.json DIR    a mirror setup per map and map seed
//! options: --time-limit MIN (game time, default 180), --ai-seed N,
//!   --keep DIR (replays and logs), --jobs N (matches at once, default 8),
//!   --results FILE (JSON lines, default target/harness/<dir>.jsonl),
//!   --wall-limit MIN (per match, default 60)
//! pairs: --seeds N (map and AI seeds 1..=N, default 1, files <a>-<b>-<seed>
//!   when above 1), --no-mirrors
//! maps: --civ CODE (default athen), --seeds N (default 10), --keyword WORD
//!   (the random maps with it in their Keywords, default multiplayer)

mod engine;
mod report;
mod setup;

use engine::MatchResult;
use engine::Options;
use serde_json::Value;
use setup::Setup;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

const CIVS_DIR: &str = "../0ad-specula/binaries/data/mods/public/simulation/data/civs";
const SCRATCH: &str = "target/harness";

struct Config {
    time_limit_minutes: f64,
    wall_limit_minutes: u64,
    ai_seed: Option<u32>,
    keep: Option<PathBuf>,
    jobs: usize,
    results: Option<PathBuf>,
    civ: String,
    seeds: Option<u32>,
    no_mirrors: bool,
    keyword: String,
}

fn main() -> ExitCode {
    let mut positional = Vec::new();
    let mut config = Config {
        time_limit_minutes: 180.0,
        wall_limit_minutes: 60,
        ai_seed: None,
        keep: None,
        jobs: 8,
        results: None,
        civ: "athen".to_string(),
        seeds: None,
        no_mirrors: false,
        keyword: "multiplayer".to_string(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        let parsed = match arg.as_str() {
            "--time-limit" => value(&arg).and_then(|v| {
                v.parse()
                    .map(|v| config.time_limit_minutes = v)
                    .map_err(|e| format!("{arg}: {e}"))
            }),
            "--wall-limit" => value(&arg).and_then(|v| {
                v.parse()
                    .map(|v| config.wall_limit_minutes = v)
                    .map_err(|e| format!("{arg}: {e}"))
            }),
            "--ai-seed" => value(&arg).and_then(|v| {
                v.parse()
                    .map(|v| config.ai_seed = Some(v))
                    .map_err(|e| format!("{arg}: {e}"))
            }),
            "--jobs" => value(&arg).and_then(|v| {
                v.parse()
                    .map(|v: usize| config.jobs = v.max(1))
                    .map_err(|e| format!("{arg}: {e}"))
            }),
            "--seeds" => value(&arg).and_then(|v| {
                v.parse()
                    .map(|v| config.seeds = Some(v))
                    .map_err(|e| format!("{arg}: {e}"))
            }),
            "--civ" => value(&arg).map(|v| config.civ = v),
            "--keyword" => value(&arg).map(|v| config.keyword = v),
            "--no-mirrors" => {
                config.no_mirrors = true;
                Ok(())
            }
            "--keep" => value(&arg).map(|v| config.keep = Some(PathBuf::from(v))),
            "--results" => value(&arg).map(|v| config.results = Some(PathBuf::from(v))),
            _ if arg.starts_with("--") => Err(format!("unknown option {arg}")),
            _ => {
                positional.push(arg);
                Ok(())
            }
        };
        if let Err(error) = parsed {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    }
    let result = match positional.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["pairs", template, out] => pairs(Path::new(template), Path::new(out), &config),
        ["maps", template, out] => maps(Path::new(template), Path::new(out), &config),
        [path] if Path::new(path).is_dir() => run_directory(Path::new(path), &config),
        [path] => run_one(Path::new(path), &config),
        _ => Err("usage: harness SETUP.json | DIR | pairs TEMPLATE.json OUT_DIR".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_one(path: &Path, config: &Config) -> Result<(), String> {
    let setup = Setup::load(path)?;
    let result = run_test(setup, config, config.jobs >= 2)?;
    eprint!("{}", report::table(&result));
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// Runs the match as given and the swapped one, at once or in turn.
fn run_test(mut setup: Setup, config: &Config, parallel: bool) -> Result<Value, String> {
    setup.resolve_seeds(config.ai_seed);
    let first = Setup {
        name: setup.name.clone(),
        value: setup.value.clone(),
    }
    .with_harness_script();
    let second = setup.swapped().with_harness_script();
    let options = |index: usize| Options {
        time_limit: config.time_limit_minutes * 60.0,
        wall_limit: Duration::from_secs(config.wall_limit_minutes * 60),
        work_dir: match &config.keep {
            Some(keep) => keep.join(format!("{}-{}", setup.name, index + 1)),
            None => Path::new(SCRATCH).join("work").join(format!(
                "{}-{}-{}",
                setup.name,
                std::process::id(),
                index + 1
            )),
        },
        cache_dir: Path::new(SCRATCH).join("cache"),
        keep: config.keep.is_some(),
    };
    let (one, two) = (options(0), options(1));
    let mirror = setup.is_mirror();
    let outcomes: [Result<MatchResult, String>; 2] = if mirror {
        // The same game with the labels exchanged; the report marks it.
        let played = engine::run_match(&first, &one);
        [played.clone(), played]
    } else if parallel {
        std::thread::scope(|scope| {
            let handle = scope.spawn(|| engine::run_match(&second, &two));
            let first = engine::run_match(&first, &one);
            [
                first,
                handle
                    .join()
                    .unwrap_or_else(|_| Err("panicked".to_string())),
            ]
        })
    } else {
        [
            engine::run_match(&first, &one),
            engine::run_match(&second, &two),
        ]
    };
    let [one, two] = outcomes;
    let outcomes = [
        one.map_err(|e| format!("{}, match 1: {e}", setup.name))?,
        two.map_err(|e| format!("{}, match 2 (swapped): {e}", setup.name))?,
    ];
    Ok(report::build(&setup, &outcomes, mirror))
}

fn run_directory(dir: &Path, config: &Config) -> Result<(), String> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let name = dir.file_name().map_or_else(
        || "setups".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let results_path = config
        .results
        .clone()
        .unwrap_or_else(|| Path::new(SCRATCH).join(format!("{name}.jsonl")));
    if let Some(parent) = results_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    // Loaded up front so a bad setup fails before any match runs.
    let mut setups = Vec::new();
    for file in &files {
        setups.push(Setup::load(file)?);
    }

    let done = read_lines(&results_path);
    let key = |result: &Value| {
        (
            result["setup"].as_str().unwrap_or("").to_string(),
            result["seed"].as_u64().unwrap_or(0) as u32,
            result["ai_seed"].as_u64().unwrap_or(0) as u32,
        )
    };
    let done_keys: HashSet<_> = done.iter().map(key).collect();
    let mut run_keys: HashSet<(String, u32, u32)> = HashSet::new();
    let mut todo = Vec::new();
    for setup in setups {
        match setup.fixed_seeds(config.ai_seed) {
            Some((seed, ai_seed)) => {
                let k = (setup.name.clone(), seed, ai_seed);
                run_keys.insert(k.clone());
                if !done_keys.contains(&k) {
                    todo.push(setup);
                }
            }
            None => todo.push(setup),
        }
    }
    eprintln!(
        "{} setups, {} to run, results in {}",
        files.len(),
        todo.len(),
        results_path.display()
    );

    let output = Mutex::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&results_path)
            .map_err(|e| format!("{}: {e}", results_path.display()))?,
    );
    let ran = Mutex::new(Vec::<(String, u32, u32)>::new());
    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let total = todo.len();
    // A test is two matches; with one job they run in turn.
    let parallel = config.jobs >= 2;
    let workers = if parallel { config.jobs / 2 } else { 1 };
    let todo: Vec<Mutex<Option<Setup>>> = todo.into_iter().map(|s| Mutex::new(Some(s))).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(slot) = todo.get(index) else {
                        break;
                    };
                    let Some(setup) = slot.lock().unwrap().take() else {
                        break;
                    };
                    let name = setup.name.clone();
                    match run_test(setup, config, parallel) {
                        Ok(result) => {
                            let count = finished.fetch_add(1, Ordering::SeqCst) + 1;
                            eprint!("[{count}/{total}] {}", report::table(&result));
                            let mut file = output.lock().unwrap();
                            let _ = writeln!(file, "{result}");
                            let _ = file.flush();
                            ran.lock().unwrap().push(key(&result));
                        }
                        Err(error) => {
                            let count = finished.fetch_add(1, Ordering::SeqCst) + 1;
                            eprintln!("[{count}/{total}] {name}: failed: {error}");
                        }
                    }
                }
            });
        }
    });
    run_keys.extend(ran.into_inner().unwrap());

    let results: Vec<Value> = read_lines(&results_path)
        .into_iter()
        .filter(|result| run_keys.contains(&key(result)))
        .collect();
    eprintln!(
        "\n{} tests\n{}\n{}",
        results.len(),
        report::civ_table(&results),
        report::map_table(&results)
    );
    Ok(())
}

fn read_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// One setup per civ pair and mirror: `<a>-<b>.json` for every a <= b.
fn pairs(template: &Path, out: &Path, config: &Config) -> Result<(), String> {
    let text = fs::read_to_string(template).map_err(|e| format!("{}: {e}", template.display()))?;
    let base: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", template.display()))?;
    let mut civs = Vec::new();
    for entry in fs::read_dir(CIVS_DIR)
        .map_err(|e| format!("{CIVS_DIR}: {e}"))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let civ: Value = serde_json::from_str(
            &fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
        if civ["SelectableInGameSetup"].as_bool() != Some(false)
            && let Some(code) = path.file_stem()
        {
            civs.push(code.to_string_lossy().into_owned());
        }
    }
    civs.sort();
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for (index, a) in civs.iter().enumerate() {
        for b in &civs[index..] {
            if config.no_mirrors && a == b {
                continue;
            }
            let seeds = config.seeds.unwrap_or(1);
            for seed in 1..=seeds {
                let mut value = base.clone();
                for (slot, civ) in [a, b].into_iter().enumerate() {
                    let player = value["settings"]["PlayerData"]
                        .get_mut(slot)
                        .and_then(Value::as_object_mut)
                        .ok_or("the template needs two players")?;
                    player.insert("Civ".to_string(), Value::from(civ.as_str()));
                    player.insert("Name".to_string(), Value::from(format!("Petra {civ}")));
                }
                // One seed keeps the template's, as the first inputs did.
                if config.seeds.is_some() {
                    value["settings"]["Seed"] = Value::from(seed);
                    value["settings"]["AISeed"] = Value::from(seed);
                }
                let path = if seeds > 1 {
                    out.join(format!("{a}-{b}-{seed}.json"))
                } else {
                    out.join(format!("{a}-{b}.json"))
                };
                fs::write(&path, tabbed(&value)).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
    }
    eprintln!("{} civs, setups in {}", civs.len(), out.display());
    Ok(())
}

const MAPS_DIR: &str = "../0ad-specula/binaries/data/mods/public/maps/random";

/// A mirror (`--civ` against itself) on every random map with `--keyword`,
/// `--seeds` times with map seeds 1..=N, for finding the maps whose start
/// positions are fair. The map's script, its first player placement and a
/// concrete biome are written in, because the engine would otherwise pick
/// them with its unseeded RNG and the two matches would differ.
fn maps(template: &Path, out: &Path, config: &Config) -> Result<(), String> {
    let text = fs::read_to_string(template).map_err(|e| format!("{}: {e}", template.display()))?;
    let base: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", template.display()))?;
    let mut names = Vec::new();
    for entry in fs::read_dir(MAPS_DIR)
        .map_err(|e| format!("{MAPS_DIR}: {e}"))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(map) = serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap_or_default())
        else {
            continue;
        };
        let keywords = map["settings"]["Keywords"].as_array();
        if keywords.is_some_and(|k| k.iter().any(|k| k == config.keyword.as_str()))
            && let Some(name) = path.file_stem()
        {
            names.push((name.to_string_lossy().into_owned(), map));
        }
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for (name, map) in &names {
        let settings = &map["settings"];
        for seed in 1..=config.seeds.unwrap_or(10) {
            let mut value = base.clone();
            value["map"] = Value::from(format!("maps/random/{name}"));
            value["script"] = settings["Script"].clone();
            let attributes = value["settings"]
                .as_object_mut()
                .ok_or("the template has no settings")?;
            match settings["PlayerPlacements"].get(0) {
                Some(placement) => {
                    attributes.insert("PlayerPlacement".to_string(), placement.clone())
                }
                None => attributes.remove("PlayerPlacement"),
            };
            match biome(&settings["SupportedBiomes"])? {
                Some(biome) => attributes.insert("Biome".to_string(), Value::from(biome)),
                None => attributes.remove("Biome"),
            };
            attributes.insert("Seed".to_string(), Value::from(seed));
            attributes.insert("AISeed".to_string(), Value::from(seed));
            for slot in 0..2 {
                let player = value["settings"]["PlayerData"]
                    .get_mut(slot)
                    .and_then(Value::as_object_mut)
                    .ok_or("the template needs two players")?;
                player.insert("Civ".to_string(), Value::from(config.civ.as_str()));
                player.insert(
                    "Name".to_string(),
                    Value::from(format!("Petra {}", config.civ)),
                );
            }
            let path = out.join(format!("{name}-{seed}.json"));
            fs::write(&path, tabbed(&value)).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    eprintln!(
        "{} maps x {} seeds in {}: {}",
        names.len(),
        config.seeds.unwrap_or(10),
        out.display(),
        names
            .iter()
            .map(|n| n.0.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(())
}

/// The first biome the map supports (`SupportedBiomes` is a list of ids or
/// a prefix), preferring a plain one, as the setup page lists them.
fn biome(supported: &Value) -> Result<Option<String>, String> {
    let ids = match supported {
        Value::Null => return Ok(None),
        Value::Array(list) => list
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect(),
        Value::String(prefix) => {
            let root = Path::new(MAPS_DIR).join("rmbiome");
            let mut ids = Vec::new();
            for directory in fs::read_dir(&root)
                .map_err(|e| format!("{}: {e}", root.display()))?
                .flatten()
            {
                let Ok(files) = fs::read_dir(directory.path()) else {
                    continue;
                };
                for file in files.flatten() {
                    let id = format!(
                        "{}/{}",
                        directory.file_name().to_string_lossy(),
                        file.path()
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                    );
                    if id.starts_with(prefix.as_str()) {
                        ids.push(id);
                    }
                }
            }
            ids.sort();
            ids
        }
        _ => return Err("SupportedBiomes is neither a list nor a string".to_string()),
    };
    let plain = ["temperate", "spring", "late_spring"];
    Ok(plain
        .iter()
        .find_map(|p| ids.iter().find(|id| id.ends_with(&format!("/{p}"))))
        .or(ids.first())
        .cloned())
}

/// `{:#}` indents by two spaces; the inputs use tabs. A string never holds
/// a raw newline, so a line's leading spaces are all indentation.
fn tabbed(value: &Value) -> String {
    let mut text = String::new();
    for line in format!("{value:#}").lines() {
        let body = line.trim_start_matches(' ');
        text.push_str(&"\t".repeat((line.len() - body.len()) / 2));
        text.push_str(body);
        text.push('\n');
    }
    text
}
