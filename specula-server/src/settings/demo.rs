//! The random settings of a `--demo` match: a watchable AI-only game, with
//! enough variety that the stream is not the same match again and again.

use rand::RngCore;

use super::Choice;
use super::ObserverMode;
use super::Settings;
use super::gamesetup::Change;
use super::gamesetup::MAX_TEAMS;
use super::gamesetup::map_list;
use crate::catalog::AI_BEHAVIORS;
use crate::catalog::Catalog;
use crate::catalog::MapType;

/// Crowded matches are the show, so the count is capped by what the engine
/// and the observer view handle, not by taste.
const MAX_DEMO_PLAYERS: usize = 14;
const MIN_DEMO_PLAYERS: usize = 4;
/// A map for fewer players than this is a poor stage for a big battle.
const WIDE_MAP_PLAYERS: usize = 6;
const MAP_TRIES: usize = 20;
const BOT: &str = "petra";
/// `AI_DIFFICULTIES`: medium, hard, very hard. Weaker AIs build too slowly
/// to be worth watching and sandbox does not fight at all.
const DIFFICULTIES: [u8; 3] = [3, 4, 5];
/// The one victory condition of every demo; the others are switched off.
const VICTORY: &str = "conquest_civic_centers";
/// Starting resources, by the catalog's amount, with a weight. Deathmatch
/// (50000) is left out: it ends before anything can be seen.
const RESOURCES: [(u32, u32); 4] = [(300, 40), (500, 30), (1000, 20), (3000, 10)];
const POPULATION: [(&str, u32); 3] = [("player", 70), ("team", 20), ("world", 10)];
/// How many teams, 0 for none, with a weight.
const TEAM_COUNTS: [(i32, u32); 3] = [(0, 25), (2, 50), (3, 25)];

fn below<R: RngCore + ?Sized>(rng: &mut R, bound: usize) -> usize {
    (rng.next_u64() % bound.max(1) as u64) as usize
}

fn weighted<T: Copy, R: RngCore + ?Sized>(rng: &mut R, items: &[(T, u32)]) -> T {
    let total: u32 = items.iter().map(|(_, weight)| weight).sum();
    let mut roll = below(rng, total as usize) as u32;
    for (item, weight) in items {
        if roll < *weight {
            return *item;
        }
        roll -= weight;
    }
    items[0].0
}

impl Settings {
    /// The settings of one demo match, for a map picked at random.
    pub fn demo<R: RngCore + ?Sized>(
        catalog: &Catalog,
        default_delay_secs: u32,
        rng: &mut R,
    ) -> Result<Settings, String> {
        let mut settings = Settings::open(catalog, None, &[], default_delay_secs)?;
        let mut maps = map_list(catalog, MapType::Skirmish, "multiplayer");
        maps.extend(map_list(catalog, MapType::Random, "multiplayer"));
        if maps.is_empty() {
            return Err("no multiplayer map to play".to_string());
        }
        // The player cap is only known once the map is chosen, so try
        // maps until one is wide enough, and keep the last if none is.
        for attempt in 0..MAP_TRIES {
            let map = maps[below(rng, maps.len())];
            settings.apply(
                catalog,
                Change::BrowseMap {
                    kind: map.kind,
                    filter: "multiplayer".to_string(),
                    map: Choice::Fixed(map.path.clone()),
                },
                &[],
            )?;
            if settings.player_count.max >= WIDE_MAP_PLAYERS || attempt + 1 == MAP_TRIES {
                break;
            }
        }
        let max = settings.player_count.max.min(MAX_DEMO_PLAYERS);
        let low = MIN_DEMO_PLAYERS.min(max);
        // The larger of two draws leans toward big matches.
        let span = max - low + 1;
        let players = low + below(rng, span).max(below(rng, span));
        settings.apply(catalog, Change::PlayerCount(players), &[])?;
        let players = settings.player_count.players;

        if players >= 8 {
            let bigger = catalog
                .map_sizes
                .iter()
                .skip_while(|size| size.tiles != settings.map_size.default)
                .nth(1)
                .map(|size| size.tiles);
            if let Some(tiles) = bigger.filter(|_| below(rng, 2) == 0) {
                // A map with a fixed size refuses it; the default stands.
                let _ = settings.apply(catalog, Change::MapSize(tiles), &[]);
            }
        }

        let difficulty = DIFFICULTIES[below(rng, DIFFICULTIES.len())];
        for index in 0..players {
            settings.apply(
                catalog,
                Change::Ai {
                    index,
                    bot: BOT.to_string(),
                },
                &[],
            )?;
            settings.set_ai_difficulty(index, difficulty);
            let behavior = AI_BEHAVIORS[below(rng, AI_BEHAVIORS.len())];
            settings.set_ai_behavior(index, behavior.to_string());
            settings.apply(
                catalog,
                Change::Civ {
                    index,
                    civ: Choice::Random,
                },
                &[],
            )?;
        }

        let teams = weighted(rng, &TEAM_COUNTS);
        // Shuffled, so a team is not always the first slots.
        let mut order: Vec<usize> = (0..players).collect();
        for at in (1..order.len()).rev() {
            order.swap(at, below(rng, at + 1));
        }
        for (rank, index) in order.into_iter().enumerate() {
            let locked = settings.player_team.locked.get(index) == Some(&true);
            if teams > 0 && !locked && players >= 2 * teams as usize {
                let team = (rank as i32 % teams).min(MAX_TEAMS - 1);
                settings.apply(catalog, Change::Team { index, team }, &[])?;
            } else if !locked {
                settings.apply(catalog, Change::Team { index, team: -1 }, &[])?;
            }
        }

        // Scenario-like maps refuse these; the engine defaults then stand.
        let resources = weighted(rng, &RESOURCES);
        let _ = settings.apply(catalog, Change::StartingResources(resources), &[]);
        let population = weighted(rng, &POPULATION);
        let _ = settings.apply(
            catalog,
            Change::PopulationCapType(population.to_string()),
            &[],
        );

        // Off first: the default conquest keeps Civic Centers disabled
        // until it is switched off.
        for condition in &catalog.victory_conditions {
            if condition.name != VICTORY {
                let _ = settings.apply(
                    catalog,
                    Change::VictoryCondition {
                        name: condition.name.clone(),
                        enabled: false,
                    },
                    &[],
                );
            }
        }
        settings.apply(
            catalog,
            Change::VictoryCondition {
                name: VICTORY.to_string(),
                enabled: true,
            },
            &[],
        )?;

        settings.observers.mode = ObserverMode::Everything;
        settings.observers.delay_secs = 0;
        Ok(settings)
    }
}
