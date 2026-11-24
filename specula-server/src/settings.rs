//! The game setup's settings, one field group per class of the engine's
//! `gamesettings/attributes/`, holding the state that class holds, and their
//! serializer to `initAttributes` (`GameSettings.toInitAttributes`, then
//! `launchGame`'s `onFinalizeAttributes`). The classes' dependency rules
//! (their `onMapChange` and `maybeUpdate` watchers) are in `rules`,
//! `fromInitAttributes` in `from_init`, and vanilla's `gui/gamesetup` layer
//! in `gamesetup`. The random choices (`pickRandomItems`) are resolved in
//! `random`.
//!
//! Groups and the serializer follow the engine's class order, the VFS's
//! byte order of the file names, since `GameSettings.init` runs each
//! `init()` in that order and the watchers an earlier class registered fire
//! during a later one's.

use rand::thread_rng;
use serde_json::Map as Object;
use serde_json::Value;
use serde_json::json;

use crate::catalog::Catalog;
use crate::catalog::MapType;
use crate::catalog::VictoryCondition;

mod compare;
mod from_init;
pub mod gamesetup;
mod random;
mod rules;

pub use compare::differences;

/// `Seeds` and `PlayerCiv`'s civs, the map, the biome, the daytime and the
/// placement may be left to chance until launch.
#[derive(Clone, PartialEq)]
pub enum Choice<T> {
    Random,
    Fixed(T),
}

impl<T: Clone + Into<Value>> Choice<T> {
    fn to_value(&self) -> Value {
        match self {
            Choice::Random => Value::from(RANDOM),
            Choice::Fixed(value) => value.clone().into(),
        }
    }
}

impl Choice<String> {
    /// `"random"` is the choice left to chance, anything else is itself.
    pub(crate) fn from_name(name: &str) -> Choice<String> {
        if name == RANDOM {
            Choice::Random
        } else {
            Choice::Fixed(name.to_string())
        }
    }
}

/// The value `"random"` stands for in every class.
const RANDOM: &str = "random";

/// A landscape may also be left to chance within one of the map's groups,
/// whose id is the value itself (`Landscape.pickRandomItems`).
#[derive(Clone, PartialEq)]
pub enum LandscapeChoice {
    Random,
    RandomGroup(String),
    Fixed(String),
}

impl LandscapeChoice {
    fn to_value(&self) -> Value {
        match self {
            LandscapeChoice::Random => Value::from(RANDOM),
            LandscapeChoice::RandomGroup(id) | LandscapeChoice::Fixed(id) => {
                Value::from(id.clone())
            }
        }
    }
}

/// `MapSize.init`'s fallback when no size is the default.
const DEFAULT_MAP_SIZE: u32 = 256;
/// `StartingResources.init`'s fallback when no amount is the default.
const DEFAULT_STARTING_RESOURCES: u32 = 300;
/// `GameSpeed.init` does not read `game_speeds.json`'s default.
const DEFAULT_GAME_SPEED: f64 = 1.0;
/// `g_MaxPlayers` in `gui/common/settings.js`.
pub const MAX_PLAYERS: usize = 8;
/// `PlayerColor.prototype.DefaultColors`, which the class offers instead of
/// `player_defaults.json`'s colors.
const DEFAULT_COLORS: [[u8; 3]; 16] = [
    [10, 10, 190],
    [230, 10, 10],
    [125, 235, 15],
    [255, 255, 55],
    [130, 0, 230],
    [255, 130, 0],
    [10, 230, 230],
    [20, 80, 60],
    [220, 160, 220],
    [80, 255, 190],
    [50, 150, 255],
    [100, 150, 30],
    [100, 60, 30],
    [128, 0, 64],
    [255, 200, 140],
    [80, 80, 80],
];

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub biome: Biome,
    pub campaign_data: Option<Value>,
    /// Minutes.
    pub ceasefire: u32,
    pub cheats: bool,
    pub circular_map: bool,
    pub daytime: Daytime,
    pub disable_spies: bool,
    pub disable_treasures: bool,
    pub game_speed: f64,
    pub landscape: Landscape,
    pub last_man_standing: Toggle,
    pub locked_teams: Toggle,
    pub map: MapChoice,
    pub map_exploration: MapExploration,
    pub map_name: Option<String>,
    pub map_preview: Option<String>,
    pub map_size: MapSize,
    pub nomad: bool,
    /// Index 0 is player 1, in every per-player list.
    pub player_ai: Vec<Option<Ai>>,
    pub player_civ: Locked<Choice<String>>,
    pub player_color: PlayerColor,
    pub player_count: PlayerCount,
    pub player_name: Vec<Option<String>>,
    pub player_placement: PlayerPlacement,
    /// Vanilla's gamesetup page adds it (`PlayerRemoved.js`), not
    /// `gamesettings/attributes`.
    pub player_removed: Vec<bool>,
    /// `-1` is no team.
    pub player_team: Locked<i32>,
    pub population: Population,
    pub rating: Toggle,
    pub regicide_garrison: Toggle,
    pub relic: Relic,
    pub sea_level_rise: SeaLevelRise,
    pub seeds: Seeds,
    /// A map's camera objects, passed through.
    pub starting_camera: Vec<Option<Value>>,
    pub starting_resources: StartingResources,
    pub trigger_difficulty: TriggerDifficulty,
    pub trigger_scripts: TriggerScripts,
    pub victory_conditions: VictoryConditions,
    pub water_level: WaterLevel,
    pub wonder: Wonder,
    pub gui: GuiData,
}

