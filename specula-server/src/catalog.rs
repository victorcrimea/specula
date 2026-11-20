//! The game setup's catalog: the maps, the settings values, the biomes and
//! the civs, read from the engine's game data at start, as vanilla's
//! `gui/common/settings.js`, `gui/maps/MapCache.js` and
//! `globalscripts/Templates.js` read them. Texts are the engine's English
//! source strings, which the pages translate.

use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

/// As `scripts/play.sh` finds the engine, relative to the repository root
/// the server runs from.
const DEFAULT_PYROGENESIS: &str = "../0ad-specula/binaries/system/pyrogenesis";
/// `MapFilters.prototype.HiddenFilesPrefix`: vanilla never lists these.
const HIDDEN_PREFIX: &str = "_";
const BIOMES_DIR: &str = "maps/random/rmbiome";
/// `loadBiomes` skips it: it is the fallback every biome builds on, not a
/// choice.
const DEFAULT_BIOME: &str = "defaultbiome";
const SETTINGS_DIR: &str = "simulation/data/settings";
const CIVS_DIR: &str = "simulation/data/civs";
const PREVIEWS_DIR: &str = "art/textures/ui/session/icons/mappreview";
const AIS_DIR: &str = "simulation/ai";

/// `loadAIDifficulties`; the AI reads the index, not the name.
pub const AI_DIFFICULTIES: [&str; 6] = [
    "sandbox",
    "very easy",
    "easy",
    "medium",
    "hard",
    "very hard",
];
/// `gui.gamesetup.aidifficulty` in default.cfg.
pub const DEFAULT_AI_DIFFICULTY: u8 = 3;
/// `loadAIBehaviors`.
pub const AI_BEHAVIORS: [&str; 4] = ["random", "balanced", "defensive", "aggressive"];
/// `gui.gamesetup.aibehavior` in default.cfg.
pub const DEFAULT_AI_BEHAVIOR: &str = "random";

// M15 reads the catalog; until then only its counts are.
#[allow(dead_code)]
pub struct Catalog {
    pub maps: Vec<Map>,
    pub biomes: Vec<Biome>,
    pub civs: Vec<Civ>,
    pub game_speeds: Vec<GameSpeed>,
    pub map_sizes: Vec<MapSize>,
    /// Index 0 is gaia, as `g_Settings.PlayerDefaults[playerIndex + 1]`
    /// expects.
    pub player_defaults: Vec<PlayerDefault>,
    pub player_placements: Vec<PlayerPlacement>,
    pub population_capacities: Vec<PopulationCapacity>,
    pub starting_resources: Vec<StartingResources>,
    pub trigger_difficulties: Vec<TriggerDifficulty>,
    /// In `loadVictoryConditions`' order: `GUIOrder`, then title.
    pub victory_conditions: Vec<VictoryCondition>,
    /// File names, with suffix, in the map preview directory. Stands in for
    /// `MapCache.previewExists` (`gui/maps/MapCache.js:72`), i.e.
    /// `Engine.TextureExists`, which a loose mod's file existing satisfies.
    pub previews: Vec<String>,
    /// In `loadAIDescriptions`' order, by English name.
    pub ais: Vec<AiBot>,
}

#[allow(dead_code)]
pub struct AiBot {
    /// The directory under `simulation/ai/`, which `PlayerData[i].AI` names.
    pub id: String,
    pub name: String,
    pub description: String,
    /// `PlayerAssignment` offers only AIs without it; the AI config page
    /// offers every AI.
    pub hidden: bool,
}

/// `loadMapTypes`, in its order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MapType {
    Skirmish,
    Random,
    Scenario,
}

impl MapType {
    pub const ALL: [MapType; 3] = [MapType::Skirmish, MapType::Random, MapType::Scenario];

    /// `initAttributes.mapType`.
    pub fn name(self) -> &'static str {
        match self {
            MapType::Skirmish => "skirmish",
            MapType::Random => "random",
            MapType::Scenario => "scenario",
        }
    }

    fn dir(self) -> &'static str {
        match self {
            MapType::Skirmish => "maps/skirmishes",
            MapType::Random => "maps/random",
            MapType::Scenario => "maps/scenarios",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            MapType::Random => ".json",
            MapType::Skirmish | MapType::Scenario => ".xml",
        }
    }
}

