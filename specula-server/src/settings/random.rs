//! `GameSettings.pickRandomItems` and each class's `pickRandomItems`.
//! Picking a random map fires the map watchers, which make the biome,
//! daytime, landscape and placement random again, so the classes run in
//! passes until none picks.

use rand::RngCore;
use rand::seq::SliceRandom;
use serde_json::Value;

use super::Choice;
use super::LandscapeChoice;
use super::RANDOM;
use super::Settings;
use super::gamesetup::data_ids_of;
use crate::catalog::Catalog;

/// `GameSettings.pickRandomItems` gives up after one pass more than it has
/// settings; three passes always suffice, so any bound above that only
/// catches a bug.
const PASS_LIMIT: usize = 39;

/// Vanilla's `pickRandom` is `undefined` on an empty list, which would start
/// the engine with the value missing, so the launch is refused instead.
fn pick<'a, T, R: RngCore + ?Sized>(
    items: &'a [T],
    rng: &mut R,
    class: &str,
) -> Result<&'a T, String> {
    items
        .choose(rng)
        .ok_or_else(|| format!("{class}: nothing to pick from"))
}

fn item_ids(group: &Value) -> impl Iterator<Item = String> + '_ {
    group["Items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["Id"].as_str().map(str::to_string))
}

impl Settings {
    /// `GameSettings.pickRandomItems`.
    pub fn pick_random_items<R: RngCore + ?Sized>(
        &mut self,
        catalog: &Catalog,
        rng: &mut R,
    ) -> Result<(), String> {
        for _ in 0..=PASS_LIMIT {
            // Every picker runs in every pass, as vanilla's
            // `rePick = pick() || rePick` does, so no `||` here.
            let mut picked = false;
            picked |= self.pick_biome(catalog, rng)?;
            picked |= self.pick_daytime(catalog, rng)?;
            picked |= self.pick_landscape(catalog, rng)?;
            picked |= self.pick_map(catalog, rng)?;
            picked |= self.pick_civs(catalog, rng)?;
            picked |= self.pick_ai_names(catalog, rng)?;
            picked |= self.pick_placement(rng)?;
            picked |= self.pick_seeds(rng)?;
            if !picked {
                return Ok(());
            }
        }
        Err("random choices did not settle".into())
    }

    fn map_is_random(&self) -> bool {
        self.map.map == Some(Choice::Random)
    }

    fn pick_biome<R: RngCore + ?Sized>(
        &mut self,
        catalog: &Catalog,
        rng: &mut R,
    ) -> Result<bool, String> {
        if self.map_is_random() || self.biome.biome != Some(Choice::Random) {
            return Ok(false);
        }
        let id = pick(&self.biome.available, rng, "Biome")?.clone();
        self.assign_biome(catalog, Some(Choice::Fixed(id)));
        Ok(true)
    }

    fn pick_daytime<R: RngCore + ?Sized>(
        &mut self,
        catalog: &Catalog,
        rng: &mut R,
    ) -> Result<bool, String> {
        if self.map_is_random() || self.daytime.value != Some(Choice::Random) {
            return Ok(false);
        }
        let ids = data_ids_of(&self.daytime.data);
        let id = pick(&ids, rng, "Daytime")?.clone();
        self.assign_daytime(catalog, Some(Choice::Fixed(id)));
        Ok(true)
    }

    fn pick_landscape<R: RngCore + ?Sized>(
        &mut self,
        catalog: &Catalog,
        rng: &mut R,
    ) -> Result<bool, String> {
        if self.map_is_random() {
            return Ok(false);
        }
        let Some(value) = &self.landscape.value else {
            return Ok(false);
        };
        let s = match value {
            LandscapeChoice::Random => RANDOM,
            LandscapeChoice::RandomGroup(id) | LandscapeChoice::Fixed(id) => id.as_str(),
        };
        if !s.starts_with(RANDOM) {
            return Ok(false);
        }
        let groups = self.landscape.data.as_ref().and_then(Value::as_array);
        let candidates: Vec<String> = if s.contains('_') {
            let group = groups
                .and_then(|groups| groups.iter().find(|group| group["Id"].as_str() == Some(s)))
                .filter(|group| group["Items"].is_array())
                .ok_or_else(|| format!("Landscape: no group {s}"))?;
            item_ids(group).collect()
        } else {
            groups.into_iter().flatten().flat_map(item_ids).collect()
        };
        let id = pick(&candidates, rng, "Landscape")?.clone();
        self.assign_landscape(catalog, Some(LandscapeChoice::Fixed(id)));
        Ok(true)
    }

