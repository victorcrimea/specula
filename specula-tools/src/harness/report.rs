//! A test's result as JSON, and the text tables.

use crate::engine::MatchResult;
use crate::engine::PlayerResult;
use crate::setup::Setup;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

/// Input players: A is `PlayerData[0]` of the input setup, B is `[1]`.
/// `outcomes[0]` is the match as given, `[1]` the swapped one.
pub fn build(setup: &Setup, outcomes: &[MatchResult; 2], mirror: bool) -> Value {
    let mut matches = Vec::new();
    let mut totals = [[0.0; 5]; 2];
    for (index, outcome) in outcomes.iter().enumerate() {
        // Player ids start at 1; the swap puts B on player 1.
        let ids = if index == 0 { [1, 2] } else { [2, 1] };
        let mut entry = json!({
            "player_1": if index == 0 { "A" } else { "B" },
            "time": outcome.time,
            "timed_out": outcome.timed_out,
            "wall_seconds": outcome.wall.as_secs(),
            "peak_resident_mb": outcome.peak_resident >> 20,
            // Not played: the swap of identical players is the same game.
            "duplicate": mirror && index == 1,
        });
        for (who, name) in ["A", "B"].iter().enumerate() {
            let player = outcome.players.iter().find(|p| p.id == ids[who]);
            let scores = scores(player, outcome.timed_out);
            for (sum, score) in totals[who].iter_mut().zip(scores) {
                *sum += score;
            }
            entry[name] = json!({
                "state": player.map_or("", |p| p.state.as_str()),
                "match": scores[0],
                "economy": scores[1],
                "military": scores[2],
                "exploration": scores[3],
                "total": scores[4],
            });
        }
        matches.push(entry);
    }
    let total = |who: usize| {
        json!({
            "match": totals[who][0],
            "economy": totals[who][1],
            "military": totals[who][2],
            "exploration": totals[who][3],
            "total": totals[who][4],
        })
    };
    json!({
        "setup": setup.name,
        "map": setup.map().unwrap_or(""),
        "seed": setup.seed_of("Seed"),
        "ai_seed": setup.seed_of("AISeed"),
        "players": { "A": { "civ": setup.civ(0) }, "B": { "civ": setup.civ(1) } },
        "matches": matches,
        "totals": { "A": total(0), "B": total(1) },
    })
}

/// Match points, economy, military, exploration, total.
fn scores(player: Option<&PlayerResult>, timed_out: bool) -> [f64; 5] {
    let Some(player) = player else {
        return [0.0; 5];
    };
    let points = if timed_out {
        0.5
    } else {
        match player.state.as_str() {
            "won" => 1.0,
            "defeated" => 0.0,
            _ => 0.5,
        }
    };
    [
        points,
        player.economy,
        player.military,
        player.exploration,
        player.total,
    ]
}

/// The short text form of one test, for stderr.
pub fn table(result: &Value) -> String {
    let mut text = format!(
        "{}: seed {}, AI seed {}\n",
        result["setup"].as_str().unwrap_or(""),
        result["seed"],
        result["ai_seed"]
    );
    text.push_str("match  player 1  time    ended      who  points  economy  military\n");
    for (index, entry) in result["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let ended = if entry["timed_out"].as_bool() == Some(true) {
            "timed out"
        } else {
            "decided"
        };
        for who in ["A", "B"] {
            let civ = result["players"][who]["civ"].as_str().unwrap_or("");
            text.push_str(&format!(
                "{:<6} {:<9} {:<7.0} {:<10} {:<4} {:<7} {:<8} {}\n",
                index + 1,
                entry["player_1"].as_str().unwrap_or(""),
                entry["time"].as_f64().unwrap_or(0.0),
                ended,
                format!("{who} {civ}"),
                entry[who]["match"],
                entry[who]["economy"],
                entry[who]["military"],
            ));
        }
    }
    for who in ["A", "B"] {
        let total = &result["totals"][who];
        text.push_str(&format!(
            "total {who} {}: {} of 2, economy {}, military {}\n",
            result["players"][who]["civ"].as_str().unwrap_or(""),
            total["match"],
            total["economy"],
            total["military"]
        ));
    }
    text
}