/// What vanilla's setup page holds outside `GameSettings`.
#[derive(Clone, PartialEq)]
pub struct GuiData {
    /// `GameSettingsGuiData.mapFilter.filter`.
    pub map_filter: String,
    /// `GameSpeed` control's `previousAllowFastForward`.
    pub allow_fast_forward: Option<bool>,
    /// Each `PlayerCiv` control's `wasLocked`, one per possible player.
    pub civ_was_locked: [bool; MAX_PLAYERS],
}

#[derive(Clone, PartialEq)]
pub struct Biome {
    pub biome: Option<Choice<String>>,
    /// Ids in the catalog's order; a map with none offers no biome.
    pub available: Vec<String>,
    /// `cachedMapData`: the map's `SupportedBiomes`, and the map it came from,
    /// since vanilla compares an array by identity and a string by value.
    pub cached: Option<(String, Value)>,
}

#[derive(Clone, PartialEq)]
pub struct Daytime {
    /// The map's `Daytime` options, passed through.
    pub data: Option<Value>,
    pub value: Option<Choice<String>>,
}

#[derive(Clone, PartialEq)]
pub struct Landscape {
    /// The map's `Landscapes` groups, passed through.
    pub data: Option<Value>,
    pub value: Option<LandscapeChoice>,
}

/// The classes that are on or off and may be unavailable.
#[derive(Clone, PartialEq)]
pub struct Toggle {
    pub available: bool,
    pub enabled: bool,
}

#[derive(Clone, PartialEq)]
pub struct MapChoice {
    pub kind: Option<MapType>,
    /// The map's path, such as `maps/random/mainland`.
    pub map: Option<Choice<String>>,
    pub script: Option<String>,
    /// What a random map is picked among.
    pub random_options: Vec<String>,
}

#[derive(Clone, PartialEq)]
pub struct MapExploration {
    pub explored: bool,
    pub revealed: bool,
    pub allied: bool,
}

#[derive(Clone, PartialEq)]
pub struct MapSize {
    pub default: u32,
    pub size: u32,
    pub available: bool,
}

#[derive(Clone, PartialEq)]
pub struct Ai {
    pub bot: String,
    pub difficulty: u8,
    pub behavior: String,
}

/// A per-player list whose entries a map can fix.
#[derive(Clone, PartialEq)]
pub struct Locked<T> {
    pub values: Vec<T>,
    pub locked: Vec<bool>,
}

#[derive(Clone, PartialEq)]
pub struct PlayerColor {
    pub values: Vec<Option<[u8; 3]>>,
    pub locked: Vec<bool>,
    pub available: Vec<[u8; 3]>,
}

#[derive(Clone, PartialEq)]
pub struct PlayerCount {
    pub players: usize,
    pub max: usize,
}

