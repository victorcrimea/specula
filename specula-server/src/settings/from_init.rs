//! `GameSettings.fromInitAttributes`: the host's stored settings (D34) loaded
//! back into the classes, which settle by running every class's step until
//! nothing changes, as vanilla does.

use serde_json::Value;

use super::Ai;
use super::Choice;
use super::LandscapeChoice;
use super::MAX_PLAYERS;
use super::RANDOM;
use super::Settings;
use super::gamesetup;
use super::js_number;
use super::rules::ai_by_id;
use super::truthy;
use crate::catalog::AI_BEHAVIORS;
use crate::catalog::Catalog;
use crate::catalog::DEFAULT_AI_BEHAVIOR;
use crate::catalog::DEFAULT_AI_DIFFICULTY;
use crate::catalog::MapType;

/// `GameSettings.prototype.settings` count: vanilla loops this many times
/// at most.
const COMPONENTS: usize = 40;

fn as_i32(value: &Value) -> Option<i32> {
    value.as_i64().and_then(|value| i32::try_from(value).ok())
}

/// `Biome` and `Daytime` share their parsing: `"random"`, a known id, else
/// random.
fn choice_among(value: &Value, known: impl Fn(&str) -> bool) -> Choice<String> {
    match value.as_str() {
        Some(id) if id != RANDOM && known(id) => Choice::Fixed(id.to_string()),
        _ => Choice::Random,
    }
}

impl Settings {
    /// Values outside what a setup control offers are ignored, keeping the
    /// current one (D34). Not read at all, since no setup control sets them
    /// and a browser must not inject them: `campaignData`, `CircularMap`,
    /// `mapName`, `mapPreview`, `Seed`, `AISeed`, `PlayerData[].StartingCamera`,
    /// `TriggerScripts`, `PlayerData[].Name` (vanilla skips names for
    /// persistent settings too), `RatingEnabled` (inert) and `matchID`.
    #[allow(clippy::wrong_self_convention)] // vanilla's name, and it mutates
    pub(super) fn from_init_attributes(
        &mut self,
        catalog: &Catalog,
        attribs: &Value,
    ) -> Result<(), String> {
        for _ in 0..=COMPONENTS {
            let before = self.clone();
            self.init_step(catalog, attribs);
            if *self == before {
                return Ok(());
            }
        }
        Err("settings did not settle".to_string())
    }

