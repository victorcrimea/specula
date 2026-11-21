//! Vanilla's `gui/gamesetup` layer over `GameSettings`: the map filter and
//! list, the AIs filling new slots, the AI dropped when a human takes the
//! slot, the fast-forward speeds, the civ reset on unlock, and every value
//! checked against what its control offers. This is the only entry the setup
//! protocol uses.

use serde_json::Value;

use super::Ai;
use super::Choice;
use super::LandscapeChoice;
use super::MAX_PLAYERS;
use super::RANDOM;
use super::Settings;
use super::rules::ai_by_id;
use crate::catalog::AI_BEHAVIORS;
use crate::catalog::AI_DIFFICULTIES;
use crate::catalog::Catalog;
use crate::catalog::DEFAULT_AI_BEHAVIOR;
use crate::catalog::DEFAULT_AI_DIFFICULTY;
use crate::catalog::Map;
use crate::catalog::MapType;

/// `MapFilters.prototype.Filters` (`gui/maps/MapFilters.js:88`).
pub struct MapFilter {
    pub name: &'static str,
    pub title: &'static str,
    pub matches: &'static [&'static str],
}

pub const MAP_FILTERS: [MapFilter; 7] = [
    MapFilter {
        name: "default",
        title: "Default",
        matches: &["!naval !demo !hidden"],
    },
    MapFilter {
        name: "naval",
        title: "Naval Maps",
        matches: &["naval"],
    },
    MapFilter {
        name: "demo",
        title: "Demo Maps",
        matches: &["demo"],
    },
    MapFilter {
        name: "new",
        title: "New Maps",
        matches: &["new"],
    },
    MapFilter {
        name: "multiplayer",
        title: "Best for MP",
        matches: &["multiplayer"],
    },
    MapFilter {
        name: "trigger",
        title: "Trigger Maps",
        matches: &["trigger"],
    },
    MapFilter {
        name: "all",
        title: "All Maps",
        matches: &["!"],
    },
];
/// `loadMapTypes`' `Default`.
pub const DEFAULT_MAP_TYPE: MapType = MapType::Skirmish;
/// `g_MaxTeams`.
pub const MAX_TEAMS: i32 = 4;
/// `Sliders/Ceasefire.js`.
pub const CEASEFIRE_MAX: f64 = 45.0;
/// `Sliders/RelicDuration.js`.
pub const RELIC_DURATION_MAX: f64 = 60.0;
/// `Sliders/WonderDuration.js`.
pub const WONDER_DURATION_MAX: f64 = 60.0;
/// `Sliders/SeaLevelRiseTime.js`.
pub const SEA_LEVEL_RISE_MAX: f64 = 60.0;
/// `GameSpeed.init` does not read the catalog's default.
const FALLBACK_GAME_SPEED: f64 = 1.0;

/// `MatchesClassList` (`globalscripts/Templates.js:84`).
fn matches_class_list(keywords: &[String], matches: &[&str]) -> bool {
    matches.iter().any(|entry| {
        entry
            .split(|c: char| c == '+' || c.is_whitespace())
            .filter(|token| !token.is_empty())
            .all(|token| match token.strip_prefix('!') {
                Some(keyword) => !keywords.iter().any(|k| k == keyword),
                None => keywords.iter().any(|k| k == token),
            })
    })
}

/// `getFilteredMaps` with `MapSelection.updateMapList`'s sort, by the English
/// name.
pub fn map_list<'a>(catalog: &'a Catalog, kind: MapType, filter: &str) -> Vec<&'a Map> {
    let Some(filter) = MAP_FILTERS
        .iter()
        .find(|candidate| candidate.name == filter)
    else {
        return Vec::new();
    };
    let mut maps: Vec<&Map> = catalog
        .maps
        .iter()
        .filter(|map| map.kind == kind && matches_class_list(&map.keywords, filter.matches))
        .collect();
    maps.sort_by_key(|map| map.name.to_lowercase());
    maps
}

/// `getAvailableMapFilters`.
pub fn available_filters(catalog: &Catalog, kind: MapType) -> Vec<&'static MapFilter> {
    MAP_FILTERS
        .iter()
        .filter(|filter| !map_list(catalog, kind, filter.name).is_empty())
        .collect()
}