#[derive(Clone, PartialEq)]
pub struct PlayerPlacement {
    /// The map's `PlayerPlacements`.
    pub available: Option<Vec<String>>,
    pub value: Option<Choice<String>>,
}

#[derive(Clone, PartialEq)]
pub struct Population {
    /// `PopulationCapType`, a `population_capacities.json` name.
    pub cap_type: String,
    pub cap: u32,
    /// A scenario's per-player limits, which replace the cap.
    pub per_player: Option<Vec<Option<u32>>>,
}

#[derive(Clone, PartialEq)]
pub struct Relic {
    pub available: bool,
    pub count: u32,
    /// Minutes.
    pub duration: u32,
}

#[derive(Clone, PartialEq)]
pub struct SeaLevelRise {
    pub min: Option<i64>,
    pub max: Option<i64>,
    /// Minutes.
    pub value: Option<i64>,
}

#[derive(Clone, PartialEq)]
pub struct Seeds {
    pub seed: Choice<u32>,
    pub ai_seed: Choice<u32>,
}

#[derive(Clone, PartialEq)]
pub struct StartingResources {
    pub default: u32,
    pub resources: u32,
    /// A scenario's per-player amounts, written next to `resources`.
    pub per_player: Option<Vec<Option<Value>>>,
}

#[derive(Clone, PartialEq)]
pub struct TriggerDifficulty {
    /// The map's `SupportedTriggerDifficulties` names.
    pub available: Option<Vec<String>>,
    pub value: Option<u8>,
}

/// Each list is a set in insertion order, as the class's `Set`s.
#[derive(Clone, PartialEq)]
pub struct TriggerScripts {
    pub custom: Vec<String>,
    pub victory: Vec<String>,
    pub map: Vec<String>,
}

/// Each list is a set in insertion order, as the class's `Set`s.
#[derive(Clone, PartialEq)]
pub struct VictoryConditions {
    pub active: Vec<String>,
    pub disabled: Vec<String>,
}

#[derive(Clone, PartialEq)]
pub struct WaterLevel {
    /// The map's `WaterLevels`, passed through.
    pub options: Option<Value>,
    pub value: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct Wonder {
    pub available: bool,
    /// Minutes.
    pub duration: u32,
}

/// A seated human for `PlayerName.onFinalizeAttributes`.
pub struct Assignment<'a> {
    /// 1 is the first player, as `playerAssignments` numbers them.
    pub player: u8,
    pub name: &'a str,
}

/// What the lobby shows of a map (M40).
pub struct MapInfo {
    pub name: Option<String>,
    pub preview: Option<String>,
}

impl Settings {
    pub fn map_info(&self) -> MapInfo {
        MapInfo {
            name: self.map_name.clone(),
            preview: self.map_preview.clone(),
        }
    }
}

/// What a launch hands the engine and keeps for later.
pub struct Launch {
    /// Every random choice resolved. A rematch (D62) starts from these with
    /// only the AI seed random again, so it replays the same draw.
    pub settings: Settings,
    /// `finalizedAttributes`, what the engine starts with and M21 compares.
    pub attributes: Value,
}