#[derive(Default)]
struct CivStats {
    tests: u32,
    matches: u32,
    points: f64,
    economy: f64,
    military: f64,
    timeouts: u32,
}

/// One row per civ over the given tests; a mirror counts for its civ from
/// both input players.
pub fn civ_table(results: &[Value]) -> String {
    let mut civs: BTreeMap<String, CivStats> = BTreeMap::new();
    for result in results {
        let timeouts = result["matches"]
            .as_array()
            .map_or(0, |m| m.iter().filter(|e| e["timed_out"] == true).count())
            as u32;
        for who in ["A", "B"] {
            let Some(civ) = result["players"][who]["civ"].as_str() else {
                continue;
            };
            let stats = civs.entry(civ.to_string()).or_default();
            let total = &result["totals"][who];
            stats.tests += 1;
            stats.matches += 2;
            stats.points += total["match"].as_f64().unwrap_or(0.0);
            stats.economy += total["economy"].as_f64().unwrap_or(0.0);
            stats.military += total["military"].as_f64().unwrap_or(0.0);
            stats.timeouts += timeouts;
        }
    }
    let mut text =
        String::from("civ     tests  points  win rate  mean economy  mean military  timeouts\n");
    for (civ, stats) in &civs {
        let matches = f64::from(stats.matches);
        text.push_str(&format!(
            "{:<7} {:<6} {:<7} {:<9.3} {:<13.0} {:<14.0} {}\n",
            civ,
            stats.tests,
            stats.points,
            stats.points / matches,
            stats.economy / matches,
            stats.military / matches,
            stats.timeouts
        ));
    }
    text
}

/// One row per map: how often the player in the first slot won. A fair map
/// is near 0.5; with a mirror setup this is the only signal, as the swap
/// gives each side a point.
pub fn map_table(results: &[Value]) -> String {
    // decided matches, first-slot wins, timeouts, game seconds
    let mut maps: BTreeMap<String, (u32, u32, u32, f64)> = BTreeMap::new();
    for result in results {
        let map = result["map"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("maps/");
        let row = maps.entry(map.to_string()).or_default();
        for entry in result["matches"].as_array().into_iter().flatten() {
            if is_duplicate(result, entry) {
                continue;
            }
            row.3 += entry["time"].as_f64().unwrap_or(0.0);
            if entry["timed_out"] == true {
                row.2 += 1;
                continue;
            }
            row.0 += 1;
            let first = entry["player_1"].as_str().unwrap_or("A");
            if entry[first]["match"].as_f64() == Some(1.0) {
                row.1 += 1;
            }
        }
    }
    let mut rows: Vec<_> = maps.into_iter().collect();
    // Fairest first; a map with no decided match last.
    rows.sort_by(|a, b| {
        let skew = |r: &(u32, u32, u32, f64)| {
            if r.0 == 0 {
                f64::MAX
            } else {
                (f64::from(r.1) / f64::from(r.0) - 0.5).abs()
            }
        };
        skew(&a.1).total_cmp(&skew(&b.1))
    });
    let mut text = String::from(
        "map                              matches  decided  first-slot wins  rate   timeouts  mean minutes\n",
    );
    for (map, (decided, wins, timeouts, seconds)) in rows {
        let matches = decided + timeouts;
        text.push_str(&format!(
            "{:<32} {:<8} {:<8} {:<16} {:<6.2} {:<9} {:.0}\n",
            map,
            matches,
            decided,
            wins,
            if decided == 0 {
                f64::NAN
            } else {
                f64::from(wins) / f64::from(decided)
            },
            timeouts,
            seconds / 60.0 / f64::from(matches.max(1)),
        ));
    }
    text
}

/// The second match of a mirror, which is the first one again. Results
/// stored before the flag existed are told by their numbers.
fn is_duplicate(result: &Value, entry: &Value) -> bool {
    if let Some(flag) = entry["duplicate"].as_bool() {
        return flag;
    }
    let first = &result["matches"][0];
    entry["player_1"] == "B"
        && result["players"]["A"]["civ"] == result["players"]["B"]["civ"]
        && entry["time"] == first["time"]
        && entry["A"]["economy"] == first["B"]["economy"]
        && entry["B"]["economy"] == first["A"]["economy"]
}
