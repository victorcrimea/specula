//! The scores a match's time limit settles (D20, M30): the vanilla
//! `calculateScoreTotal` of every player at the last sample of a summary
//! capture, grouped by the mutual alliances in force then, so that the
//! leading group can be left standing and everyone else resigned.
//!
//! The arithmetic follows `gui/summary/counters.js` and the web client's
//! `score_part` (`specula-web/src/summary.rs`), which the summary screen
//! shows. This is pure state: no locks and no I/O.

use serde_json::Value;

/// One player's vanilla `calculateScoreTotal` at the last sample of a
/// summary capture, and whom they are a mutual ally of then.
pub struct PlayerScore {
    pub player: u8,
    pub total: f64,
    pub allies: Vec<u8>,
}

pub enum Outcome {
    /// The leading group: everyone else is resigned.
    Winners(Vec<u8>),
    /// An exact tie: everyone is resigned.
    Draw,
}

/// Parses view 0 of a kind 25 capture (`GetSpeculaSummaries()[0]`).
pub fn scores(full: &[u8]) -> Option<Vec<PlayerScore>> {
    let value: Value = serde_json::from_slice(full).ok()?;
    let states = value.get("playerStates")?.as_array()?;
    let mut scores = Vec::new();
    for (index, state) in states.iter().enumerate().skip(1) {
        // Gaia has the index but no score row.
        let Ok(player) = u8::try_from(index) else {
            continue;
        };
        let sequences = match state.get("sequences") {
            Some(sequences) => sequences,
            None => continue,
        };
        let last = match sequences.get("time").and_then(Value::as_array) {
            Some(time) if !time.is_empty() => time.len() - 1,
            _ => continue,
        };
        let at = |name: &str| -> f64 {
            sequences
                .get(name)
                .and_then(Value::as_array)
                .and_then(|series| series.get(last))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
        };
        let gathered: f64 = sequences
            .get("resourcesGathered")
            .and_then(Value::as_object)
            .map(|resources| {
                resources
                    .iter()
                    .filter(|(code, _)| code.as_str() != "vegetarianFood")
                    .map(|(_, series)| {
                        series
                            .as_array()
                            .and_then(|series| series.get(last))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                    })
                    .sum()
            })
            .unwrap_or(0.0);
        let economy = ((gathered + at("tradeIncome")) / 10.0).round();
        let military = ((at("enemyUnitsKilledValue")
            + at("unitsCapturedValue")
            + at("enemyBuildingsDestroyedValue")
            + at("buildingsCapturedValue"))
            / 10.0)
            .round();
        let exploration = at("percentMapExplored") * 10.0;
        let allies = state
            .get("isMutualAlly")
            .and_then(Value::as_array)
            .map(|allies| {
                allies
                    .iter()
                    .enumerate()
                    .filter(|(index, ally)| *index >= 1 && ally.as_bool() == Some(true))
                    .filter_map(|(index, _)| u8::try_from(index).ok())
                    .filter(|ally| *ally != player)
                    .collect()
            })
            .unwrap_or_default();
        scores.push(PlayerScore {
            player,
            total: economy + military + exploration,
            allies,
        });
    }
    Some(scores)
}

/// Groups the `active` players by mutual alliance, each with the sum of its
/// members' totals, in ascending player order.
pub fn groups(scores: &[PlayerScore], active: &[u8]) -> Vec<(Vec<u8>, f64)> {
    let total = |player: u8| -> f64 {
        scores
            .iter()
            .find(|score| score.player == player)
            .map_or(0.0, |score| score.total)
    };
    let allies_of = |player: u8| -> Vec<u8> {
        scores
            .iter()
            .find(|score| score.player == player)
            .map(|score| score.allies.clone())
            .unwrap_or_default()
    };
    let mut ordered: Vec<u8> = active.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    let mut placed = vec![false; ordered.len()];
    let mut groups = Vec::new();
    for (start, &player) in ordered.iter().enumerate() {
        if placed[start] {
            continue;
        }
        let mut group = Vec::new();
        let mut queue = vec![player];
        placed[start] = true;
        while let Some(member) = queue.pop() {
            group.push(member);
            for ally in allies_of(member) {
                let Some(index) = ordered.iter().position(|other| *other == ally) else {
                    continue;
                };
                // The edge needs both directions, as IsMutualAlly does.
                if placed[index] || !allies_of(ally).contains(&member) {
                    continue;
                }
                placed[index] = true;
                queue.push(ally);
            }
        }
        group.sort_unstable();
        let sum = group.iter().map(|player| total(*player)).sum();
        groups.push((group, sum));
    }
    groups
}

/// Groups the `active` players by mutual alliance and picks the group with
/// the highest summed total.
pub fn decide(scores: &[PlayerScore], active: &[u8]) -> Outcome {
    let groups = groups(scores, active);
    let best = groups
        .iter()
        .map(|(_, score)| *score)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut leaders = groups
        .iter()
        .filter(|(_, score)| *score == best)
        .map(|(members, _)| members.clone());
    let Some(winners) = leaders.next() else {
        return Outcome::Draw;
    };
    if leaders.next().is_some() {
        return Outcome::Draw;
    }
    Outcome::Winners(winners)
}