impl Settings {
    /// The state `GameSettings.init` leaves before any map is chosen,
    /// including what the watchers registered by earlier classes do during
    /// later classes' `init()`. `Err` where vanilla's init would throw.
    pub fn new(catalog: &Catalog) -> Result<Settings, String> {
        let map_size = catalog
            .map_sizes
            .iter()
            .find(|size| size.default)
            .map_or(DEFAULT_MAP_SIZE, |size| size.tiles);
        // `Population.setPopCapType` reads the default type's `Options`,
        // which vanilla throws on without one.
        let population = catalog
            .population_capacities
            .iter()
            .find(|capacity| capacity.default)
            .ok_or("population_capacities.json has no default")?;
        let resources = catalog
            .starting_resources
            .iter()
            .find(|resources| resources.default)
            .map_or(DEFAULT_STARTING_RESOURCES, |resources| resources.resources);
        let mut victory_conditions = VictoryConditions {
            active: Vec::new(),
            disabled: Vec::new(),
        };
        let defaults: Vec<&str> = catalog
            .victory_conditions
            .iter()
            .filter(|condition| condition.default)
            .map(|condition| condition.name.as_str())
            .collect();
        victory_conditions.add(catalog, &defaults);
        let mut settings = Settings {
            biome: Biome {
                biome: None,
                available: Vec::new(),
                cached: None,
            },
            campaign_data: None,
            ceasefire: 0,
            cheats: false,
            circular_map: false,
            daytime: Daytime {
                data: None,
                value: None,
            },
            disable_spies: false,
            disable_treasures: false,
            game_speed: DEFAULT_GAME_SPEED,
            landscape: Landscape {
                data: None,
                value: None,
            },
            // Its init reads `lockedTeams.enabled` before LockedTeams' init
            // has set it.
            last_man_standing: Toggle {
                available: true,
                enabled: false,
            },
            // Rating's init (Rating sorts after LockedTeams) turns rating
            // off, whose watcher makes the teams unlocked but lockable.
            locked_teams: Toggle {
                available: true,
                enabled: false,
            },
            map: MapChoice {
                kind: None,
                map: None,
                script: None,
                random_options: Vec::new(),
            },
            map_exploration: MapExploration {
                explored: false,
                revealed: false,
                allied: false,
            },
            map_name: None,
            map_preview: None,
            map_size: MapSize {
                default: map_size,
                size: map_size,
                available: false,
            },
            nomad: false,
            player_ai: Vec::new(),
            player_civ: Locked {
                values: Vec::new(),
                locked: Vec::new(),
            },
            player_color: PlayerColor {
                values: Vec::new(),
                locked: Vec::new(),
                available: DEFAULT_COLORS.to_vec(),
            },
            player_count: PlayerCount {
                players: 1,
                max: MAX_PLAYERS,
            },
            player_name: Vec::new(),
            player_placement: PlayerPlacement {
                available: None,
                value: None,
            },
            player_removed: Vec::new(),
            player_team: Locked {
                values: Vec::new(),
                locked: Vec::new(),
            },
            population: Population {
                cap_type: population.name.clone(),
                cap: population.option_default,
                per_player: None,
            },
            // Without a lobby (`Engine.HasXmppClient`) no game is rated.
            rating: Toggle {
                available: false,
                enabled: false,
            },
            regicide_garrison: Toggle {
                available: false,
                enabled: false,
            },
            relic: Relic {
                available: false,
                count: 0,
                duration: 0,
            },
            sea_level_rise: SeaLevelRise {
                min: None,
                max: None,
                value: None,
            },
            seeds: Seeds {
                seed: Choice::Random,
                ai_seed: Choice::Random,
            },
            starting_camera: Vec::new(),
            starting_resources: StartingResources {
                default: resources,
                resources,
                per_player: None,
            },
            trigger_difficulty: TriggerDifficulty {
                available: None,
                value: None,
            },
            trigger_scripts: TriggerScripts {
                custom: Vec::new(),
                victory: Vec::new(),
                map: Vec::new(),
            },
            victory_conditions,
            water_level: WaterLevel {
                options: None,
                value: None,
            },
            // Wonder's init registers its watcher after VictoryConditions'
            // init has set the defaults, so it stays unavailable even with
            // the wonder victory a default.
            wonder: Wonder {
                available: false,
                duration: 0,
            },
            gui: GuiData {
                map_filter: "default".into(),
                allow_fast_forward: None,
                civ_was_locked: [false; MAX_PLAYERS],
            },
        };
        // The watchers on `victoryConditions.active` registered before
        // VictoryConditions' init: RegicideGarrison, Relic, TriggerScripts.
        let active = &settings.victory_conditions.active;
        let regicide = active.iter().any(|name| name == "regicide");
        let relic = active.iter().any(|name| name == "capture_the_relic");
        settings.regicide_garrison.available = regicide;
        settings.relic.available = relic;
        settings.relic.count = 1;
        settings.trigger_scripts.victory = settings.victory_conditions.scripts(catalog);
        // PlayerAI, PlayerCiv and PlayerColor sort before PlayerCount, so
        // they hear its init set one player; the later lists stay empty.
        settings.player_ai.push(None);
        settings.player_civ.values.push(Choice::Random);
        settings.player_civ.locked.push(false);
        settings.player_color.values.push(Some(DEFAULT_COLORS[0]));
        settings.player_color.locked.push(false);
        Ok(settings)
    }