#[allow(dead_code)]
pub struct Map {
    pub kind: MapType,
    /// `initAttributes.map`: the path without its suffix, such as
    /// `maps/random/mainland`.
    pub path: String,
    pub name: String,
    pub description: String,
    /// Under `art/textures/ui/session/icons/mappreview/`.
    pub preview: Option<String>,
    pub keywords: Vec<String>,
    /// The map's whole `settings` object, which the setup rules read field by
    /// field, with gaia's empty `PlayerData` entry removed as
    /// `MapCache.getMapData` removes it, so entry 0 is player 1.
    pub settings: Value,
}

#[allow(dead_code)]
pub struct Biome {
    /// Relative to `rmbiome/` without `.json`, such as `generic/aegean`, which
    /// a map's `SupportedBiomes` prefixes or lists.
    pub id: String,
    pub title: String,
    pub description: String,
    pub preview: Option<String>,
}

/// Names are not here: they are the player templates', which `civs.txt`
/// already holds.
#[allow(dead_code)]
pub struct Civ {
    pub code: String,
    /// A list, as `germ.json` writes it, though most civs write one string.
    pub cultures: Vec<String>,
    /// What `PlayerName` names an AI of this civ.
    pub ai_names: Vec<String>,
    /// `PlayerCiv` picks a random civ among these only.
    pub selectable: bool,
}

#[allow(dead_code)]
pub struct GameSpeed {
    pub speed: f64,
    pub title: String,
    pub default: bool,
    /// Offered only while no human holds a player slot (`GameSpeed.js`).
    pub fast_forward: bool,
}

#[allow(dead_code)]
pub struct MapSize {
    pub name: String,
    pub tooltip: String,
    pub tiles: u32,
    pub default: bool,
}

#[allow(dead_code)]
pub struct PlayerDefault {
    pub name: String,
    pub civ: String,
    pub color: [u8; 3],
    pub ai: String,
    pub ai_difficulty: u8,
    pub ai_behavior: String,
}

#[allow(dead_code)]
pub struct PlayerPlacement {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[allow(dead_code)]
pub struct PopulationCapacity {
    pub name: String,
    pub title: String,
    pub cap_title: String,
    pub tooltip: String,
    pub cap_tooltip: String,
    pub appendage: String,
    pub default: bool,
    pub options: Vec<u32>,
    pub option_default: u32,
}

#[allow(dead_code)]
pub struct StartingResources {
    pub resources: u32,
    pub title: String,
    pub default: bool,
}

#[allow(dead_code)]
pub struct TriggerDifficulty {
    pub difficulty: u8,
    pub name: String,
    pub title: String,
    pub tooltip: String,
    pub default: bool,
}

#[allow(dead_code)]
pub struct VictoryCondition {
    /// The file name without `.json`, as `loadVictoryConditions` names it.
    pub name: String,
    pub title: String,
    pub description: String,
    pub scripts: Vec<String>,
    pub gui_order: i64,
    pub default: bool,
    /// Conditions that cannot be chosen while this one is.
    pub disabled_when_checked: Vec<String>,
}

/// The public mod of the engine the server runs: `PYROGENESIS` or
/// `scripts/play.sh`'s default. The engine runs with the default mod list,
/// and of it only `public` holds setup data.
pub fn data_dir() -> Result<PathBuf, String> {
    let engine = std::env::var_os("PYROGENESIS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PYROGENESIS));
    // A symlinked binary would put the data next to the link otherwise.
    let engine = std::fs::canonicalize(&engine)
        .map_err(|error| format!("engine {}: {error}", engine.display()))?;
    let binaries = engine
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("engine {} is not under binaries/system", engine.display()))?;
    Ok(binaries.join("data").join("mods").join("public"))
}

impl Catalog {
    /// A map that cannot be read is left out with a warning, as vanilla's
    /// map list leaves it out; anything else missing is an error, since the
    /// setup cannot work without it.
    pub fn load(root: &Path) -> Result<Catalog, String> {
        let mut maps = Vec::new();
        for kind in MapType::ALL {
            read_maps(root, kind, &mut maps)?;
        }
        let settings = root.join(SETTINGS_DIR);
        let data = |name: &str| read_data(&settings.join(name));
        Ok(Catalog {
            maps,
            biomes: read_biomes(root)?,
            civs: read_civs(root)?,
            game_speeds: items(&data("game_speeds.json")?, "game_speeds.json", game_speed)?,
            map_sizes: items(&data("map_sizes.json")?, "map_sizes.json", map_size)?,
            player_defaults: player_defaults(&settings.join("player_defaults.json"))?,
            player_placements: items(
                &data("player_placements.json")?,
                "player_placements.json",
                player_placement,
            )?,
            population_capacities: items(
                &data("population_capacities.json")?,
                "population_capacities.json",
                population_capacity,
            )?,
            starting_resources: items(
                &data("starting_resources.json")?,
                "starting_resources.json",
                starting_resources,
            )?,
            trigger_difficulties: items(
                &data("trigger_difficulties.json")?,
                "trigger_difficulties.json",
                trigger_difficulty,
            )?,
            victory_conditions: read_victory_conditions(&settings.join("victory_conditions"))?,
            previews: read_previews(root)?,
            ais: read_ais(root)?,
        })
    }