/// Index 0 is the default, as `prepareForDropdown` without a `Default`.
pub(crate) fn map_choices(catalog: &Catalog, kind: MapType, filter: &str) -> Vec<String> {
    let mut choices = Vec::new();
    if kind == MapType::Random {
        choices.push(RANDOM.to_string());
    }
    choices.extend(
        map_list(catalog, kind, filter)
            .into_iter()
            .map(|map| map.path.clone()),
    );
    choices
}

fn choice_string(choice: &Choice<String>) -> &str {
    match choice {
        Choice::Random => RANDOM,
        Choice::Fixed(path) => path,
    }
}

// ---- validity, shared with `from_init` ----

pub(super) fn valid_filter(settings: &Settings, catalog: &Catalog, filter: &str) -> bool {
    settings.map.kind.is_some_and(|kind| {
        available_filters(catalog, kind)
            .iter()
            .any(|available| available.name == filter)
    })
}

pub(super) fn valid_map(settings: &Settings, catalog: &Catalog, map: &Choice<String>) -> bool {
    settings.map.kind.is_some_and(|kind| {
        map_choices(catalog, kind, &settings.gui.map_filter)
            .iter()
            .any(|choice| choice == choice_string(map))
    })
}

pub(super) fn valid_map_size(settings: &Settings, catalog: &Catalog, tiles: u32) -> bool {
    settings.map_size.available && catalog.map_sizes.iter().any(|size| size.tiles == tiles)
}

pub(super) fn valid_biome(settings: &Settings, biome: &Choice<String>) -> bool {
    !settings.biome.available.is_empty()
        && match biome {
            Choice::Random => true,
            Choice::Fixed(id) => settings.biome.available.contains(id),
        }
}

fn data_ids(data: &Option<Value>) -> impl Iterator<Item = &str> {
    data.iter()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|item| item.get("Id")?.as_str())
}

pub(super) fn data_ids_of(data: &Option<Value>) -> Vec<String> {
    data_ids(data).map(str::to_string).collect()
}

pub(super) fn valid_daytime(settings: &Settings, daytime: &Choice<String>) -> bool {
    settings.daytime.data.is_some()
        && match daytime {
            Choice::Random => true,
            Choice::Fixed(id) => data_ids(&settings.daytime.data).any(|known| known == id),
        }
}

pub(super) fn valid_landscape(settings: &Settings, landscape: &LandscapeChoice) -> bool {
    let data = &settings.landscape.data;
    if data.is_none() {
        return false;
    }
    match landscape {
        LandscapeChoice::Random => true,
        LandscapeChoice::RandomGroup(id) => data_ids(data).any(|known| known == id),
        LandscapeChoice::Fixed(id) => data
            .iter()
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|group| group.get("Items")?.as_array())
            .flatten()
            .any(|item| item.get("Id").and_then(Value::as_str) == Some(id)),
    }
}

pub(super) fn valid_placement(settings: &Settings, placement: &Choice<String>) -> bool {
    settings
        .player_placement
        .available
        .as_ref()
        .is_some_and(|available| match placement {
            Choice::Random => true,
            Choice::Fixed(id) => available.contains(id),
        })
}

pub(super) fn valid_water_level(settings: &Settings, name: &str) -> bool {
    settings
        .water_level
        .options
        .as_ref()
        .and_then(Value::as_array)
        .is_some_and(|options| {
            options
                .iter()
                .any(|option| option.get("Name").and_then(Value::as_str) == Some(name))
        })
}

pub(super) fn valid_trigger_difficulty(
    settings: &Settings,
    catalog: &Catalog,
    difficulty: u8,
) -> bool {
    settings
        .trigger_difficulty
        .available
        .as_ref()
        .is_some_and(|available| {
            catalog
                .trigger_difficulties
                .iter()
                .any(|entry| entry.difficulty == difficulty && available.contains(&entry.name))
        })
}

pub(super) fn valid_ceasefire(settings: &Settings, minutes: f64) -> bool {
    !settings.is_scenario() && (0.0..=CEASEFIRE_MAX).contains(&minutes)
}

pub(super) fn valid_relic_count(settings: &Settings, catalog: &Catalog, count: f64) -> bool {
    settings.relic.available
        && !settings.is_scenario()
        && (1.0..=catalog.civs.len() as f64).contains(&count)
}