    /// `GameSettings.toInitAttributes`.
    pub fn to_init_attributes(&self) -> Value {
        let mut top = Object::new();
        let mut settings = Object::new();
        let mut players: Vec<Object<String, Value>> = Vec::new();
        let random_map = self.map.kind == Some(MapType::Random);

        if let Some(biome) = &self.biome.biome {
            settings.insert("Biome".into(), biome.to_value());
        }
        if let Some(data) = &self.campaign_data
            && truthy(data)
        {
            top.insert("campaignData".into(), data.clone());
        }
        settings.insert("Ceasefire".into(), self.ceasefire.into());
        settings.insert("CheatsEnabled".into(), self.cheats.into());
        settings.insert("CircularMap".into(), self.circular_map.into());
        if let Some(daytime) = &self.daytime.value {
            settings.insert("Daytime".into(), daytime.to_value());
        }
        settings.insert("DisableSpies".into(), self.disable_spies.into());
        settings.insert("DisableTreasures".into(), self.disable_treasures.into());
        top.insert("gameSpeed".into(), self.game_speed.into());
        if let Some(landscape) = &self.landscape.value {
            settings.insert("Landscape".into(), landscape.to_value());
        }
        settings.insert(
            "LastManStanding".into(),
            self.last_man_standing.enabled.into(),
        );
        settings.insert("LockTeams".into(), self.locked_teams.enabled.into());
        if let Some(map) = &self.map.map {
            top.insert("map".into(), map.to_value());
        }
        if let Some(kind) = self.map.kind {
            top.insert("mapType".into(), kind.name().into());
        }
        if let Some(script) = self.map.script.as_ref().filter(|script| !script.is_empty()) {
            top.insert("script".into(), script.as_str().into());
        }
        settings.insert("RevealMap".into(), self.map_exploration.revealed.into());
        settings.insert("ExploreMap".into(), self.map_exploration.explored.into());
        settings.insert("AllyView".into(), self.map_exploration.allied.into());
        if let Some(name) = &self.map_name {
            settings.insert("mapName".into(), name.as_str().into());
        }
        if let Some(preview) = &self.map_preview {
            settings.insert("mapPreview".into(), preview.as_str().into());
        }
        if random_map {
            settings.insert("Size".into(), self.map_size.size.into());
            settings.insert("Nomad".into(), self.nomad.into());
        }
        for (index, ai) in self.player_ai.iter().enumerate() {
            let player = player_data(&mut players, index);
            match ai {
                Some(ai) => {
                    player.insert("AI".into(), ai.bot.as_str().into());
                    player.insert("AIDiff".into(), ai.difficulty.into());
                    player.insert("AIBehavior".into(), ai.behavior.as_str().into());
                }
                None => {
                    player.insert("AI".into(), false.into());
                }
            }
        }
        for (index, civ) in self.player_civ.values.iter().enumerate() {
            player_data(&mut players, index).insert("Civ".into(), civ.to_value());
        }
        for (index, color) in self.player_color.values.iter().enumerate() {
            let player = player_data(&mut players, index);
            if let Some([r, g, b]) = color {
                player.insert("Color".into(), json!({ "r": r, "g": g, "b": b }));
            }
        }
        if self.player_count.players > 0 {
            player_data(&mut players, self.player_count.players - 1);
        }
        for (index, name) in self.player_name.iter().enumerate() {
            let player = player_data(&mut players, index);
            if let Some(name) = name.as_ref().filter(|name| !name.is_empty()) {
                player.insert("Name".into(), name.as_str().into());
            }
        }
        if let Some(placement) = &self.player_placement.value {
            settings.insert("PlayerPlacement".into(), placement.to_value());
        }
        for (index, team) in self.player_team.values.iter().enumerate() {
            player_data(&mut players, index).insert("Team".into(), (*team).into());
        }
        settings.insert(
            "PopulationCapType".into(),
            self.population.cap_type.as_str().into(),
        );
        match &self.population.per_player {
            Some(limits) => per_player(&mut players, limits, "PopulationLimit"),
            None => {
                settings.insert("PopulationCap".into(), self.population.cap.into());
            }
        }
        if self.rating.available {
            settings.insert("RatingEnabled".into(), self.rating.enabled.into());
        }
        if self.regicide_garrison.available {
            settings.insert(
                "RegicideGarrison".into(),
                self.regicide_garrison.enabled.into(),
            );
        }
        if self.relic.available {
            settings.insert("RelicCount".into(), self.relic.count.into());
            settings.insert("RelicDuration".into(), self.relic.duration.into());
        }
        if let Some(time) = self.sea_level_rise.value {
            settings.insert("SeaLevelRiseTime".into(), time.into());
        }
        settings.insert("Seed".into(), self.seeds.seed.to_value());
        settings.insert("AISeed".into(), self.seeds.ai_seed.to_value());
        for (index, camera) in self.starting_camera.iter().enumerate() {
            let player = player_data(&mut players, index);
            if let Some(camera) = camera.as_ref().filter(|camera| truthy(camera)) {
                player.insert("StartingCamera".into(), camera.clone());
            }
        }
        if let Some(amounts) = &self.starting_resources.per_player {
            per_player_values(&mut players, amounts, "Resources");
        }
        settings.insert(
            "StartingResources".into(),
            self.starting_resources.resources.into(),
        );
        if let (Some(_), Some(difficulty)) = (
            &self.trigger_difficulty.available,
            self.trigger_difficulty.value,
        ) {
            settings.insert("TriggerDifficulty".into(), difficulty.into());
        }
        settings.insert(
            "TriggerScripts".into(),
            self.trigger_scripts.custom.clone().into(),
        );
        settings.insert(
            "VictoryConditions".into(),
            self.victory_conditions.active.clone().into(),
        );
        if let Some(level) = self
            .water_level
            .value
            .as_ref()
            .filter(|level| !level.is_empty())
        {
            settings.insert("waterLevel".into(), level.as_str().into());
        }
        if self.wonder.available {
            settings.insert("WonderDuration".into(), self.wonder.duration.into());
        }
        for (index, removed) in self.player_removed.iter().enumerate() {
            player_data(&mut players, index).insert("Removed".into(), (*removed).into());
        }

        settings.insert(
            "PlayerData".into(),
            Value::Array(players.into_iter().map(Value::Object).collect()),
        );
        top.insert("settings".into(), Value::Object(settings));
        Value::Object(top)
    }