    /// One line for the startup log.
    pub fn summary(&self, root: &Path) -> String {
        let count = |kind| self.maps.iter().filter(|map| map.kind == kind).count();
        format!(
            "setup catalog from {}: {} random, {} skirmish and {} scenario maps, {} biomes, {} civs ({} selectable), {} victory conditions, {} game speeds, {} map sizes, {} placements, {} population modes, {} starting resources, {} trigger difficulties, {} player defaults, {} AIs, {} map previews",
            root.display(),
            count(MapType::Random),
            count(MapType::Skirmish),
            count(MapType::Scenario),
            self.biomes.len(),
            self.civs.len(),
            self.civs.iter().filter(|civ| civ.selectable).count(),
            self.victory_conditions.len(),
            self.game_speeds.len(),
            self.map_sizes.len(),
            self.player_placements.len(),
            self.population_capacities.len(),
            self.starting_resources.len(),
            self.trigger_difficulties.len(),
            self.player_defaults.len(),
            self.ais.len(),
            self.previews.len(),
        )
    }
}

/// The file stems with `suffix` in `dir`, sorted so the catalog does not
/// follow the file system's order.
fn list(dir: &Path, suffix: &str) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut stems = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        let name = entry.file_name();
        if let (true, Some(name)) = (is_file, name.to_str())
            && let Some(stem) = name.strip_suffix(suffix)
        {
            stems.push(stem.to_string());
        }
    }
    stems.sort();
    Ok(stems)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// A settings values file's `Data`, as `loadSettingValuesFile` returns it.
fn read_data(path: &Path) -> Result<Value, String> {
    let mut json = read_json(path)?;
    match json.get_mut("Data") {
        Some(data) => Ok(data.take()),
        None => Err(format!("{}: no Data", path.display())),
    }
}

fn items<T>(data: &Value, file: &str, item: fn(&Value) -> Option<T>) -> Result<Vec<T>, String> {
    let list = data
        .as_array()
        .ok_or_else(|| format!("{file}: Data is not a list"))?;
    list.iter()
        .enumerate()
        .map(|(index, value)| {
            item(value).ok_or_else(|| format!("{file}: entry {index} is malformed"))
        })
        .collect()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

/// For the optional texts vanilla shows as empty when absent.
fn text_or_empty(value: &Value, key: &str) -> String {
    text(value, key).unwrap_or_default()
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn number<T: TryFrom<u64>>(value: &Value, key: &str) -> Option<T> {
    T::try_from(value.get(key)?.as_u64()?).ok()
}

fn texts(value: &Value, key: &str) -> Option<Vec<String>> {
    value
        .get(key)?
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_string))
        .collect()
}

fn read_maps(root: &Path, kind: MapType, maps: &mut Vec<Map>) -> Result<(), String> {
    let dir = root.join(kind.dir());
    for stem in list(&dir, kind.suffix())? {
        if stem.starts_with(HIDDEN_PREFIX) {
            continue;
        }
        let file = dir.join(format!("{stem}{}", kind.suffix()));
        let settings = match kind {
            MapType::Random => {
                read_json(&file).map(|mut json| json.get_mut("settings").map(Value::take))
            }
            MapType::Skirmish | MapType::Scenario => xml_settings(&file),
        };
        // "Map files may come with custom json files" (`MapFilters`): a
        // random map's script data shares the suffix, with no settings.
        let mut settings = match settings {
            Ok(Some(settings)) if settings.is_object() => settings,
            Ok(_) => continue,
            Err(error) => {
                eprintln!("setup catalog: skipping map {error}");
                continue;
            }
        };
        if let Some(players) = settings.get_mut("PlayerData").and_then(Value::as_array_mut)
            && players.first().is_some_and(falsy)
        {
            players.remove(0);
        }
        maps.push(Map {
            kind,
            path: format!("{}/{stem}", kind.dir()),
            name: text_or_empty(&settings, "Name"),
            description: text_or_empty(&settings, "Description"),
            preview: text(&settings, "Preview"),
            keywords: texts(&settings, "Keywords").unwrap_or_default(),
            settings,
        });
    }
    Ok(())
}

