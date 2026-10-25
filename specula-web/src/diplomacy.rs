//! Displayed player colours, ported from 0 A.D.'s
//! gui/session/DiplomacyColors.js (computeTeamColors).

use std::collections::HashMap;

use crate::stream::Player;
use crate::stream::PlayerState;
use crate::stream::Stance;

// The defaults of [gui.session.diplomacycolors] in default.cfg; a viewer has
// no user config to override them.
const SELF: u32 = 0xFF15_3795;
pub(crate) const ALLY: u32 = 0xFF56_B41F;
pub(crate) const NEUTRAL: u32 = 0xFFE7_C805;
pub(crate) const ENEMY: u32 = 0xFF96_1414;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Perspective {
    Observer,
    /// A player id, never gaia.
    Player(usize),
}

/// The colour to draw each player id in, 0xFFRRGGBB.
pub fn palette(players: &[Player], enabled: bool, perspective: Perspective) -> Vec<u32> {
    let mut colors: Vec<u32> = players.iter().map(|player| player.color).collect();
    if !enabled {
        return colors;
    }
    match perspective {
        Perspective::Player(viewed) if viewed > 0 && viewed < players.len() => {
            let stances = &players[viewed].stances;
            for (id, color) in colors.iter_mut().enumerate().skip(1) {
                *color = if id == viewed {
                    SELF
                } else {
                    match stances.get(id) {
                        Some(Stance::Ally) => ALLY,
                        Some(Stance::Neutral) => NEUTRAL,
                        _ => ENEMY,
                    }
                };
            }
        }
        _ => {
            // A team shows as its first active player; a player out of the
            // game no longer belongs to a team.
            let mut representatives: HashMap<i32, usize> = HashMap::new();
            for (id, player) in players.iter().enumerate().skip(1) {
                if player.state != PlayerState::Active || player.team == -1 {
                    continue;
                }
                let representative = *representatives.entry(player.team).or_insert(id);
                colors[id] = players[representative].color;
            }
        }
    }
    colors
}