    /// `launchGame`'s attributes after the classes' `onFinalizeAttributes`,
    /// so the server holds what the engine starts with and M21 can compare.
    /// `match_id` replaces `Engine.GetMatchID`'s fresh GUID, since the
    /// match's UUID is already its id everywhere else (D23). Rating's
    /// finalizer only tells the engine whether the game is ranked, which is
    /// no attribute.
    pub fn finalized(&self, assignments: &[Assignment], match_id: &str) -> Value {
        let mut attributes = self.to_init_attributes();
        let settings = &mut attributes["settings"];
        if let Some(players) = settings["PlayerData"].as_array_mut() {
            for assignment in assignments {
                if let Some(Value::Object(player)) = usize::from(assignment.player)
                    .checked_sub(1)
                    .and_then(|index| players.get_mut(index))
                {
                    player.insert("Name".into(), assignment.name.into());
                }
            }
        }
        let mut scripts = self.trigger_scripts.custom.clone();
        for script in self
            .trigger_scripts
            .victory
            .iter()
            .chain(&self.trigger_scripts.map)
        {
            insert_unique(&mut scripts, script);
        }
        settings["TriggerScripts"] = scripts.into();
        attributes["matchID"] = match_id.into();
        attributes
    }

    /// `GameSettings.launchGame` up to the engine call. It resolves a clone
    /// because the setup keeps its random choices for the next match, as
    /// vanilla saves its settings before resolving them.
    pub fn launch(
        &self,
        catalog: &Catalog,
        assignments: &[Assignment],
        match_id: &str,
    ) -> Result<Launch, String> {
        let mut settings = self.clone();
        settings.pick_random_items(catalog, &mut thread_rng())?;
        let attributes = settings.finalized(assignments, match_id);
        Ok(Launch {
            settings,
            attributes,
        })
    }
}