/// JavaScript's `!value`, which `MapCache.getMapData` tests gaia's entry
/// with: skirmishes write it as `""`, others as `null`.
fn falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(value) => !value,
        Value::Number(number) => number.as_f64().is_none_or(|number| number == 0.0),
        Value::String(text) => text.is_empty(),
        Value::Array(_) | Value::Object(_) => false,
    }
}

/// `CMapSummaryReader`: the text of the root's `ScriptSettings`, parsed as
/// JSON. Only that element is wanted, so the rest of a file of thousands of
/// entities is not parsed.
fn xml_settings(path: &Path) -> Result<Option<Value>, String> {
    let xml =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let Some(start) = xml.find("<ScriptSettings>") else {
        return Ok(None);
    };
    let body = &xml[start + "<ScriptSettings>".len()..];
    let end = body
        .find("</ScriptSettings>")
        .ok_or_else(|| format!("{}: ScriptSettings not closed", path.display()))?;
    let body = body[..end].trim();
    let json = match body
        .strip_prefix("<![CDATA[")
        .and_then(|inner| inner.strip_suffix("]]>"))
    {
        Some(inner) => inner.to_string(),
        None => unescape(body),
    };
    if json.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&json)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// The predefined XML entities, for settings written without CDATA.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn read_biomes(root: &Path) -> Result<Vec<Biome>, String> {
    let dir = root.join(BIOMES_DIR);
    let mut ids = Vec::new();
    collect_biomes(&dir, "", &mut ids)?;
    ids.retain(|id| id != DEFAULT_BIOME);
    ids.sort();
    ids.into_iter()
        .map(|id| {
            let path = dir.join(format!("{id}.json"));
            let json = read_json(&path)?;
            let description = json
                .get("Description")
                .ok_or_else(|| format!("{}: no Description", path.display()))?;
            Ok(Biome {
                title: text(description, "Title")
                    .ok_or_else(|| format!("{}: no Title", path.display()))?,
                description: text_or_empty(description, "Description"),
                preview: text(description, "Preview"),
                id,
            })
        })
        .collect()
}

/// `listFiles(g_BiomesDirectory, ".json", true)`: recursive, ids relative
/// to `rmbiome/`.
fn collect_biomes(dir: &Path, prefix: &str, ids: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_biomes(&entry.path(), &format!("{prefix}{name}/"), ids)?;
        } else if let Some(stem) = name.strip_suffix(".json") {
            ids.push(format!("{prefix}{stem}"));
        }
    }
    Ok(())
}

fn read_civs(root: &Path) -> Result<Vec<Civ>, String> {
    let dir = root.join(CIVS_DIR);
    list(&dir, ".json")?
        .into_iter()
        .map(|stem| {
            let path = dir.join(format!("{stem}.json"));
            let json = read_json(&path)?;
            // `loadCivFiles` throws on a civ missing these, so a broken
            // civ fails the server's start as it fails vanilla's setup.
            let missing = |key| format!("{}: no {key}", path.display());
            Ok(Civ {
                code: text(&json, "Code").ok_or_else(|| missing("Code"))?,
                cultures: text(&json, "Culture")
                    .map(|culture| vec![culture])
                    .or_else(|| texts(&json, "Culture"))
                    .ok_or_else(|| missing("Culture"))?,
                ai_names: texts(&json, "AINames").ok_or_else(|| missing("AINames"))?,
                selectable: json
                    .get("SelectableInGameSetup")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| missing("SelectableInGameSetup"))?,
            })
        })
        .collect()
}

fn game_speed(value: &Value) -> Option<GameSpeed> {
    Some(GameSpeed {
        speed: value.get("Speed")?.as_f64()?,
        title: text(value, "Title")?,
        default: flag(value, "Default"),
        fast_forward: flag(value, "FastForward"),
    })
}

fn map_size(value: &Value) -> Option<MapSize> {
    Some(MapSize {
        name: text(value, "Name")?,
        tooltip: text_or_empty(value, "Tooltip"),
        tiles: number(value, "Tiles")?,
        default: flag(value, "Default"),
    })
}

fn player_defaults(path: &Path) -> Result<Vec<PlayerDefault>, String> {
    let json = read_json(path)?;
    let file = path.display().to_string();
    let data = json
        .get("PlayerData")
        .ok_or_else(|| format!("{file}: no PlayerData"))?;
    items(data, &file, player_default)
}

