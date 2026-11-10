//! Displayed player colours, ported from 0 A.D.'s
//! gui/session/DiplomacyColors.js (computeTeamColors).

use std::collections::HashMap;

use crate::stream::Dot;
use crate::stream::HIDDEN;
use crate::stream::Player;
use crate::stream::PlayerState;
use crate::stream::Stance;
use crate::stream::VISIBLE;
use crate::stream::visibility_of;

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

impl Perspective {
    /// Whether an entity with `Dot::visibility` is shown to it, visible or
    /// in the fog: what the engine draws and picks (`CCmpUnitRenderer`,
    /// `Selection.cpp` skip only HIDDEN). An observer sees all, as a 0 A.D.
    /// observer does.
    pub fn sees(self, visibility: u32) -> bool {
        match self {
            Perspective::Observer => true,
            Perspective::Player(player) => visibility_of(visibility, player) != HIDDEN,
        }
    }

    /// Whether it hears the entity's sounds: in sight, not in the fog
    /// (`CCmpSoundManager::PlaySoundGroup`).
    pub fn hears(self, visibility: u32) -> bool {
        match self {
            Perspective::Observer => true,
            Perspective::Player(player) => visibility_of(visibility, player) == VISIBLE,
        }
    }

    /// Whether the view draws the entity. A mirage is its player's alone.
    /// `filtered`: the stream has this player's LOS, so its visibility
    /// tells the fog from the hidden; older streams only had "visible",
    /// and drew everything.
    pub fn shows(self, dot: &Dot, filtered: bool) -> bool {
        if dot.mirage != 0 {
            return self == Perspective::Player(dot.mirage as usize - 1);
        }
        !filtered || self.sees(dot.visibility)
    }

    /// The player whose entities it views, `None` for an observer.
    pub fn player(self) -> Option<i32> {
        match self {
            Perspective::Observer => None,
            Perspective::Player(player) => i32::try_from(player).ok(),
        }
    }
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