impl VictoryConditions {
    /// `VictoryConditions._add` onto the current sets.
    pub fn add(&mut self, catalog: &Catalog, names: &[&str]) {
        let mut active = self.active.clone();
        for name in names {
            if self.disabled.iter().any(|disabled| disabled == name) {
                continue;
            }
            insert_unique(&mut active, name);
            // "Assume we want to remove incompatible ones."
            if let Some(condition) = condition(catalog, name) {
                active.retain(|other| !condition.disabled_when_checked.contains(other));
            }
        }
        self.disabled = disabled(catalog, &active);
        self.active = active;
    }

    /// `VictoryConditions._delete`.
    pub fn delete(&mut self, catalog: &Catalog, names: &[&str]) {
        let mut active = self.active.clone();
        active.retain(|name| !names.contains(&name.as_str()));
        self.disabled = disabled(catalog, &active);
        self.active = active;
    }

    /// `VictoryConditions.fromList`. An empty list really empties `active`,
    /// as in vanilla, where `![]` is false.
    #[allow(clippy::wrong_self_convention)] // vanilla's name, and it mutates
    pub fn from_list(&mut self, catalog: &Catalog, names: &[&str]) {
        let kept: Vec<&str> = names
            .iter()
            .copied()
            .filter(|name| condition(catalog, name).is_some())
            .collect();
        self.active.clear();
        self.disabled.clear();
        self.add(catalog, &kept);
    }

    /// `TriggerScripts.updateVictoryScripts`: the active conditions' scripts.
    pub fn scripts(&self, catalog: &Catalog) -> Vec<String> {
        let mut scripts = Vec::new();
        for name in &self.active {
            if let Some(condition) = condition(catalog, name) {
                for script in &condition.scripts {
                    insert_unique(&mut scripts, script);
                }
            }
        }
        scripts
    }
}

fn condition<'a>(catalog: &'a Catalog, name: &str) -> Option<&'a VictoryCondition> {
    catalog
        .victory_conditions
        .iter()
        .find(|condition| condition.name == name)
}

/// `VictoryConditions._reconstructDisabled`.
fn disabled(catalog: &Catalog, active: &[String]) -> Vec<String> {
    let mut disabled = Vec::new();
    for name in active {
        if let Some(condition) = condition(catalog, name) {
            for other in &condition.disabled_when_checked {
                insert_unique(&mut disabled, other);
            }
        }
    }
    disabled
}

fn insert_unique(set: &mut Vec<String>, item: &str) {
    if !set.iter().any(|other| other == item) {
        set.push(item.to_string());
    }
}

/// Each class pads `PlayerData` with empty objects up to its own list's
/// length before writing into it.
fn player_data(
    players: &mut Vec<Object<String, Value>>,
    index: usize,
) -> &mut Object<String, Value> {
    while players.len() <= index {
        players.push(Object::new());
    }
    &mut players[index]
}

/// Population's and StartingResources' per-player lists skip the players
/// the map gives no value.
fn per_player(players: &mut Vec<Object<String, Value>>, values: &[Option<u32>], key: &str) {
    for (index, value) in values.iter().enumerate() {
        let player = player_data(players, index);
        if let Some(value) = value.filter(|value| *value != 0) {
            player.insert(key.into(), value.into());
        }
    }
}

fn per_player_values(
    players: &mut Vec<Object<String, Value>>,
    values: &[Option<Value>],
    key: &str,
) {
    for (index, value) in values.iter().enumerate() {
        let player = player_data(players, index);
        if let Some(value) = value.as_ref().filter(|value| truthy(value)) {
            player.insert(key.into(), value.clone());
        }
    }
}

/// JavaScript's unary `+`.
fn js_number(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                0.0
            } else {
                text.parse().unwrap_or(f64::NAN)
            }
        }
        Value::Bool(value) => f64::from(u8::from(*value)),
        Value::Null => 0.0,
        Value::Array(_) | Value::Object(_) => f64::NAN,
    }
}

/// `Math.round`, which differs from `f64::round` for negative halves.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// JavaScript's truthiness, which the classes test their passthrough values
/// with before writing them.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|number| number != 0.0 && !number.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
