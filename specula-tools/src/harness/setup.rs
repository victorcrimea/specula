//! A game setup, the init attributes a replay's `start` line holds, and the
//! checks and rewrites a test needs.

use serde_json::Value;
use std::hash::BuildHasher;
use std::hash::Hasher;
use std::path::Path;

/// What the engine's trigger script that reports the result is called.
const HARNESS_SCRIPT: &str = "scripts/SpeculaHarness.js";

pub struct Setup {
    pub name: String,
    pub value: Value,
}

impl Setup {
    pub fn load(path: &Path) -> Result<Setup, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        let name = path
            .file_stem()
            .map_or_else(|| "setup".to_string(), |s| s.to_string_lossy().into_owned());
        let setup = Setup { name, value };
        setup
            .validate()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(setup)
    }

    fn validate(&self) -> Result<(), String> {
        let settings = self.value.get("settings").ok_or("no \"settings\" object")?;
        let players = settings
            .get("PlayerData")
            .and_then(Value::as_array)
            .ok_or("no PlayerData")?;
        if players.len() != 2 {
            return Err(format!("{} players, not 2", players.len()));
        }
        for (index, player) in players.iter().enumerate() {
            if player
                .get("AI")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(format!("player {} has no AI", index + 1));
            }
            // Resolved by the GUI's unseeded RNG, so the two matches would
            // not agree on it.
            if player.get("Civ").and_then(Value::as_str) == Some("random") {
                return Err(format!("player {}'s civ is random", index + 1));
            }
            if player.get("Civ").and_then(Value::as_str).is_none() {
                return Err(format!("player {} has no civ", index + 1));
            }
        }
        let team = |player: &Value| player.get("Team").and_then(Value::as_i64).unwrap_or(-1);
        if team(&players[0]) >= 0 && team(&players[0]) == team(&players[1]) {
            return Err("both players are on the same team".to_string());
        }
        let map = self.map()?;
        if map == "random" || map.ends_with("/random") {
            return Err("the map is random".to_string());
        }
        for key in ["Biome", "PlayerPlacement"] {
            if settings.get(key).and_then(Value::as_str) == Some("random") {
                return Err(format!("{key} is random"));
            }
        }
        Ok(())
    }

    /// The map's VFS path, as the setup's `map`.
    pub fn map(&self) -> Result<&str, String> {
        self.value
            .get("map")
            .and_then(Value::as_str)
            .ok_or_else(|| "no \"map\"".to_string())
    }

    /// What `-autostart=` takes: the map without its "maps/".
    pub fn autostart_map(&self) -> Result<String, String> {
        let map = self.map()?;
        Ok(map.strip_prefix("maps/").unwrap_or(map).to_string())
    }

    fn settings_mut(&mut self) -> &mut serde_json::Map<String, Value> {
        self.value["settings"].as_object_mut().expect("validated")
    }

    /// Replaces a "random" or missing seed with a number, and applies an
    /// `AISeed` override. Returns the two seeds.
    pub fn resolve_seeds(&mut self, ai_seed: Option<u32>) -> (u32, u32) {
        let settings = self.settings_mut();
        if let Some(seed) = ai_seed {
            settings.insert("AISeed".to_string(), Value::from(seed));
        }
        let mut resolve = |key: &str| {
            let seed = match settings.get(key) {
                Some(Value::Number(number)) => number.as_u64().unwrap_or(0) as u32,
                Some(Value::String(text)) if text == "random" => random_seed(),
                _ => 0,
            };
            settings.insert(key.to_string(), Value::from(seed));
            seed
        };
        let seed = resolve("Seed");
        let ai_seed = resolve("AISeed");
        (seed, ai_seed)
    }

    /// The same setup with the two players swapped whole, so the start
    /// positions, which follow the player number, swap.
    pub fn swapped(&self) -> Setup {
        let mut other = Setup {
            name: self.name.clone(),
            value: self.value.clone(),
        };
        if let Some(players) = other.settings_mut()["PlayerData"].as_array_mut() {
            players.swap(0, 1);
        }
        other
    }

    /// The trigger script that prints the result; the victory scripts are
    /// added by the engine at launch.
    pub fn with_harness_script(mut self) -> Setup {
        let settings = self.settings_mut();
        let scripts = settings
            .entry("TriggerScripts")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(scripts) = scripts.as_array_mut()
            && !scripts.iter().any(|s| s == HARNESS_SCRIPT)
        {
            scripts.push(Value::from(HARNESS_SCRIPT));
        }
        self
    }

    /// True when the two players differ only in colour, so the swapped
    /// match would be the same game and need not be run.
    pub fn is_mirror(&self) -> bool {
        let strip = |index: usize| {
            let mut player = self.value["settings"]["PlayerData"][index].clone();
            if let Some(player) = player.as_object_mut() {
                player.remove("Color");
            }
            player
        };
        strip(0) == strip(1)
    }

    /// The two seeds when neither is "random", so a rerun can be matched
    /// with a stored result before it starts.
    pub fn fixed_seeds(&self, ai_seed: Option<u32>) -> Option<(u32, u32)> {
        let settings = &self.value["settings"];
        let random = |key: &str| settings[key].as_str() == Some("random");
        if random("Seed") || (ai_seed.is_none() && random("AISeed")) {
            return None;
        }
        let mut copy = Setup {
            name: self.name.clone(),
            value: self.value.clone(),
        };
        Some(copy.resolve_seeds(ai_seed))
    }

    pub fn seed_of(&self, key: &str) -> u32 {
        self.value["settings"][key].as_u64().unwrap_or(0) as u32
    }

    pub fn civ(&self, player: usize) -> &str {
        self.value["settings"]["PlayerData"][player]["Civ"]
            .as_str()
            .unwrap_or("")
    }
}

/// Fresh per call: `RandomState` is keyed per thread and counted per
/// instance, so no clock is needed.
fn random_seed() -> u32 {
    std::hash::RandomState::new().build_hasher().finish() as u32
}