pub(super) fn valid_relic_duration(settings: &Settings, minutes: f64) -> bool {
    settings.relic.available
        && !settings.is_scenario()
        && (0.0..=RELIC_DURATION_MAX).contains(&minutes)
}

pub(super) fn valid_wonder_duration(settings: &Settings, minutes: f64) -> bool {
    settings.wonder.available
        && !settings.is_scenario()
        && (0.0..=WONDER_DURATION_MAX).contains(&minutes)
}

pub(super) fn valid_starting_resources(
    settings: &Settings,
    catalog: &Catalog,
    resources: u32,
) -> bool {
    !settings.is_scenario()
        && settings.starting_resources.per_player.is_none()
        && catalog
            .starting_resources
            .iter()
            .any(|entry| entry.resources == resources)
}

pub(super) fn valid_pop_cap_type(settings: &Settings, catalog: &Catalog, name: &str) -> bool {
    !settings.is_scenario()
        && catalog
            .population_capacities
            .iter()
            .any(|capacity| capacity.name == name)
}

pub(super) fn valid_pop_cap(settings: &Settings, catalog: &Catalog, cap: u32) -> bool {
    !settings.is_scenario()
        && settings.population.per_player.is_none()
        && catalog
            .population_capacities
            .iter()
            .find(|capacity| capacity.name == settings.population.cap_type)
            .is_some_and(|capacity| capacity.options.contains(&cap))
}

pub(super) fn valid_game_speed(settings: &Settings, catalog: &Catalog, speed: f64) -> bool {
    catalog.game_speeds.iter().any(|entry| {
        entry.speed == speed
            && (!entry.fast_forward || settings.gui.allow_fast_forward != Some(false))
    })
}

pub(super) fn valid_civ(
    settings: &Settings,
    catalog: &Catalog,
    index: usize,
    civ: &Choice<String>,
) -> bool {
    index < settings.player_count.players
        && settings.player_civ.locked.get(index) != Some(&true)
        && match civ {
            Choice::Random => true,
            Choice::Fixed(code) => catalog
                .civs
                .iter()
                .any(|known| known.selectable && known.code == *code),
        }
}

pub(super) fn valid_team(settings: &Settings, index: usize, team: i32) -> bool {
    index < settings.player_count.players
        && !settings.is_scenario()
        && (-1..MAX_TEAMS).contains(&team)
}

pub(super) fn valid_color(settings: &Settings, index: usize, color: [u8; 3]) -> bool {
    index < settings.player_count.players
        && !settings.is_scenario()
        && settings.player_color.available.contains(&color)
}

pub(super) fn valid_ai_difficulty(difficulty: u8) -> bool {
    usize::from(difficulty) < AI_DIFFICULTIES.len()
}

pub(super) fn valid_ai_behavior(behavior: &str) -> bool {
    AI_BEHAVIORS.contains(&behavior)
}

// ---- changes ----

pub enum Change {
    MapType(MapType),
    MapFilter(String),
    Map(Choice<String>),
    /// `MapBrowserPage.onSubmitMapSelection`.
    // The setup page has no map browser, so the protocol never builds it.
    #[allow(dead_code)]
    BrowseMap {
        kind: MapType,
        filter: String,
        map: Choice<String>,
    },
    MapSize(u32),
    PlayerCount(usize),
    Biome(Choice<String>),
    Daytime(Choice<String>),
    Landscape(LandscapeChoice),
    Placement(Choice<String>),
    WaterLevel(String),
    TriggerDifficulty(u8),
    SeaLevelRise(f64),
    Ceasefire(f64),
    RelicCount(f64),
    RelicDuration(f64),
    WonderDuration(f64),
    StartingResources(u32),
    PopulationCapType(String),
    PopulationCap(u32),
    GameSpeed(f64),
    VictoryCondition {
        name: String,
        enabled: bool,
    },
    Explored(bool),
    Revealed(bool),
    Allied(bool),
    DisableSpies(bool),
    DisableTreasures(bool),
    Nomad(bool),
    LastManStanding(bool),
    RegicideGarrison(bool),
    LockedTeams(bool),
    Cheats(bool),
    Civ {
        index: usize,
        civ: Choice<String>,
    },
    Team {
        index: usize,
        team: i32,
    },
    Color {
        index: usize,
        color: [u8; 3],
    },
    /// `PlayerAssignmentItem.AI`: an AI takes the slot.
    Ai {
        index: usize,
        bot: String,
    },
    /// The AI config page's controls.
    AiBot {
        index: usize,
        bot: Option<String>,
    },
    AiDifficulty {
        index: usize,
        difficulty: u8,
    },
    AiBehavior {
        index: usize,
        behavior: String,
    },
    /// `PlayerAssignmentItem.Unassigned`.
    Unassign {
        index: usize,
    },
    /// `PlayerAssignmentItem.Removed`.
    Remove {
        index: usize,
    },
    ResetCivs,
    ResetTeams,
}