    fn pick_map<R: RngCore + ?Sized>(
        &mut self,
        catalog: &Catalog,
        rng: &mut R,
    ) -> Result<bool, String> {
        if !self.map_is_random() {
            return Ok(false);
        }
        let path = pick(&self.map.random_options, rng, "Map")?.clone();
        // The cascade sets biome, daytime, landscape and placement back to
        // random, which the next pass resolves.
        self.select_map(catalog, Choice::Fixed(path));
        Ok(true)
    }

    fn pick_civs<R: RngCore + ?Sized>(
        &mut self,
        catalog: &Catalog,
        rng: &mut R,
    ) -> Result<bool, String> {
        let civs: Vec<&str> = catalog
            .civs
            .iter()
            .filter(|civ| civ.selectable)
            .map(|civ| civ.code.as_str())
            .collect();
        let mut picked = false;
        for value in &mut self.player_civ.values {
            if *value == Choice::Random {
                let code = pick(&civs, rng, "PlayerCiv")?;
                *value = Choice::Fixed(code.to_string());
                picked = true;
            }
        }
        Ok(picked)
    }

    fn pick_ai_names<R: RngCore + ?Sized>(
        &mut self,
        catalog: &Catalog,
        rng: &mut R,
    ) -> Result<bool, String> {
        // Vanilla's `AIPlayerNamesList`, local to one call.
        let mut chosen: Vec<String> = Vec::new();
        let mut picked = false;
        for i in 0..self.player_name.len() {
            let default = catalog.player_defaults.get(i + 1).map(|d| d.name.as_str());
            let current = self.player_name[i].as_deref();
            if matches!(current, Some(name) if !name.is_empty() && Some(name) != default) {
                continue;
            }
            if !matches!(self.player_ai.get(i), Some(Some(_))) {
                continue;
            }
            let Some(Choice::Fixed(code)) = self.player_civ.values.get(i) else {
                continue;
            };
            picked = true;
            let civ = catalog
                .civs
                .iter()
                .find(|civ| &civ.code == code)
                .ok_or_else(|| format!("PlayerName: unknown civ {code}"))?;
            if civ.ai_names.is_empty() {
                return Err(format!("PlayerName: civ {code} has no AINames"));
            }
            let remaining: Vec<String> = civ
                .ai_names
                .iter()
                .filter(|name| !chosen.contains(name))
                .cloned()
                .collect();
            let pool = if remaining.is_empty() {
                &civ.ai_names
            } else {
                &remaining
            };
            let name = pick(pool, rng, "PlayerName")?.clone();
            let dup = chosen.iter().filter(|other| **other == name).count();
            chosen.push(name.clone());
            // `CountLabel` untranslated: vanilla skips translation when
            // networked, and every match here is.
            self.player_name[i] = Some(if dup == 0 {
                name
            } else {
                format!("{name} ({})", dup + 1)
            });
        }
        Ok(picked)
    }

    fn pick_placement<R: RngCore + ?Sized>(&mut self, rng: &mut R) -> Result<bool, String> {
        if self.map_is_random() || self.player_placement.value != Some(Choice::Random) {
            return Ok(false);
        }
        let Some(available) = &self.player_placement.available else {
            return Ok(false);
        };
        let id = pick(available, rng, "PlayerPlacement")?.clone();
        self.player_placement.value = Some(Choice::Fixed(id));
        Ok(true)
    }

    fn pick_seeds<R: RngCore + ?Sized>(&mut self, rng: &mut R) -> Result<bool, String> {
        let mut picked = false;
        if self.seeds.seed == Choice::Random {
            self.seeds.seed = Choice::Fixed(rng.next_u32());
            picked = true;
        }
        if self.seeds.ai_seed == Choice::Random {
            self.seeds.ai_seed = Choice::Fixed(rng.next_u32());
            picked = true;
        }
        Ok(picked)
    }
}
