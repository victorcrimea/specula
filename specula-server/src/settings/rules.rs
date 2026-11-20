//! The dependency rules of the engine's `gamesettings/attributes` classes
//! (and of `gui/gamesetup`'s `PlayerRemoved`): their setters and the
//! watchers they register on each other.
//!
//! Vanilla's `Observable` fires a watcher synchronously at the assignment,
//! only when the value differs (`trigger()` on `values` and `locked` is
//! unconditional but only GUI controls watch those). Each watched field here
//! has one `assign_*` method that compares, stores and then calls the
//! watchers in vanilla's registration order, and every write to such a field
//! goes through it, so cascades interleave as they do in JS.
//!
//! Rating is inert without a lobby (`Engine.HasXmppClient` is false):
//! `Rating.maybeUpdate` always leaves it unavailable and disabled, so
//! `rating.enabled` never changes and the watchers on it in Cheats and
//! LockedTeams never fire. `self.rating` is therefore left alone here.
//!
//! Vanilla throws where a map or a catalog lacks what a class reads; here
//! the value is left unset or unchanged, with a comment at each spot.

use serde_json::Value;

use super::Choice;
use super::DEFAULT_COLORS;
use super::LandscapeChoice;
use super::MAX_PLAYERS;
use super::RANDOM;
use super::Settings;
use super::js_number;
use super::js_round;
use super::truthy;
use crate::catalog::AiBot;
use crate::catalog::Catalog;
use crate::catalog::Map;
use crate::catalog::MapType;

/// `MapPreview.updatePreview`'s `cropped:` prefix: 400/512 and 300/512 as
/// JavaScript prints them.
fn preview_path(file: &str) -> String {
    format!("cropped:0.78125,0.5859375:session/icons/mappreview/{file}")
}

fn color_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    let d = |x: u8, y: u8| (f64::from(x) - f64::from(y)).powi(2);
    (d(a[0], b[0]) + d(a[1], b[1]) + d(a[2], b[2])).sqrt()
}

/// `PlayerColor._findFarthestUnusedColor`.
fn farthest_unused(values: &[[u8; 3]]) -> [u8; 3] {
    let mut farthest = DEFAULT_COLORS[0];
    let mut farthest_distance = 0.0;
    for default in DEFAULT_COLORS {
        let mut smallest = f64::INFINITY;
        for used in values {
            let distance = color_distance(*used, default);
            if distance < smallest {
                smallest = distance;
            }
        }
        // `>=`, so the last of equals wins, as in the class.
        if smallest >= farthest_distance {
            farthest = default;
            farthest_distance = smallest;
        }
    }
    farthest
}