    /// Every class's step, in class order.
    fn init_step(&mut self, catalog: &Catalog, attribs: &Value) {
        let legacy = |key: &str| {
            attribs
                .get("settings")
                .and_then(|settings| settings.get(key))
        };
        let flag = |key: &str| legacy(key).is_some_and(truthy);
        let players = legacy("PlayerData").and_then(Value::as_array);
        let player_field = |index: usize, key: &str| {
            players
                .and_then(|list| list.get(index))
                .and_then(|entry| entry.get(key))
        };

        // Biome
        match legacy("Biome").filter(|value| truthy(value)) {
            None => self.set_biome(catalog, None),
            Some(value) => {
                let biome = choice_among(value, |id| self.biome.available.iter().any(|a| a == id));
                self.set_biome(catalog, Some(biome));
            }
        }

        // Ceasefire
        match legacy("Ceasefire").filter(|value| truthy(value)) {
            None => self.ceasefire = 0,
            Some(value) => {
                let minutes = js_number(value);
                if (0.0..=gamesetup::CEASEFIRE_MAX).contains(&minutes) {
                    self.set_ceasefire(minutes);
                }
            }
        }

        // Cheats
        self.set_cheats(flag("CheatsEnabled"));

        // Daytime
        match legacy("Daytime").filter(|value| truthy(value)) {
            None => self.set_daytime(catalog, None),
            Some(value) => {
                let data = self.daytime.data.clone();
                let ids = gamesetup::data_ids_of(&data);
                let daytime = choice_among(value, |id| ids.iter().any(|known| known == id));
                self.set_daytime(catalog, Some(daytime));
            }
        }

        // DisableSpies, DisableTreasures
        self.set_disable_spies(flag("DisableSpies"));
        self.set_disable_treasures(flag("DisableTreasures"));

        // GameSpeed
        if let Some(speed) = attribs.get("gameSpeed").filter(|value| truthy(value)) {
            let speed = js_number(speed);
            if gamesetup::valid_game_speed(self, catalog, speed) {
                self.set_game_speed(speed);
            }
        }

        // Landscape
        match legacy("Landscape").filter(|value| truthy(value)) {
            None => self.set_landscape(catalog, None),
            Some(value) => {
                let landscape = match value.as_str() {
                    Some(id) if id != RANDOM => {
                        let group = LandscapeChoice::RandomGroup(id.to_string());
                        let item = LandscapeChoice::Fixed(id.to_string());
                        if gamesetup::valid_landscape(self, &group) {
                            group
                        } else if gamesetup::valid_landscape(self, &item) {
                            item
                        } else {
                            LandscapeChoice::Random
                        }
                    }
                    _ => LandscapeChoice::Random,
                };
                self.set_landscape(catalog, Some(landscape));
            }
        }

        // LastManStanding: assigned directly, as vanilla does.
        self.last_man_standing.enabled = flag("LastManStanding");

        // LockedTeams
        self.assign_locked_teams(flag("LockTeams"));

        // Map
        if let Some(kind) = attribs
            .get("mapType")
            .and_then(Value::as_str)
            .and_then(|name| MapType::ALL.into_iter().find(|kind| kind.name() == name))
        {
            self.set_map_type(catalog, kind);
        }
        if let Some(map) = attribs.get("map").and_then(Value::as_str) {
            let kind = self.map.kind;
            let known = catalog
                .maps
                .iter()
                .any(|candidate| candidate.path == map && Some(candidate.kind) == kind);
            if map == RANDOM && kind == Some(MapType::Random) {
                self.select_map(catalog, Choice::Random);
            } else if known {
                self.select_map(catalog, Choice::Fixed(map.to_string()));
            }
        }

        // MapExploration: direct, with no consistency fix, as vanilla.
        self.map_exploration.explored = flag("ExploreMap");
        self.map_exploration.revealed = flag("RevealMap");
        self.map_exploration.allied = flag("AllyView");

        // MapSize
        if let Some(tiles) = legacy("Size")
            .filter(|value| truthy(value))
            .and_then(Value::as_u64)
            .and_then(|tiles| u32::try_from(tiles).ok())
            && gamesetup::valid_map_size(self, catalog, tiles)
        {
            self.set_map_size(tiles);
        }

        // Nomad
        self.set_nomad(flag("Nomad"));

        // PlayerAI
        if players.is_some() {
            for index in 0..self.player_ai.len() {
                let ai = player_field(index, "AI")
                    .filter(|bot| truthy(bot))
                    .and_then(Value::as_str)
                    .and_then(|bot| ai_by_id(catalog, bot))
                    .map(|bot| {
                        let difficulty = player_field(index, "AIDiff")
                            .and_then(Value::as_u64)
                            .and_then(|level| u8::try_from(level).ok())
                            .filter(|level| gamesetup::valid_ai_difficulty(*level))
                            .unwrap_or(DEFAULT_AI_DIFFICULTY);
                        let behavior = player_field(index, "AIBehavior")
                            .and_then(Value::as_str)
                            .filter(|behavior| AI_BEHAVIORS.contains(behavior))
                            .unwrap_or(DEFAULT_AI_BEHAVIOR);
                        Ai {
                            bot: bot.id.clone(),
                            difficulty,
                            behavior: behavior.to_string(),
                        }
                    });
                self.set_player_ai(index, ai);
            }
        }

        // PlayerCiv
        for index in 0..self.player_civ.values.len() {
            let civ = player_field(index, "Civ")
                .filter(|civ| truthy(civ))
                .and_then(Value::as_str);
            let known = civ.is_some_and(|civ| {
                civ == RANDOM
                    || catalog
                        .civs
                        .iter()
                        .any(|known| known.selectable && known.code == civ)
            });
            if let (Some(civ), true) = (civ, known) {
                self.set_civ(catalog, index, Choice::from_name(civ));
            }
        }

        // PlayerColor
        for index in 0..self.player_color.values.len() {
            let Some(color) = player_field(index, "Color") else {
                continue;
            };
            let channel = |key: &str| {
                color
                    .get(key)
                    .and_then(Value::as_u64)
                    .and_then(|channel| u8::try_from(channel).ok())
            };
            if let (Some(r), Some(g), Some(b)) = (channel("r"), channel("g"), channel("b")) {
                self.set_color(index, [r, g, b]);
            }
        }

        // PlayerCount
        if self.map.map.is_some() {
            if self.map.kind != Some(MapType::Random) {
                if let Some(list) = self.map_players(catalog) {
                    let count = list.len();
                    self.assign_player_count(catalog, count);
                }
            } else if let Some(list) = players {
                self.assign_player_count(catalog, list.len().clamp(1, MAX_PLAYERS));
            }
        }

        // PlayerPlacement
        if let Some(value) = legacy("PlayerPlacement")
            .filter(|value| truthy(value))
            .and_then(Value::as_str)
            && self.player_placement.available.is_some()
        {
            let choice = Choice::from_name(value);
            if gamesetup::valid_placement(self, &choice) {
                self.player_placement.value = Some(choice);
            }
        }

        // PlayerTeam
        for index in 0..self.player_team.values.len() {
            if let Some(team) = player_field(index, "Team").and_then(as_i32)
                && (-1..gamesetup::MAX_TEAMS).contains(&team)
            {
                self.set_team(index, team);
            }
        }

        // Population
        if let Some(name) = legacy("PopulationCapType").and_then(Value::as_str)
            && catalog
                .population_capacities
                .iter()
                .any(|capacity| capacity.name == name)
        {
            self.set_pop_cap_type(catalog, name);
        }
        if let Some(cap) = legacy("PopulationCap")
            .and_then(Value::as_u64)
            .and_then(|cap| u32::try_from(cap).ok())
            && catalog
                .population_capacities
                .iter()
                .find(|capacity| capacity.name == self.population.cap_type)
                .is_some_and(|capacity| capacity.options.contains(&cap))
        {
            self.set_pop_cap(cap);
        }

        // RegicideGarrison: assigned directly, as vanilla does.
        self.regicide_garrison.enabled = flag("RegicideGarrison");

        // Relic
        if let Some(count) = legacy("RelicCount").filter(|value| truthy(value)) {
            let count = js_number(count);
            if (1.0..=catalog.civs.len() as f64).contains(&count) {
                self.set_relic_count(count);
            }
        }
        if let Some(minutes) = legacy("RelicDuration").filter(|value| truthy(value)) {
            let minutes = js_number(minutes);
            if (0.0..=gamesetup::RELIC_DURATION_MAX).contains(&minutes) {
                self.set_relic_duration(minutes);
            }
        }

        // SeaLevelRise
        if let Some(time) = legacy("SeaLevelRiseTime").filter(|value| truthy(value)) {
            let time = js_number(time);
            if !time.is_nan() {
                self.set_sea_level_rise(catalog, time);
            }
        }

        // StartingResources
        if let Some(resources) = legacy("StartingResources")
            .and_then(Value::as_u64)
            .and_then(|resources| u32::try_from(resources).ok())
            && catalog
                .starting_resources
                .iter()
                .any(|entry| entry.resources == resources)
        {
            self.set_starting_resources(resources);
        }

        // TriggerDifficulty
        if let Some(difficulty) = legacy("TriggerDifficulty")
            .and_then(Value::as_u64)
            .and_then(|level| u8::try_from(level).ok())
            && gamesetup::valid_trigger_difficulty(self, catalog, difficulty)
        {
            self.set_trigger_difficulty(difficulty);
        }

        // VictoryConditions
        if let Some(list) = legacy("VictoryConditions")
            .filter(|value| truthy(value))
            .and_then(Value::as_array)
        {
            let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
            self.victory_conditions.from_list(catalog, &names);
            self.assign_victory(catalog);
        }

        // WaterLevel
        if let Some(name) = legacy("waterLevel").and_then(Value::as_str)
            && gamesetup::valid_water_level(self, name)
        {
            self.set_water_level(name.to_string());
        }

        // Wonder
        if let Some(duration) = legacy("WonderDuration") {
            let duration = js_number(duration);
            if (0.0..=gamesetup::WONDER_DURATION_MAX).contains(&duration) {
                self.set_wonder_duration(duration);
            }
        }

        // PlayerRemoved
        if players.is_some() {
            for index in 0..self.player_removed.len() {
                let removed = player_field(index, "Removed").is_some_and(truthy);
                self.set_removed(index, removed);
            }
        }
    }
}