impl Settings {
    /// `MapFilter.checkMapTypeChange`.
    fn check_filter(&mut self, catalog: &Catalog) {
        let Some(kind) = self.map.kind else {
            return;
        };
        let filters = available_filters(catalog, kind);
        if !filters
            .iter()
            .any(|filter| filter.name == self.gui.map_filter)
            && let Some(first) = filters.first()
        {
            self.gui.map_filter = first.name.to_string();
        }
    }

    /// `MapSelection.updateMapList`.
    fn update_map_list(&mut self, catalog: &Catalog) {
        let Some(kind) = self.map.kind else {
            return;
        };
        // Vanilla gives up on an empty list before it adds "random".
        if map_list(catalog, kind, &self.gui.map_filter).is_empty() {
            return;
        }
        let choices = map_choices(catalog, kind, &self.gui.map_filter);
        self.set_random_options(&choices);
        let current = self.map.map.as_ref().map(choice_string);
        if !current.is_some_and(|current| choices.iter().any(|choice| choice == current)) {
            self.select_map(catalog, Choice::from_name(&choices[0]));
        }
    }

    /// The GUI's type watchers, which register after the classes'.
    fn choose_type(&mut self, catalog: &Catalog, kind: MapType) {
        self.set_map_type(catalog, kind);
        self.check_filter(catalog);
        self.update_map_list(catalog);
    }