fn as_color(value: &Value) -> Option<[u8; 3]> {
    let channel = |key: &str| {
        value
            .get(key)?
            .as_u64()
            .and_then(|channel| u8::try_from(channel).ok())
    };
    Some([channel("r")?, channel("g")?, channel("b")?])
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

impl Settings {
    // ---- accessors ----

    /// `Map.data`; a random map has none.
    pub(super) fn map_data<'a>(&self, catalog: &'a Catalog) -> Option<&'a Map> {
        match &self.map.map {
            Some(Choice::Fixed(path)) => catalog.maps.iter().find(|map| map.path == *path),
            _ => None,
        }
    }

    /// `GameSetting.getMapSetting`: `None` only when the key is absent.
    pub(super) fn map_setting<'a>(&self, catalog: &'a Catalog, key: &str) -> Option<&'a Value> {
        self.map_data(catalog)?.settings.get(key)
    }

    pub(super) fn map_players<'a>(&self, catalog: &'a Catalog) -> Option<&'a Vec<Value>> {
        self.map_setting(catalog, "PlayerData")?.as_array()
    }

    /// `PlayerData[index][key]`, the `_getMapData(i)` of PlayerCiv,
    /// PlayerColor and PlayerName. Vanilla throws on a `null` entry; it is
    /// missing here.
    pub(super) fn map_player_field<'a>(
        &self,
        catalog: &'a Catalog,
        index: usize,
        key: &str,
    ) -> Option<&'a Value> {
        self.map_players(catalog)?.get(index)?.as_object()?.get(key)
    }

    pub(super) fn is_scenario(&self) -> bool {
        self.map.kind == Some(MapType::Scenario)
    }

    /// The map value as a string: `"random"` or the path.
    fn map_value(&self) -> Option<&str> {
        match &self.map.map {
            Some(Choice::Random) => Some(RANDOM),
            Some(Choice::Fixed(path)) => Some(path),
            None => None,
        }
    }

    // ---- watched fields ----

    pub(super) fn assign_map(&mut self, catalog: &Catalog, map: Option<Choice<String>>) {
        if self.map.map != map {
            self.map.map = map;
            self.map_changed(catalog);
        }
    }

    pub(super) fn assign_map_type(&mut self, catalog: &Catalog, kind: Option<MapType>) {
        if self.map.kind != kind {
            let old = self.map.kind;
            self.map.kind = kind;
            self.type_changed(catalog, old);
        }
    }

    pub(super) fn assign_player_count(&mut self, catalog: &Catalog, n: usize) {
        if self.player_count.players != n {
            let old = self.player_count.players;
            self.player_count.players = n;
            self.players_changed(catalog, old);
        }
    }

    /// Call after mutating `victory_conditions`: the class always assigns a
    /// new Set, so the watchers always fire.
    pub(super) fn assign_victory(&mut self, catalog: &Catalog) {
        self.victory_changed(catalog);
    }

    pub(super) fn assign_locked_teams(&mut self, enabled: bool) {
        if self.locked_teams.enabled != enabled {
            self.locked_teams.enabled = enabled;
            self.lms_maybe_update();
        }
    }

    fn assign_cap_type(&mut self, name: &str) {
        if self.population.cap_type != name {
            self.population.cap_type = name.to_string();
            self.cap_type_changed();
        }
    }

    pub(super) fn assign_biome(&mut self, catalog: &Catalog, value: Option<Choice<String>>) {
        if self.biome.biome != value {
            self.biome.biome = value;
            self.update_preview(catalog);
        }
    }

    pub(super) fn assign_landscape(&mut self, catalog: &Catalog, value: Option<LandscapeChoice>) {
        if self.landscape.value != value {
            self.landscape.value = value;
            self.update_preview(catalog);
        }
    }

    pub(super) fn assign_daytime(&mut self, catalog: &Catalog, value: Option<Choice<String>>) {
        if self.daytime.value != value {
            self.daytime.value = value;
            self.update_preview(catalog);
        }
    }

    /// The class always assigns a new array, so the watcher always fires.
    fn assign_colors_available(&mut self, values: Vec<[u8; 3]>) {
        self.player_color.available = values;
        self.color_maybe_update();
    }

    // ---- the watcher lists ----

    /// The `map` watchers, in the class order of the VFS's byte order.
    fn map_changed(&mut self, catalog: &Catalog) {
        let scenario = self.is_scenario();
        let settings = self.map_data(catalog).map(|map| &map.settings);
        let get = |key: &str| settings.and_then(|settings| settings.get(key));
        let flag = |key: &str| get(key).is_some_and(truthy);
        let players = get("PlayerData").and_then(Value::as_array);
        let player_count = players.map_or(0, Vec::len);

        // Biome.onMapChange
        if let Some(supported) = get("SupportedBiomes") {
            let path = self.map_value().unwrap_or_default().to_string();
            let same = self.biome.cached.as_ref().is_some_and(|(cached, value)| {
                value == supported && (supported.is_string() || *cached == path)
            });
            if !same {
                self.biome.cached = Some((path, supported.clone()));
                self.biome.available = catalog
                    .biomes
                    .iter()
                    .filter(|biome| match supported {
                        Value::String(prefix) => biome.id.starts_with(prefix.as_str()),
                        Value::Array(ids) => ids.iter().any(|id| id.as_str() == Some(&biome.id)),
                        _ => false,
                    })
                    .map(|biome| biome.id.clone())
                    .collect();
                self.assign_biome(catalog, Some(Choice::Random));
            }
        } else if self.biome.cached.is_some() {
            self.biome.cached = None;
            self.biome.available.clear();
            self.assign_biome(catalog, None);
        }

        // Ceasefire.onMapChange
        if scenario {
            self.ceasefire = match get("Ceasefire") {
                Some(value) if truthy(value) => {
                    let minutes = js_number(value);
                    if minutes.is_nan() || minutes < 0.0 {
                        0
                    } else {
                        minutes as u32
                    }
                }
                _ => 0,
            };
        }

        // CircularMap.onMapChange
        self.circular_map = flag("CircularMap");

        // Daytime.onMapChange; `setDataValueHelper` assigns the data first.
        match get("Daytime").filter(|value| truthy(value)) {
            Some(data) => {
                self.daytime.data = Some(data.clone());
                self.assign_daytime(catalog, Some(Choice::Random));
            }
            None => {
                self.daytime.data = None;
                self.assign_daytime(catalog, None);
            }
        }

        if scenario {
            // DisableSpies and DisableTreasures
            self.disable_spies = flag("DisableSpies");
            self.disable_treasures = flag("DisableTreasures");
        }

        // GameSpeed.onMapChange
        if let Some(speed) = get("gameSpeed").filter(|value| truthy(value)) {
            self.game_speed = js_number(speed);
        }

        // Landscape.onMapChange: value, then data when there is none.
        match get("Landscapes").filter(|value| truthy(value)) {
            None => {
                self.assign_landscape(catalog, None);
                self.landscape.data = None;
            }
            Some(data) => {
                self.landscape.data = Some(data.clone());
                self.assign_landscape(catalog, Some(LandscapeChoice::Random));
            }
        }

        // LastManStanding.onMapChange
        if scenario {
            self.set_last_man_standing(flag("LastManStanding"));
        }

        // LockedTeams.onMapChange: for every map type, so choosing a map
        // unlocks the teams, as in vanilla.
        self.locked_teams.available = !scenario;
        self.assign_locked_teams(flag("LockTeams"));

        // Map.updateMapMetadata
        self.map.script = if self.map.kind == Some(MapType::Random) {
            get("Script").and_then(Value::as_str).map(str::to_string)
        } else {
            None
        };

        // MapExploration.onMapChange
        if scenario {
            self.map_exploration.explored = false;
            self.map_exploration.revealed = false;
            self.map_exploration.allied = false;
            if let Some(value) = get("ExploreMap") {
                self.set_explored(truthy(value));
            }
            if let Some(value) = get("RevealMap") {
                self.set_revealed(truthy(value));
            }
            if let Some(value) = get("AllyView") {
                self.set_allied(truthy(value));
            }
        }

        // MapName.onMapChange
        self.map_name = match get("Name").filter(|value| truthy(value)) {
            Some(name) => name.as_str().map(str::to_string),
            None => self.map_value().map(str::to_string),
        };

        // MapPreview.onMapChange
        self.update_preview(catalog);

        // PlayerCiv.onMapChange
        let map_civ =
            players.is_some_and(|list| list.iter().any(|data| data.get("Civ").is_some_and(truthy)));
        if scenario || map_civ {
            self.civ_resize(0);
            self.civ_maybe_update(catalog);
        } else {
            self.player_civ
                .locked
                .iter_mut()
                .for_each(|locked| *locked = false);
        }

        // PlayerColor.onMapChange
        self.player_color
            .locked
            .iter_mut()
            .for_each(|locked| *locked = scenario);
        if scenario {
            self.color_resize(0);
        }
        self.color_update_available(catalog);
        self.color_maybe_update();

        // PlayerCount.onMapChange
        if self.map.kind != Some(MapType::Random)
            && let Some(list) = players
        {
            self.assign_player_count(catalog, list.len());
            self.player_count.max = list.len();
        }

        // PlayerName.onMapChange
        self.name_resize(0);
        self.name_maybe_update(catalog);

        // PlayerPlacement.onMapChange
        match get("PlayerPlacements").filter(|value| truthy(value)) {
            None => {
                self.player_placement.value = None;
                self.player_placement.available = None;
            }
            Some(list) => {
                self.player_placement.available = Some(strings(list));
                self.player_placement.value = Some(Choice::Random);
            }
        }

        // PlayerTeam.onMapChange
        self.player_team
            .locked
            .iter_mut()
            .for_each(|locked| *locked = scenario);
        if scenario && let Some(list) = players {
            for (index, data) in list.iter().enumerate() {
                let team = data
                    .get("Team")
                    .and_then(Value::as_i64)
                    .and_then(|team| i32::try_from(team).ok())
                    .unwrap_or(-1);
                // JS would extend `values`, not `locked`, when the map has
                // more players than the list.
                while self.player_team.values.len() <= index {
                    self.player_team.values.push(-1);
                }
                self.player_team.values[index] = team;
            }
        }

        // Population.onMapChange
        self.population.per_player = None;
        if scenario {
            if players.is_some_and(|list| {
                list.iter()
                    .any(|data| data.get("PopulationLimit").is_some_and(truthy))
            }) {
                self.population.per_player = players.map(|list| {
                    list.iter()
                        .map(|data| {
                            data.get("PopulationLimit")
                                .filter(|limit| truthy(limit))
                                .and_then(Value::as_u64)
                                .and_then(|limit| u32::try_from(limit).ok())
                        })
                        .collect()
                });
            } else {
                let default = catalog
                    .population_capacities
                    .iter()
                    .find(|capacity| capacity.default)
                    .map(|capacity| capacity.name.clone())
                    .unwrap_or_else(|| self.population.cap_type.clone());
                let cap_type = get("PopulationCapType")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .map_or(default, str::to_string);
                self.set_pop_cap_type(catalog, &cap_type);
                if let Some(cap) = get("PopulationCap").filter(|cap| truthy(cap)) {
                    let cap = js_number(cap);
                    if cap >= 0.0 {
                        self.population.cap = cap as u32;
                    }
                }
            }
        }

        // RegicideGarrison.onMapChange
        if scenario {
            self.set_regicide_garrison(flag("RegicideGarrison"));
        }

        // Relic.onMapChange
        if scenario {
            match get("RelicCount").filter(|count| truthy(count)) {
                None => self.relic.available = false,
                Some(count) => {
                    // A missing duration is NaN in vanilla; 0 here.
                    let duration = get("RelicDuration").map_or(0.0, js_number);
                    let duration = if duration.is_nan() { 0.0 } else { duration };
                    self.relic_set(js_number(count), duration);
                }
            }
        }

        // SeaLevelRise.onMapChange
        match get("SeaLevelRise").filter(|rise| truthy(rise)) {
            None => self.sea_level_rise.value = None,
            Some(rise) => {
                self.sea_level_rise.min = rise.get("Min").and_then(Value::as_i64);
                self.sea_level_rise.max = rise.get("Max").and_then(Value::as_i64);
                self.sea_level_rise.value = rise.get("Default").and_then(Value::as_i64);
            }
        }

        // StartingCamera.onMapChange
        self.starting_camera.clear();
        for index in 0..player_count {
            self.starting_camera.push(
                players
                    .and_then(|list| list.get(index))
                    .and_then(|data| data.get("StartingCamera"))
                    .cloned(),
            );
        }

        // StartingResources.onMapChange
        self.starting_resources.per_player = None;
        if scenario {
            if players.is_some_and(|list| {
                list.iter()
                    .any(|data| data.get("Resources").is_some_and(truthy))
            }) {
                self.starting_resources.per_player = players.map(|list| {
                    list.iter()
                        .map(|data| data.get("Resources").filter(|r| truthy(r)).cloned())
                        .collect()
                });
            } else {
                match get("StartingResources").filter(|value| truthy(value)) {
                    None => self.starting_resources.resources = self.starting_resources.default,
                    Some(value) => {
                        let amount = js_number(value);
                        if amount >= 0.0 {
                            self.starting_resources.resources = amount as u32;
                        }
                    }
                }
            }
        }

        // TriggerDifficulty.onMapChange
        match get("SupportedTriggerDifficulties").filter(|value| truthy(value)) {
            None => {
                self.trigger_difficulty.value = None;
                self.trigger_difficulty.available = None;
            }
            Some(supported) => {
                let available = supported.get("Values").map(strings).unwrap_or_default();
                // Vanilla throws without a default among them; unset here.
                self.trigger_difficulty.value = catalog
                    .trigger_difficulties
                    .iter()
                    .find(|entry| entry.default && available.contains(&entry.name))
                    .map(|entry| entry.difficulty);
                self.trigger_difficulty.available = Some(available);
            }
        }

        // TriggerScripts.updateMapScripts
        self.trigger_scripts.map.clear();
        if let Some(scripts) = get("TriggerScripts") {
            for script in strings(scripts) {
                if !self.trigger_scripts.map.contains(&script) {
                    self.trigger_scripts.map.push(script);
                }
            }
        }

        // VictoryConditions.onMapChange
        if scenario && let Some(list) = get("VictoryConditions").filter(|list| truthy(list)) {
            let names = strings(list);
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            self.victory_conditions.from_list(catalog, &names);
            self.assign_victory(catalog);
        }

        // WaterLevel.onMapChange. Vanilla throws without a default option.
        self.water_level.options = get("WaterLevels").cloned();
        self.water_level.value = self
            .water_level
            .options
            .as_ref()
            .and_then(Value::as_array)
            .and_then(|options| {
                options
                    .iter()
                    .find(|option| option.get("Default").is_some_and(truthy))
            })
            .and_then(|option| option.get("Name")?.as_str().map(str::to_string));

        // Wonder.onMapChange
        if scenario {
            let duration = get("WonderDuration").map_or(0.0, js_number);
            self.set_wonder_duration(if duration.is_nan() { 0.0 } else { duration });
        }
    }

    /// The `type` watchers.
    fn type_changed(&mut self, catalog: &Catalog, old: Option<MapType>) {
        let random = self.map.kind == Some(MapType::Random);
        let was_random = old == Some(MapType::Random);
        // MapSize.onTypeChange
        if random && !was_random {
            self.set_map_size(self.map_size.default);
        } else if !random {
            self.map_size.available = false;
        }
        // PlayerCount.onMapTypeChange
        if random && !was_random {
            self.assign_player_count(catalog, 2);
        }
        if random {
            self.player_count.max = MAX_PLAYERS;
        }
    }

    /// The `nbPlayers` watchers.
    fn players_changed(&mut self, catalog: &Catalog, old: usize) {
        let n = self.player_count.players;
        // PlayerAI.maybeUpdate
        if self.player_ai.len() != n {
            self.player_ai.resize(n, None);
        }
        // PlayerCiv.maybeUpdate
        self.civ_maybe_update(catalog);
        // PlayerColor.maybeUpdate
        self.color_maybe_update();
        // PlayerName.maybeUpdate
        self.name_maybe_update(catalog);
        // PlayerTeam.maybeUpdate
        self.player_team.values.resize(n, -1);
        self.player_team.locked.resize(n, false);
        // Rating.maybeUpdate is inert.
        // StartingCamera.maybeUpdate
        if self.starting_camera.len() != n {
            self.starting_camera.resize(n, None);
        }
        // PlayerRemoved.maybeUpdate
        if self.player_removed.len() != n {
            self.player_removed.resize(n, false);
        }
        // The setup page's watcher registers after every class.
        self.fill_new_slots(catalog, old);
    }

    /// The `victoryConditions.active` watchers.
    fn victory_changed(&mut self, catalog: &Catalog) {
        self.set_regicide_garrison(self.regicide_garrison.enabled);
        self.relic_set(f64::from(self.relic.count), f64::from(self.relic.duration));
        self.trigger_scripts.victory = self.victory_conditions.scripts(catalog);
        self.set_wonder_duration(f64::from(self.wonder.duration));
    }

    /// `LockedTeams.onPopCapTypeChange`.
    fn cap_type_changed(&mut self) {
        let team = self.population.cap_type == "team";
        self.locked_teams.available = !team;
        self.assign_locked_teams(team);
    }

    /// `LastManStanding.maybeUpdate`.
    fn lms_maybe_update(&mut self) {
        self.set_last_man_standing(self.last_man_standing.enabled);
    }

    // ---- per-player list upkeep ----

    fn civ_resize(&mut self, n: usize) {
        self.player_civ.values.resize(n, Choice::Random);
        self.player_civ.locked.resize(n, false);
    }

    fn civ_maybe_update(&mut self, catalog: &Catalog) {
        self.civ_resize(self.player_count.players);
        for index in 0..self.player_civ.values.len() {
            let value = self.player_civ.values[index].clone();
            self.civ_set(catalog, index, value);
        }
    }

    /// `PlayerCiv._set`.
    fn civ_set(&mut self, catalog: &Catalog, index: usize, value: Choice<String>) {
        let from_map = self
            .map_player_field(catalog, index, "Civ")
            .filter(|civ| truthy(civ))
            .and_then(Value::as_str);
        let scenario = self.is_scenario();
        let (value, locked) = match from_map {
            Some(civ) => (Choice::Fixed(civ.to_string()), true),
            None => (value, scenario),
        };
        if let (Some(slot), Some(lock)) = (
            self.player_civ.values.get_mut(index),
            self.player_civ.locked.get_mut(index),
        ) {
            *slot = value;
            *lock = locked;
        }
    }

    fn color_resize(&mut self, n: usize) {
        self.player_color.values.resize(n, None);
        self.player_color.locked.resize(n, false);
    }

    fn color_maybe_update(&mut self) {
        self.color_resize(self.player_count.players);
        for index in 0..self.player_color.values.len() {
            // Live, since an earlier swap changes later entries; `forEach`
            // sees it too.
            let color = self.player_color.values[index];
            self.color_set(index, color);
        }
    }

    /// `PlayerColor._set`.
    fn color_set(&mut self, index: usize, color: Option<[u8; 3]>) {
        if index >= self.player_color.values.len() {
            return;
        }
        let in_use = color.and_then(|color| {
            self.player_color
                .values
                .iter()
                .position(|other| *other == Some(color))
        });
        if let Some(other) = in_use
            && other != index
        {
            let taken = self.player_color.values[index];
            self.player_color.values[index] = None;
            self.color_set(other, taken);
        }
        let available = &self.player_color.available;
        let chosen = match color {
            Some(color) if available.contains(&color) => Some(color),
            Some(color) => {
                let mut closest: Option<([u8; 3], f64)> = None;
                for candidate in available {
                    let distance = color_distance(color, *candidate);
                    if closest.is_none_or(|(_, best)| distance < best) {
                        closest = Some((*candidate, distance));
                    }
                }
                closest.map(|(color, _)| color)
            }
            None => available.iter().copied().find(|candidate| {
                self.player_color
                    .values
                    .iter()
                    .all(|other| *other != Some(*candidate))
            }),
        };
        self.player_color.values[index] = chosen;
    }

    /// `PlayerColor._updateAvailable`.
    fn color_update_available(&mut self, catalog: &Catalog) {
        let mut values: Vec<[u8; 3]> = Vec::new();
        let mut map_colors = false;
        for (index, default) in DEFAULT_COLORS.iter().enumerate() {
            let color = self
                .map_player_field(catalog, index, "Color")
                .filter(|color| truthy(color));
            if color.is_some() {
                map_colors = true;
            }
            if map_colors {
                let color = color
                    .and_then(as_color)
                    .unwrap_or_else(|| farthest_unused(&values));
                values.push(color);
            } else {
                values.push(*default);
            }
        }
        self.assign_colors_available(values);
    }

    fn name_resize(&mut self, n: usize) {
        self.player_name.resize(n, None);
    }

    fn name_maybe_update(&mut self, catalog: &Catalog) {
        self.name_resize(self.player_count.players);
        for index in 0..self.player_name.len() {
            self.name_set(catalog, index);
        }
    }

    /// `PlayerName._set`.
    fn name_set(&mut self, catalog: &Catalog, index: usize) {
        let name = self
            .map_player_field(catalog, index, "Name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| {
                catalog
                    .player_defaults
                    .get(index + 1)
                    .map(|default| default.name.clone())
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_default();
        if let Some(slot) = self.player_name.get_mut(index) {
            *slot = Some(name);
        }
    }

    // ---- MapPreview ----

    /// `MapPreview.updatePreview`, with `getPreviewForSubtype`,
    /// `getLandscapePreview` and `MapCache.getMapPreview`.
    pub(super) fn update_preview(&mut self, catalog: &Catalog) {
        let Some(value) = self.map_value() else {
            self.map_preview = None;
            return;
        };
        let base = value.rsplit('/').next().unwrap_or(value).to_string();
        let subtype = |choice: Option<String>| -> Option<String> {
            let part = choice?;
            let part = part.rsplit('/').next().unwrap_or(&part).to_string();
            let file = format!("{base}_{part}.png");
            catalog
                .previews
                .contains(&file)
                .then(|| preview_path(&file))
        };
        let string_of = |choice: &Option<Choice<String>>| match choice {
            Some(Choice::Random) => Some(RANDOM.to_string()),
            Some(Choice::Fixed(value)) => Some(value.clone()),
            None => None,
        };
        let landscape = || -> Option<String> {
            let id = match self.landscape.value.as_ref()? {
                LandscapeChoice::Random => RANDOM,
                LandscapeChoice::RandomGroup(id) | LandscapeChoice::Fixed(id) => id,
            };
            let groups = self.landscape.data.as_ref()?.as_array()?;
            // A group without items is skipped; vanilla would throw.
            for group in groups {
                let Some(items) = group.get("Items").and_then(Value::as_array) else {
                    continue;
                };
                for item in items {
                    if item.get("Id").and_then(Value::as_str) == Some(id) {
                        // Not existence-checked, like vanilla.
                        return item
                            .get("Preview")
                            .and_then(Value::as_str)
                            .map(preview_path);
                    }
                }
            }
            None
        };
        let fallback = || {
            let file = self
                .map_data(catalog)
                .and_then(|map| map.preview.clone())
                .unwrap_or_else(|| "nopreview.png".to_string());
            preview_path(&file)
        };
        self.map_preview = subtype(string_of(&self.biome.biome))
            .or_else(landscape)
            .or_else(|| subtype(string_of(&self.daytime.value)))
            .or_else(|| Some(fallback()));
    }

    // ---- setters ----

    /// `Map.setType`.
    pub fn set_map_type(&mut self, catalog: &Catalog, kind: MapType) {
        self.assign_map_type(catalog, Some(kind));
    }

    /// `Map.selectMap`.
    pub fn select_map(&mut self, catalog: &Catalog, map: Choice<String>) {
        self.assign_map(catalog, Some(map));
    }

    /// `Map.setRandomOptions`.
    pub fn set_random_options(&mut self, options: &[String]) {
        self.map.random_options = options
            .iter()
            .filter(|option| *option != RANDOM)
            .cloned()
            .collect();
    }

    /// `Biome.setBiome`.
    pub fn set_biome(&mut self, catalog: &Catalog, value: Option<Choice<String>>) {
        let value = if self.biome.available.is_empty() {
            None
        } else {
            Some(value.unwrap_or(Choice::Random))
        };
        self.assign_biome(catalog, value);
    }

    /// `Ceasefire.setValue`.
    pub fn set_ceasefire(&mut self, minutes: f64) {
        self.ceasefire = js_round(minutes).max(0.0) as u32;
    }

    /// `Cheats._set`, with rating inert.
    pub fn set_cheats(&mut self, enabled: bool) {
        self.cheats = enabled;
    }

    /// `Daytime.setValue`.
    pub fn set_daytime(&mut self, catalog: &Catalog, value: Option<Choice<String>>) {
        let value = self
            .daytime
            .data
            .is_some()
            .then(|| value.unwrap_or(Choice::Random));
        self.assign_daytime(catalog, value);
    }

    /// `DisableSpies.setEnabled`.
    pub fn set_disable_spies(&mut self, enabled: bool) {
        self.disable_spies = enabled;
    }

    /// `DisableTreasures.setEnabled`.
    pub fn set_disable_treasures(&mut self, enabled: bool) {
        self.disable_treasures = enabled;
    }

    /// `GameSpeed.setSpeed`.
    pub fn set_game_speed(&mut self, speed: f64) {
        self.game_speed = speed;
    }

    /// `Landscape.setValue`.
    pub fn set_landscape(&mut self, catalog: &Catalog, value: Option<LandscapeChoice>) {
        let value = self
            .landscape
            .data
            .is_some()
            .then(|| value.unwrap_or(LandscapeChoice::Random));
        self.assign_landscape(catalog, value);
    }

    /// `LastManStanding.setEnabled`.
    pub fn set_last_man_standing(&mut self, enabled: bool) {
        self.last_man_standing.available = !self.locked_teams.enabled;
        self.last_man_standing.enabled = enabled && self.last_man_standing.available;
    }

    /// `LockedTeams.setEnabled`.
    pub fn set_locked_teams(&mut self, enabled: bool) {
        self.assign_locked_teams(enabled);
    }

    /// `MapExploration.setExplored`.
    pub fn set_explored(&mut self, explored: bool) {
        self.map_exploration.explored = explored;
        self.map_exploration.revealed = self.map_exploration.revealed && explored;
    }

    /// `MapExploration.setRevealed`.
    pub fn set_revealed(&mut self, revealed: bool) {
        self.map_exploration.revealed = revealed;
        self.map_exploration.explored = self.map_exploration.explored || revealed;
    }

    /// `MapExploration.setAllied`.
    pub fn set_allied(&mut self, allied: bool) {
        self.map_exploration.allied = allied;
    }

    /// `MapSize.setSize`.
    pub fn set_map_size(&mut self, tiles: u32) {
        self.map_size.available = self.map.kind == Some(MapType::Random);
        self.map_size.size = tiles;
    }

    /// `Nomad.setEnabled`.
    pub fn set_nomad(&mut self, nomad: bool) {
        self.nomad = nomad;
    }

    /// `PlayerAI.set`.
    pub fn set_player_ai(&mut self, index: usize, ai: Option<super::Ai>) {
        if let Some(slot) = self.player_ai.get_mut(index) {
            *slot = ai;
        }
    }

    /// `PlayerAI.setAI`. Vanilla throws on an empty slot; it stays empty.
    pub fn set_ai_bot(&mut self, index: usize, bot: Option<String>) {
        let Some(slot) = self.player_ai.get_mut(index) else {
            return;
        };
        match (bot, slot.as_mut()) {
            (None, _) => *slot = None,
            (Some(bot), Some(ai)) => ai.bot = bot,
            (Some(_), None) => {}
        }
    }

    /// `PlayerAI.setDifficulty`.
    pub fn set_ai_difficulty(&mut self, index: usize, difficulty: u8) {
        if let Some(Some(ai)) = self.player_ai.get_mut(index) {
            ai.difficulty = difficulty;
        }
    }

    /// `PlayerAI.setBehavior`.
    pub fn set_ai_behavior(&mut self, index: usize, behavior: String) {
        if let Some(Some(ai)) = self.player_ai.get_mut(index) {
            ai.behavior = behavior;
        }
    }

    /// `PlayerCiv.setValue`.
    pub fn set_civ(&mut self, catalog: &Catalog, index: usize, civ: Choice<String>) {
        self.civ_set(catalog, index, civ);
    }

    /// `PlayerColor.setColor`.
    pub fn set_color(&mut self, index: usize, color: [u8; 3]) {
        self.color_set(index, Some(color));
    }

    /// `PlayerCount.setNb`.
    pub fn set_player_count(&mut self, catalog: &Catalog, n: usize) {
        let n = n.clamp(1, self.player_count.max.max(1));
        self.assign_player_count(catalog, n);
    }

    /// `PlayerPlacement.set`.
    pub fn set_placement(&mut self, value: Option<Choice<String>>) -> Result<(), &'static str> {
        if self.player_placement.available.is_some() {
            self.player_placement.value = Some(value.unwrap_or(Choice::Random));
            Ok(())
        } else if value.is_some() {
            Err("map has no placements")
        } else {
            Ok(())
        }
    }

    /// `PlayerTeam.setValue`.
    pub fn set_team(&mut self, index: usize, team: i32) {
        if let Some(slot) = self.player_team.values.get_mut(index) {
            *slot = team;
        }
    }

    /// `Population.setPopCap`.
    pub fn set_pop_cap(&mut self, cap: u32) {
        self.population.cap = cap;
    }

    /// `Population.setPopCapType`. Vanilla throws on an unknown type; nothing
    /// changes here.
    pub fn set_pop_cap_type(&mut self, catalog: &Catalog, name: &str) {
        let Some(entry) = catalog
            .population_capacities
            .iter()
            .find(|capacity| capacity.name == name)
        else {
            return;
        };
        self.assign_cap_type(name);
        self.population.cap = entry.option_default;
    }

    /// `Relic._set`.
    fn relic_set(&mut self, count: f64, duration: f64) {
        self.relic.available = self
            .victory_conditions
            .active
            .iter()
            .any(|name| name == "capture_the_relic");
        self.relic.count = count.max(1.0) as u32;
        self.relic.duration = duration.max(0.0) as u32;
    }

    /// `Relic.setCount`.
    pub fn set_relic_count(&mut self, count: f64) {
        self.relic_set(js_round(count), f64::from(self.relic.duration));
    }

    /// `Relic.setDuration`.
    pub fn set_relic_duration(&mut self, duration: f64) {
        self.relic_set(f64::from(self.relic.count), js_round(duration));
    }

    /// `RegicideGarrison.setEnabled`.
    pub fn set_regicide_garrison(&mut self, enabled: bool) {
        self.regicide_garrison.available = self
            .victory_conditions
            .active
            .iter()
            .any(|name| name == "regicide");
        self.regicide_garrison.enabled = enabled && self.regicide_garrison.available;
    }

    /// `SeaLevelRise.setValue`.
    pub fn set_sea_level_rise(&mut self, catalog: &Catalog, value: f64) {
        if !self
            .map_setting(catalog, "SeaLevelRise")
            .is_some_and(truthy)
        {
            self.sea_level_rise.value = None;
            return;
        }
        if let (Some(min), Some(max)) = (self.sea_level_rise.min, self.sea_level_rise.max) {
            self.sea_level_rise.value = Some((js_round(value) as i64).clamp(min, max.max(min)));
        }
    }

    /// `StartingResources.setResources`.
    pub fn set_starting_resources(&mut self, resources: u32) {
        self.starting_resources.resources = resources;
    }

    /// `TriggerDifficulty.setValue`.
    pub fn set_trigger_difficulty(&mut self, difficulty: u8) {
        self.trigger_difficulty.value = Some(difficulty);
    }

    /// `VictoryConditions.setEnabled`.
    pub fn set_victory_condition(&mut self, catalog: &Catalog, name: &str, enabled: bool) {
        if enabled {
            self.victory_conditions.add(catalog, &[name]);
        } else {
            self.victory_conditions.delete(catalog, &[name]);
        }
        self.assign_victory(catalog);
    }

    /// The water level control assigns `value` directly.
    pub fn set_water_level(&mut self, name: String) {
        self.water_level.value = Some(name);
    }

    /// `Wonder.setDuration`.
    pub fn set_wonder_duration(&mut self, duration: f64) {
        self.wonder.available = self
            .victory_conditions
            .active
            .iter()
            .any(|name| name == "wonder");
        self.wonder.duration = js_round(duration).max(0.0) as u32;
    }

    /// `PlayerRemoved.set`.
    pub fn set_removed(&mut self, index: usize, removed: bool) {
        if let Some(slot) = self.player_removed.get_mut(index) {
            *slot = removed;
        }
    }

    /// `PlayerAI.swap`.
    pub fn swap_ai(&mut self, a: usize, b: usize) {
        if a < self.player_ai.len() && b < self.player_ai.len() {
            self.player_ai.swap(a, b);
        }
    }

    /// `PlayerCiv.swap`.
    pub fn swap_civ(&mut self, a: usize, b: usize) {
        if a < self.player_civ.values.len() && b < self.player_civ.values.len() {
            self.player_civ.values.swap(a, b);
            self.player_civ.locked.swap(a, b);
        }
    }

    /// `PlayerColor.swap`.
    pub fn swap_color(&mut self, a: usize, b: usize) {
        if a < self.player_color.values.len() && b < self.player_color.values.len() {
            self.player_color.values.swap(a, b);
            self.player_color.locked.swap(a, b);
        }
    }

    /// `PlayerRemoved.swap`.
    pub fn swap_removed(&mut self, a: usize, b: usize) {
        if a < self.player_removed.len() && b < self.player_removed.len() {
            self.player_removed.swap(a, b);
        }
    }
}

/// Whether `bot` is an AI of the catalog.
pub(super) fn ai_by_id<'a>(catalog: &'a Catalog, bot: &str) -> Option<&'a AiBot> {
    catalog.ais.iter().find(|ai| ai.id == bot)
}