fn player_default(value: &Value) -> Option<PlayerDefault> {
    let color = value.get("Color")?;
    Some(PlayerDefault {
        name: text(value, "Name")?,
        civ: text(value, "Civ")?,
        color: [
            number(color, "r")?,
            number(color, "g")?,
            number(color, "b")?,
        ],
        ai: text_or_empty(value, "AI"),
        ai_difficulty: number(value, "AIDiff")?,
        ai_behavior: text(value, "AIBehavior")?,
    })
}

fn player_placement(value: &Value) -> Option<PlayerPlacement> {
    Some(PlayerPlacement {
        id: text(value, "Id")?,
        name: text(value, "Name")?,
        description: text_or_empty(value, "Description"),
    })
}

fn population_capacity(value: &Value) -> Option<PopulationCapacity> {
    let options = value.get("Options")?;
    Some(PopulationCapacity {
        name: text(value, "Name")?,
        title: text(value, "Title")?,
        cap_title: text(value, "CapTitle")?,
        tooltip: text_or_empty(value, "Tooltip"),
        cap_tooltip: text_or_empty(value, "CapTooltip"),
        appendage: text_or_empty(value, "Appendage"),
        default: flag(value, "Default"),
        options: options
            .get("List")?
            .as_array()?
            .iter()
            .map(|option| {
                option
                    .as_u64()
                    .and_then(|option| u32::try_from(option).ok())
            })
            .collect::<Option<Vec<u32>>>()?,
        option_default: number(options, "Default")?,
    })
}

fn starting_resources(value: &Value) -> Option<StartingResources> {
    Some(StartingResources {
        resources: number(value, "Resources")?,
        title: text(value, "Title")?,
        default: flag(value, "Default"),
    })
}

fn trigger_difficulty(value: &Value) -> Option<TriggerDifficulty> {
    Some(TriggerDifficulty {
        difficulty: number(value, "Difficulty")?,
        name: text(value, "Name")?,
        title: text(value, "Title")?,
        tooltip: text_or_empty(value, "Tooltip"),
        default: flag(value, "Default"),
    })
}

fn read_victory_conditions(dir: &Path) -> Result<Vec<VictoryCondition>, String> {
    let mut conditions = list(dir, ".json")?
        .into_iter()
        .map(|name| {
            let path = dir.join(format!("{name}.json"));
            let data = read_data(&path)?;
            let malformed = || format!("{}: malformed", path.display());
            Ok(VictoryCondition {
                title: text(&data, "Title").ok_or_else(malformed)?,
                description: text_or_empty(&data, "Description"),
                scripts: texts(&data, "Scripts").ok_or_else(malformed)?,
                gui_order: data
                    .get("GUIOrder")
                    .and_then(Value::as_i64)
                    .ok_or_else(malformed)?,
                default: flag(&data, "Default"),
                disabled_when_checked: texts(&data, "DisabledWhenChecked").unwrap_or_default(),
                name,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    conditions.sort_by(|a, b| {
        a.gui_order
            .cmp(&b.gui_order)
            .then_with(|| a.title.cmp(&b.title))
    });
    Ok(conditions)
}

fn read_previews(root: &Path) -> Result<Vec<String>, String> {
    let dir = root.join(PREVIEWS_DIR);
    let entries = std::fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
        if entry.file_type().is_ok_and(|kind| kind.is_file())
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// `ICmpAIManager::GetAIs`: every `*.json` below `simulation/ai/`.
fn read_ais(root: &Path) -> Result<Vec<AiBot>, String> {
    let dir = root.join(AIS_DIR);
    let mut ais = Vec::new();
    collect_ais(&dir, None, &mut ais)?;
    ais.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ais)
}

fn collect_ais(dir: &Path, id: Option<&str>, ais: &mut Vec<AiBot>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
        let kind = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", dir.display()))?;
        paths.push((entry.path(), entry.file_name(), kind));
    }
    // The walk order is the file system's otherwise, and ties in the name
    // sort would follow it.
    paths.sort_by(|a, b| a.1.cmp(&b.1));
    for (path, name, kind) in paths {
        let Some(name) = name.to_str() else {
            continue;
        };
        if kind.is_dir() {
            collect_ais(&path, Some(id.unwrap_or(name)), ais)?;
        } else if name.ends_with(".json") {
            // The engine names an AI by the path's third component, which
            // for a file directly in `simulation/ai/` is the file itself.
            let json = read_json(&path)?;
            ais.push(AiBot {
                id: id.unwrap_or(name).to_string(),
                name: text(&json, "name").ok_or_else(|| format!("{}: no name", path.display()))?,
                description: text_or_empty(&json, "description"),
                hidden: flag(&json, "hidden"),
            });
        }
    }
    Ok(())
}