    /// Whether `change` is valid now, with a reason for the log if not.
    fn check_change(
        &self,
        catalog: &Catalog,
        change: &Change,
        seated: &[u8],
    ) -> Result<(), &'static str> {
        let count = self.player_count.players;
        let scenario = self.is_scenario();
        let ok = |valid: bool, reason: &'static str| if valid { Ok(()) } else { Err(reason) };
        match change {
            Change::MapType(_) | Change::Cheats(_) | Change::ResetCivs | Change::ResetTeams => {
                if scenario && matches!(change, Change::ResetCivs | Change::ResetTeams) {
                    return Err("scenario");
                }
                Ok(())
            }
            Change::MapFilter(filter) => ok(valid_filter(self, catalog, filter), "filter"),
            Change::Map(map) => ok(valid_map(self, catalog, map), "map"),
            Change::BrowseMap { kind, filter, map } => ok(
                available_filters(catalog, *kind)
                    .iter()
                    .any(|available| available.name == filter)
                    && map_choices(catalog, *kind, filter)
                        .iter()
                        .any(|choice| choice == choice_string(map)),
                "map",
            ),
            Change::MapSize(tiles) => ok(valid_map_size(self, catalog, *tiles), "map size"),
            Change::PlayerCount(n) => ok(
                self.map.kind == Some(MapType::Random) && (1..=MAX_PLAYERS).contains(n),
                "player count",
            ),
            Change::Biome(biome) => ok(valid_biome(self, biome), "biome"),
            Change::Daytime(daytime) => ok(valid_daytime(self, daytime), "daytime"),
            Change::Landscape(landscape) => ok(valid_landscape(self, landscape), "landscape"),
            Change::Placement(placement) => ok(valid_placement(self, placement), "placement"),
            Change::WaterLevel(name) => ok(valid_water_level(self, name), "water level"),
            Change::TriggerDifficulty(difficulty) => ok(
                valid_trigger_difficulty(self, catalog, *difficulty),
                "trigger difficulty",
            ),
            Change::SeaLevelRise(minutes) => ok(
                self.sea_level_rise.value.is_some()
                    && !scenario
                    && (0.0..=SEA_LEVEL_RISE_MAX).contains(minutes),
                "sea level",
            ),
            Change::Ceasefire(minutes) => ok(valid_ceasefire(self, *minutes), "ceasefire"),
            Change::RelicCount(count) => {
                ok(valid_relic_count(self, catalog, *count), "relic count")
            }
            Change::RelicDuration(minutes) => {
                ok(valid_relic_duration(self, *minutes), "relic duration")
            }
            Change::WonderDuration(minutes) => {
                ok(valid_wonder_duration(self, *minutes), "wonder duration")
            }
            Change::StartingResources(resources) => ok(
                valid_starting_resources(self, catalog, *resources),
                "starting resources",
            ),
            Change::PopulationCapType(name) => {
                ok(valid_pop_cap_type(self, catalog, name), "population type")
            }
            Change::PopulationCap(cap) => ok(valid_pop_cap(self, catalog, *cap), "population cap"),
            Change::GameSpeed(speed) => ok(valid_game_speed(self, catalog, *speed), "game speed"),
            Change::VictoryCondition { name, enabled } => ok(
                !scenario
                    && catalog
                        .victory_conditions
                        .iter()
                        .any(|condition| condition.name == *name)
                    && !(*enabled && self.victory_conditions.disabled.contains(name)),
                "victory condition",
            ),
            Change::Explored(_)
            | Change::Revealed(_)
            | Change::Allied(_)
            | Change::DisableSpies(_)
            | Change::DisableTreasures(_) => ok(!scenario, "scenario"),
            Change::Nomad(_) => ok(self.map.kind == Some(MapType::Random), "not random"),
            Change::LastManStanding(_) => ok(
                !scenario && self.last_man_standing.available,
                "last man standing",
            ),
            Change::RegicideGarrison(_) => ok(
                !scenario && self.regicide_garrison.available,
                "regicide garrison",
            ),
            Change::LockedTeams(_) => ok(self.locked_teams.available, "locked teams"),
            Change::Civ { index, civ } => ok(valid_civ(self, catalog, *index, civ), "civ"),
            Change::Team { index, team } => ok(valid_team(self, *index, *team), "team"),
            Change::Color { index, color } => ok(valid_color(self, *index, *color), "color"),
            Change::Ai { index, bot } => ok(
                *index < count
                    && !seated.contains(&(*index as u8 + 1))
                    && ai_by_id(catalog, bot).is_some_and(|ai| !ai.hidden),
                "ai",
            ),
            Change::AiBot { index, bot } => ok(
                self.player_ai.get(*index).is_some_and(Option::is_some)
                    && bot
                        .as_deref()
                        .is_none_or(|bot| ai_by_id(catalog, bot).is_some()),
                "ai bot",
            ),
            Change::AiDifficulty { index, difficulty } => ok(
                self.player_ai.get(*index).is_some_and(Option::is_some)
                    && valid_ai_difficulty(*difficulty),
                "ai difficulty",
            ),
            Change::AiBehavior { index, behavior } => ok(
                self.player_ai.get(*index).is_some_and(Option::is_some)
                    && valid_ai_behavior(behavior),
                "ai behavior",
            ),
            Change::Unassign { index } | Change::Remove { index } => ok(*index < count, "slot"),
        }
    }

    /// Applies `change` if it is valid, else leaves the settings untouched.
    /// `seated` are the seated humans' player numbers, 1-based. The reasons
    /// are for the log only.
    pub fn apply(
        &mut self,
        catalog: &Catalog,
        change: Change,
        seated: &[u8],
    ) -> Result<(), &'static str> {
        self.check_change(catalog, &change, seated)?;
        let count = self.player_count.players;
        match change {
            Change::MapType(kind) => self.choose_type(catalog, kind),
            Change::MapFilter(filter) => {
                self.gui.map_filter = filter;
                self.update_map_list(catalog);
            }
            Change::Map(map) => self.select_map(catalog, map),
            Change::BrowseMap { kind, filter, map } => {
                // Vanilla's order, intermediate cascades included.
                self.choose_type(catalog, kind);
                self.gui.map_filter = filter;
                self.update_map_list(catalog);
                self.select_map(catalog, map);
            }
            Change::MapSize(tiles) => self.set_map_size(tiles),
            Change::PlayerCount(n) => self.set_player_count(catalog, n),
            Change::Biome(biome) => self.set_biome(catalog, Some(biome)),
            Change::Daytime(daytime) => self.set_daytime(catalog, Some(daytime)),
            Change::Landscape(landscape) => self.set_landscape(catalog, Some(landscape)),
            Change::Placement(placement) => {
                self.set_placement(Some(placement))?;
            }
            Change::WaterLevel(name) => self.set_water_level(name),
            Change::TriggerDifficulty(difficulty) => self.set_trigger_difficulty(difficulty),
            Change::SeaLevelRise(value) => self.set_sea_level_rise(catalog, value),
            Change::Ceasefire(minutes) => self.set_ceasefire(minutes),
            Change::RelicCount(count) => self.set_relic_count(count),
            Change::RelicDuration(minutes) => self.set_relic_duration(minutes),
            Change::WonderDuration(minutes) => self.set_wonder_duration(minutes),
            Change::StartingResources(resources) => self.set_starting_resources(resources),
            Change::PopulationCapType(name) => self.set_pop_cap_type(catalog, &name),
            Change::PopulationCap(cap) => self.set_pop_cap(cap),
            Change::GameSpeed(speed) => self.set_game_speed(speed),
            Change::VictoryCondition { name, enabled } => {
                self.set_victory_condition(catalog, &name, enabled);
            }
            Change::Explored(value) => self.set_explored(value),
            Change::Revealed(value) => self.set_revealed(value),
            Change::Allied(value) => self.set_allied(value),
            Change::DisableSpies(value) => self.set_disable_spies(value),
            Change::DisableTreasures(value) => self.set_disable_treasures(value),
            Change::Nomad(value) => self.set_nomad(value),
            Change::LastManStanding(value) => self.set_last_man_standing(value),
            Change::RegicideGarrison(value) => self.set_regicide_garrison(value),
            Change::LockedTeams(value) => self.set_locked_teams(value),
            Change::Cheats(value) => self.set_cheats(value),
            Change::Civ { index, civ } => self.set_civ(catalog, index, civ),
            Change::Team { index, team } => self.set_team(index, team),
            Change::Color { index, color } => self.set_color(index, color),
            Change::Ai { index, bot } => {
                self.set_player_ai(
                    index,
                    Some(Ai {
                        bot,
                        difficulty: DEFAULT_AI_DIFFICULTY,
                        behavior: DEFAULT_AI_BEHAVIOR.to_string(),
                    }),
                );
                self.set_removed(index, false);
            }
            Change::AiBot { index, bot } => self.set_ai_bot(index, bot),
            Change::AiDifficulty { index, difficulty } => self.set_ai_difficulty(index, difficulty),
            Change::AiBehavior { index, behavior } => self.set_ai_behavior(index, behavior),
            Change::Unassign { index } => {
                self.set_ai_bot(index, None);
                self.set_removed(index, false);
            }
            Change::Remove { index } => {
                self.set_removed(index, true);
                self.set_ai_bot(index, None);
            }
            Change::ResetCivs => {
                for index in 0..count {
                    self.set_civ(catalog, index, Choice::Random);
                }
            }
            Change::ResetTeams => {
                for index in 0..count {
                    self.set_team(index, -1);
                }
            }
        }
        self.sync_civ_locks(catalog);
        Ok(())
    }

    /// Call after every seat table change; idempotent. Ports
    /// `PlayerAssignment.onPlayerAssignmentsChange` and the conflict loop in
    /// `GameSettingsController.onLoad`.
    pub fn on_seated(&mut self, catalog: &Catalog, seated: &[u8]) {
        self.clear_seated_conflicts(seated);
        self.check_fast_forward(catalog, !seated.is_empty());
        self.sync_civ_locks(catalog);
    }

    fn clear_seated_conflicts(&mut self, seated: &[u8]) {
        for player in seated {
            let Some(index) = usize::from(*player).checked_sub(1) else {
                continue;
            };
            if self.player_ai.get(index).is_some_and(Option::is_some) {
                self.set_ai_bot(index, None);
            } else if self.player_removed.get(index) == Some(&true) {
                self.set_removed(index, false);
            }
        }
    }

    /// `GameSpeed.render`: fast-forward speeds only while no human is seated.
    fn check_fast_forward(&mut self, catalog: &Catalog, any_seated: bool) {
        let allow = !any_seated;
        if self.gui.allow_fast_forward == Some(allow) {
            return;
        }
        self.gui.allow_fast_forward = Some(allow);
        let speed = self.game_speed;
        let offered = catalog
            .game_speeds
            .iter()
            .any(|entry| entry.speed == speed && (!entry.fast_forward || allow));
        if !offered {
            self.game_speed = catalog
                .game_speeds
                .iter()
                .find(|entry| entry.default)
                .map_or(FALLBACK_GAME_SPEED, |entry| entry.speed);
        }
    }

    /// `PlayerAssignmentItem.Client.onSelectionChange` for a seated player
    /// moving to `target`; the seat itself moves in the caller.
    pub fn swap_slots(&mut self, catalog: &Catalog, source: usize, target: usize) {
        self.set_removed(target, false);
        if self.player_ai.get(target).is_some_and(Option::is_some) {
            self.swap_ai(source, target);
        }
        if !self.is_scenario() {
            self.swap_civ(source, target);
            self.swap_color(source, target);
        }
        self.sync_civ_locks(catalog);
    }

    /// `PlayerAssignment.OnPlayerNbChange`, without its `isPlayerSlot` test:
    /// the seats are not at hand here, so `open` clears an AI it put on a
    /// seated slot afterwards, and in `apply` no seat is beyond the old
    /// count.
    pub(super) fn fill_new_slots(&mut self, catalog: &Catalog, old: usize) {
        for index in old..self.player_count.players {
            let empty = self.player_ai.get(index).is_some_and(Option::is_none);
            let removed = self.player_removed.get(index) == Some(&true);
            if !empty || removed {
                continue;
            }
            let Some(default) = catalog.player_defaults.get(index + 1) else {
                continue;
            };
            if default.ai.is_empty() {
                continue;
            }
            self.set_player_ai(
                index,
                Some(Ai {
                    bot: default.ai.clone(),
                    difficulty: DEFAULT_AI_DIFFICULTY,
                    behavior: DEFAULT_AI_BEHAVIOR.to_string(),
                }),
            );
        }
    }

    /// `PlayerCiv.rebuild`: a civ control whose slot unlocks falls back to
    /// random.
    fn sync_civ_locks(&mut self, catalog: &Catalog) {
        for index in 0..MAX_PLAYERS {
            let locked = self.player_civ.locked.get(index) == Some(&true);
            if locked == self.gui.civ_was_locked[index] {
                continue;
            }
            self.gui.civ_was_locked[index] = locked;
            if !locked && self.player_civ.values.get(index).is_some() {
                self.set_civ(catalog, index, Choice::Random);
            }
        }
    }

    /// The settings a networked host's setup page holds after loading, in
    /// vanilla's order: the `SetupWindow` load handlers, then
    /// `GameSettingsController.onLoad`, then the controls' `onLoad`.
    pub fn open(
        catalog: &Catalog,
        batch: Option<&Value>,
        seated: &[u8],
    ) -> Result<Settings, String> {
        let mut settings = Settings::new(catalog)?;
        if let Some(batch) = batch {
            // On the clone, so a batch that fails leaves no filter behind.
            let mut trial = settings.clone();
            if let Some(filter) = batch["guiData"]["mapFilter"].as_str() {
                trial.gui.map_filter = filter.to_string();
            }
            match trial.from_init_attributes(catalog, batch) {
                Ok(()) => settings = trial,
                Err(error) => eprintln!("setup batch ignored: {error}"),
            }
        }
        settings.clear_seated_conflicts(seated);
        if settings.map.kind.is_none() {
            settings.choose_type(catalog, DEFAULT_MAP_TYPE);
        } else {
            settings.check_filter(catalog);
            settings.update_map_list(catalog);
        }
        // The map's player count may have filled seated slots with AIs.
        settings.clear_seated_conflicts(seated);
        // `LockedTeams.js`: `DefaultValue = Engine.HasNetClient()`, true for
        // every match here; it overrides the batch, as in vanilla.
        settings.set_locked_teams(true);
        settings.gui.allow_fast_forward = None;
        settings.check_fast_forward(catalog, !seated.is_empty());
        settings.sync_civ_locks(catalog);
        Ok(settings)
    }
}
