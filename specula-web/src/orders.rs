//! What a right click orders the player's selection to do, and the cursor
//! that shows it beforehand: a part of `gui/session/unit_actions.js`, the
//! actions a right click can pick (`determineAction`, with the hotkeys of
//! the actions ported here), in the engine's order of `specificness`, and
//! the left click of a command button's preselected action. The engine
//! checks every order again; what this gets wrong only sends an order the
//! units cannot carry out.

use specula_assets::command;
use specula_assets::command::ALLOW_CAPTURE;
use specula_assets::command::Command;
use specula_assets::command::QUEUED;

use crate::detail;
use crate::detail::Commands;
use crate::detail::DataKey;
use crate::detail::Group;
use crate::lang;
use crate::left::matches_class_list;
use crate::panel::entity_groups;
use crate::panel::mutual_allies;
use crate::panel::string;
use crate::panel::template_details;
use crate::stream::COMPONENT_ATTACK;
use crate::stream::COMPONENT_BUILDER;
use crate::stream::COMPONENT_CAPTURABLE;
use crate::stream::COMPONENT_FOUNDATION;
use crate::stream::COMPONENT_GARRISON_HOLDER;
use crate::stream::COMPONENT_GARRISONABLE;
use crate::stream::COMPONENT_GUARD;
use crate::stream::COMPONENT_MARKET;
use crate::stream::COMPONENT_RALLY_POINT;
use crate::stream::COMPONENT_REPAIRABLE;
use crate::stream::COMPONENT_RESOURCE_DROPSITE;
use crate::stream::COMPONENT_RESOURCE_GATHERER;
use crate::stream::COMPONENT_RESOURCE_SUPPLY;
use crate::stream::COMPONENT_TURRET_HOLDER;
use crate::stream::COMPONENT_TURRETABLE;
use crate::stream::COMPONENT_UNIT_AI;
use crate::stream::Dot;
use crate::stream::Scene;
use crate::stream::Stance;
use crate::stream::TEMPLATE_UNCONTROLLABLE;
use crate::stream::TemplateDetails;
use crate::tips;
use crate::tips::DataFiles;
use crate::tooltip;
use crate::trade;
use crate::trade::Details;

/// Where a right click landed, or where the mouse is: the entity under the
/// cursor, and the ground point, x and z in metres.
pub struct Target {
    pub entity: Option<u32>,
    pub point: Option<[f32; 2]>,
    pub queued: bool,
    /// The `HOTKEY_*` modifiers held.
    pub hotkeys: u8,
    /// A left click, which only a preselected action takes.
    pub left: bool,
    /// The freehand line a right drag drew (`g_FreehandSelection_InputLine`),
    /// ground points x and z in metres; empty for a click.
    pub line: Vec<[f32; 2]>,
}

/// `g_FreehandSelection_MinLengthOfLine`, metres.
const FREEHAND_MIN_LENGTH: f32 = 8.0;
/// `g_FreehandSelection_MinNumberOfUnits`.
const FREEHAND_MIN_UNITS: usize = 2;
/// `DrawTargetMarker`'s spacing along a line, metres.
const FREEHAND_MARKER_SPACING: f32 = 5.0;

/// The engine's session.guard (G), session.patrol (P), session.repair (J),
/// session.capture (C) and session.attack (F); and session.garrison,
/// session.occupyturret, session.attackmove and session.autorallypoint,
/// which the engine's default config all bind to Ctrl.
pub const HOTKEY_GARRISON: u8 = 1;
pub const HOTKEY_GUARD: u8 = 2;
pub const HOTKEY_PATROL: u8 = 4;
pub const HOTKEY_REPAIR: u8 = 8;
pub const HOTKEY_CAPTURE: u8 = 16;
pub const HOTKEY_ATTACK: u8 = 32;

/// A command button's `preSelectedAction`: the action the next left
/// click on the view takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Preselect {
    CallToArms,
    Garrison,
    OccupyTurret,
    Repair,
    Guard,
    Patrol,
}

/// What a right click would do: the cursor the engine shows for it, the
/// name of a file in `art/textures/cursors/` or empty for the default
/// arrow, the order, none when the click does nothing, and the sound group
/// the order plays with the entity it plays from, the action's
/// `firstAbleEntity`.
pub struct Action {
    pub cursor: String,
    /// The `informationTooltip` at the mouse, in the engine's markup;
    /// empty for none.
    pub tooltip: String,
    pub command: Option<Command>,
    pub sound: Option<(&'static str, u32)>,
}

/// The action for `player`'s entities among `selected`, none when the
/// engine shows the default cursor and orders nothing. A walk puts them in
/// `formation`, empty for none; `data` has the template data a rally
/// point's trade route reads.
pub fn action(
    scene: &Scene,
    selected: &[u32],
    player: i32,
    target: &Target,
    formation: &str,
    data: &DataFiles,
) -> Option<Action> {
    let (own, able) = own_and_able(scene, selected, player)?;
    let details = |id: u32| {
        scene
            .dots
            .get(&id)
            .and_then(|dot| template_details(scene, dot.template))
    };
    let components = |id: u32| details(id).map_or(0, |details| details.components);
    if able.is_empty() {
        // The "uncontrollable" action.
        return Some(Action {
            cursor: "cursor-no".to_string(),
            tooltip: String::new(),
            command: None,
            sound: None,
        });
    }
    let flags = if target.queued { QUEUED } else { 0 };
    let entities = || own.clone();
    let has = |id: u32, bit: u32| components(id) & bit != 0;
    let target_dot = target
        .entity
        .and_then(|id| Some((id, scene.dots.get(&id)?)));
    let unit_ai = able.iter().any(|&id| has(id, COMPONENT_UNIT_AI));
    let actions = Actions {
        scene,
        own: &own,
        able: &able,
        player,
        target,
        target_dot,
        flags,
        formation,
    };

    // "set-rallypoint": structures only, as commands to units take
    // precedence, and vanilla checks it before every hotkey, so a
    // fortress with F held still sets its rally point.
    if !unit_ai
        && able.iter().any(|&id| has(id, COMPONENT_RALLY_POINT))
        && let Some(action) = rally(scene, &own, player, target, target_dot, flags, data)
    {
        return Some(action);
    }

    // `hotkeyActionCheck`, in the engine's order of `specificness`, before
    // the plain actions.
    let held = |hotkey: u8| target.hotkeys & hotkey != 0;
    let by_hotkey = None
        .or_else(|| held(HOTKEY_ATTACK).then(|| actions.attack()).flatten())
        .or_else(|| held(HOTKEY_CAPTURE).then(|| actions.capture()).flatten())
        .or_else(|| held(HOTKEY_REPAIR).then(|| actions.repair()).flatten())
        .or_else(|| held(HOTKEY_GARRISON).then(|| actions.garrison()).flatten())
        .or_else(|| {
            held(HOTKEY_GARRISON)
                .then(|| actions.occupy_turret())
                .flatten()
        })
        .or_else(|| {
            held(HOTKEY_GARRISON)
                .then(|| actions.attack_move())
                .flatten()
        })
        .or_else(|| held(HOTKEY_PATROL).then(|| actions.patrol()).flatten())
        .or_else(|| held(HOTKEY_GUARD).then(|| actions.guard()).flatten())
        .or_else(|| held(HOTKEY_GUARD).then(|| actions.remove_guard()).flatten());
    if by_hotkey.is_some() {
        return by_hotkey;
    }

    if let Some((id, dot)) = target_dot {
        let target_details = template_details(scene, dot.template);
        let bits = target_details.map_or(0, |details| details.components);
        let relation = relation(scene, player, dot.owner);

        // "setup-trade-route", for the first trader it is possible for.
        let route = able.iter().find_map(|&unit| {
            let details = trade::trading_details(scene, player, unit, id)?;
            let tooltip = match &details {
                Details::IsFirst { both: false, .. } => return None,
                Details::IsFirst { gain, .. } => format!(
                    "{}\n{}",
                    lang::translate("Origin trade market."),
                    trade::gain_line(scene, gain.as_ref())
                ),
                Details::IsSecond { gain } => format!(
                    "{}\n{}",
                    lang::translate("Destination trade market."),
                    trade::gain_line(scene, gain.as_ref())
                ),
                Details::SetFirst => lang::translate("Right-click to set as origin trade market"),
                Details::SetSecond { gain } if gain.trader_gain == 0.0 => {
                    return Some((unit, false, too_close()));
                }
                Details::SetSecond { gain } => format!(
                    "{}\n{}",
                    lang::translate("Right-click to set as destination trade market."),
                    trade::gain_line(scene, Some(gain))
                ),
            };
            Some((unit, true, tooltip))
        });
        if let Some((trader, possible, tooltip)) = route {
            if !possible {
                return Some(Action {
                    cursor: "action-setup-trade-route-disabled".to_string(),
                    tooltip,
                    command: None,
                    sound: None,
                });
            }
            return Some(Action {
                cursor: "action-setup-trade-route".to_string(),
                tooltip,
                command: Some(Command::SetupTradeRoute {
                    flags,
                    entities: entities(),
                    target: id,
                }),
                sound: Some(("order_trade", trader)),
            });
        }

        // "gather"
        if bits & COMPONENT_RESOURCE_SUPPLY != 0
            && let Some((gatherer, resource)) = able.iter().find_map(|&unit| {
                let resource = gathered(
                    scene,
                    details(unit)?,
                    target_details?,
                    has(unit, COMPONENT_RESOURCE_GATHERER),
                )?;
                Some((unit, resource))
            })
        {
            return Some(Action {
                cursor: if resource.is_empty() {
                    String::new()
                } else {
                    format!("action-gather-{resource}")
                },
                tooltip: String::new(),
                command: Some(Command::Gather {
                    flags,
                    entities: entities(),
                    target: id,
                }),
                sound: Some(("order_gather", gatherer)),
            });
        }
        // "returnresource"; shared dropsites are not considered.
        if relation == Relation::Own
            && bits & COMPONENT_RESOURCE_DROPSITE != 0
            && let Some((carrier, carried)) = able.iter().find_map(|&unit| {
                let carried = carried(scene, unit)?;
                let types = target_details?.dropsite_types;
                (types == 0
                    || string(scene, types)
                        .is_some_and(|types| types.split_whitespace().any(|kind| kind == carried)))
                .then_some((unit, carried))
            })
        {
            return Some(Action {
                cursor: format!("action-return-{carried}"),
                tooltip: String::new(),
                command: Some(Command::ReturnResource {
                    flags,
                    entities: entities(),
                    target: id,
                }),
                sound: Some(("order_gather", carrier)),
            });
        }
        // "cancel-setup-trade-route": the origin, before a destination.
        if able.iter().any(|&unit| {
            matches!(
                trade::trading_details(scene, player, unit, id),
                Some(Details::IsFirst { both: false, .. })
            )
        }) {
            return Some(Action {
                cursor: "action-cancel-setup-trade-route".to_string(),
                tooltip: lang::translate(
                    "This is the origin trade market.\nRight-click to cancel trade route.",
                ),
                command: Some(Command::CancelTradeRoute {
                    flags,
                    entities: entities(),
                    target: id,
                }),
                sound: None,
            });
        }
    }

    if target_dot.is_some() {
        if let Some(action) = actions.attack().or_else(|| actions.capture()) {
            return Some(action);
        }
        if let Some(action) = actions.repair() {
            return Some(action);
        }
    }

    // "move", which has no cursor of its own.
    let &walker = able.iter().find(|&&id| has(id, COMPONENT_UNIT_AI))?;
    let point = target
        .point
        .or_else(|| target_dot.map(|(_, dot)| position(dot)));
    Some(Action {
        cursor: String::new(),
        tooltip: String::new(),
        command: point.map(|[x, z]| Command::Walk {
            flags,
            entities: entities(),
            x,
            z,
            formation: formation.to_string(),
        }),
        sound: Some(("order_walk", walker)),
    })
}

/// The action a left click takes in `preselect`'s mode, as its
/// `preSelectedActionCheck`: a disabled cursor where it cannot, which a
/// click still ends the mode on. None without any of the player's own
/// selected, where the engine leaves the mode as it is.
pub fn preselected(
    scene: &Scene,
    selected: &[u32],
    player: i32,
    target: &Target,
    formation: &str,
    preselect: Preselect,
) -> Option<Action> {
    let (own, able) = own_and_able(scene, selected, player)?;
    let actions = Actions {
        scene,
        own: &own,
        able: &able,
        player,
        target,
        target_dot: target
            .entity
            .and_then(|id| Some((id, scene.dots.get(&id)?))),
        flags: if target.queued { QUEUED } else { 0 },
        formation,
    };
    let (action, disabled) = match preselect {
        Preselect::CallToArms => (actions.call_to_arms(), ""),
        Preselect::Garrison => (actions.garrison(), "action-garrison-disabled"),
        Preselect::OccupyTurret => (actions.occupy_turret(), "action-occupy-turret-disabled"),
        Preselect::Repair => (actions.repair(), "action-repair-disabled"),
        Preselect::Guard => (actions.guard(), "action-guard-disabled"),
        Preselect::Patrol => (actions.patrol(), ""),
    };
    Some(action.unwrap_or_else(|| Action {
        cursor: disabled.to_string(),
        tooltip: String::new(),
        command: None,
        sound: None,
    }))
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The port of `positionUnitsFreehandSelectionMouseUp`: the order and the
/// target markers. None where the engine falls back to `determineAction` at
/// the release point (a click, a line under 8 m, fewer than 2 units).
pub fn freehand(
    scene: &Scene,
    selected: &[u32],
    player: i32,
    target: &Target,
) -> Option<(Action, Vec<[f32; 2]>)> {
    // As the wire carries it, so the reversal and the markers see the
    // points the server works out.
    let line = command::quantize_line(&target.line);
    let length: f32 = line.windows(2).map(|pair| dist(pair[0], pair[1])).sum();
    let (_, able) = own_and_able(scene, selected, player)?;
    // Only units the engine will not filter out, or it pairs the wrong
    // points with them.
    let mut units: Vec<u32> = able
        .into_iter()
        .filter(|id| {
            scene
                .dots
                .get(id)
                .and_then(|dot| template_details(scene, dot.template))
                .is_some_and(|details| details.components & COMPONENT_UNIT_AI != 0)
        })
        .collect();
    // The order the wire's entity list has, which the server pairs the
    // points with.
    units.sort_unstable();
    let n = units.len();
    if length < FREEHAND_MIN_LENGTH || n < FREEHAND_MIN_UNITS || line.len() > command::MAX_LINE {
        return None;
    }
    let mut out = command::freehand_points(&line, n);
    let first = position(scene.dots.get(&units[0])?);
    let last = position(scene.dots.get(&units[n - 1])?);
    let reversed =
        dist(first, out[0]) + dist(last, out[n - 1]) > dist(first, out[n - 1]) + dist(last, out[0]);
    if reversed {
        out.reverse();
    }
    let step = length / (n - 1) as f32;
    let flags = if target.queued { QUEUED } else { 0 };
    let sound = Some(("order_walk", units[0]));
    let command = if target.hotkeys & HOTKEY_GARRISON != 0 {
        Command::AttackWalkCustom {
            flags,
            entities: units,
            line,
            reversed,
        }
    } else {
        Command::WalkCustom {
            flags,
            entities: units,
            line,
            reversed,
        }
    };
    let every = (FREEHAND_MARKER_SPACING / step).ceil().max(1.0) as usize;
    let markers = out.iter().copied().step_by(every).collect();
    Some((
        Action {
            cursor: String::new(),
            tooltip: String::new(),
            command: Some(command),
            sound,
        },
        markers,
    ))
}

/// The player's own among `selected`, and of those the ones getActionInfo
/// does not skip as uncontrollable; none without any own.
fn own_and_able(scene: &Scene, selected: &[u32], player: i32) -> Option<(Vec<u32>, Vec<u32>)> {
    let own: Vec<u32> = selected
        .iter()
        .copied()
        .filter(|id| scene.dots.get(id).is_some_and(|dot| dot.owner == player))
        .collect();
    if own.is_empty() {
        return None;
    }
    let able = own
        .iter()
        .copied()
        .filter(|id| {
            scene
                .dots
                .get(id)
                .and_then(|dot| template_details(scene, dot.template))
                .is_none_or(|details| details.flags & TEMPLATE_UNCONTROLLABLE == 0)
        })
        .collect();
    Some((own, able))
}

/// What the actions that both a hotkey and a command button pick read:
/// each one's `actionCheck`, through `getActionInfo`'s first able entity.
/// The set-rallypoint market branch: the selected market that sets it and
/// `GetTradingRouteGain`'s `traderGain` to `target` for the first trader
/// it trains. `None` where the engine goes on to the next branch, and
/// while the trader's template data has not loaded.
fn rally_trade_gain(
    scene: &Scene,
    own: &[u32],
    player: i32,
    target: u32,
    data: &DataFiles,
) -> Option<(u32, trade::Gain)> {
    // `getActionInfo`'s entState: the first that has a rally point.
    let &market = own.iter().find(|&&id| {
        scene
            .dots
            .get(&id)
            .and_then(|dot| template_details(scene, dot.template))
            .is_some_and(|details| details.components & COMPONENT_RALLY_POINT != 0)
    })?;
    let (_, naval) = trade::market_types(scene, market)?;
    let (_, target_naval) = trade::market_types(scene, target)?;
    let target_dot = scene.dots.get(&target)?;
    let target_bits =
        template_details(scene, target_dot.template).map_or(0, |details| details.components);
    // A foundation has a Market too, but no `market` entity state.
    if market == target
        || target_bits & (COMPONENT_MARKET | COMPONENT_FOUNDATION) != COMPONENT_MARKET
        || (naval && !target_naval)
        || is_enemy(scene, player, target_dot.owner)
    {
        return None;
    }
    let dot = scene.dots.get(&market)?;
    let lists = &scene.details.lists;
    let trainable = match &entity_groups(scene, market, dot.template)[detail::TRAINABLE] {
        Some(Group::List(index)) => lists.get(*index as usize)?,
        _ => return None,
    };
    // `GetTemplateData(...).trader` of the first trainable that has one.
    let mut multiplier = None;
    for item in trainable {
        let key = DataKey::Template {
            name: item.name,
            player: dot.owner,
        };
        let Some(&hash) = scene.details.data.get(&key) else {
            continue;
        };
        let template = data.get(hash)?;
        let trader = tips::field(&template, "trader");
        if trader.is_object() {
            multiplier = Some(tips::number_field(&trader, "GainMultiplier"));
            break;
        }
    }
    Some((
        market,
        trade::calculate_gain(scene, market, target, trade::For::Template(multiplier?))?,
    ))
}

struct Actions<'a> {
    scene: &'a Scene,
    own: &'a [u32],
    able: &'a [u32],
    player: i32,
    target: &'a Target,
    target_dot: Option<(u32, &'a Dot)>,
    flags: u8,
    formation: &'a str,
}

impl Actions<'_> {
    fn details(&self, id: u32) -> Option<&TemplateDetails> {
        let dot = self.scene.dots.get(&id)?;
        template_details(self.scene, dot.template)
    }

    fn has(&self, id: u32, bit: u32) -> bool {
        self.details(id)
            .is_some_and(|details| details.components & bit != 0)
    }

    /// UnitAI's and the like, as the hook reports them.
    fn command_flags(&self, id: u32) -> u32 {
        let Some(dot) = self.scene.dots.get(&id) else {
            return 0;
        };
        match &entity_groups(self.scene, id, dot.template)[detail::COMMANDS] {
            Some(Group::Commands(commands)) => commands.flags,
            _ => 0,
        }
    }

    /// The target, its details' component bits and the player's relation
    /// to its owner.
    fn target(&self) -> Option<(u32, &Dot, u32, Relation)> {
        let (id, dot) = self.target_dot?;
        let bits =
            template_details(self.scene, dot.template).map_or(0, |details| details.components);
        Some((id, dot, bits, relation(self.scene, self.player, dot.owner)))
    }

    /// `playerCheck` with "Player" and "MutualAlly".
    fn own_or_mutual_ally(&self, owner: i32) -> bool {
        owner == self.player
            || matches!(
                (usize::try_from(self.player), usize::try_from(owner)),
                (Ok(player), Ok(owner)) if owner > 0 && mutual_allies(self.scene, player, owner)
            )
    }

    /// The ground clicked, else the target's position, as
    /// `GetTerrainAtScreenPoint` always has one.
    fn point(&self) -> Option<[f32; 2]> {
        self.target
            .point
            .or_else(|| self.target_dot.map(|(_, dot)| position(dot)))
    }

    /// "attack", without capturing.
    fn attack(&self) -> Option<Action> {
        let (id, dot, _, relation) = self.target()?;
        if dot.hitpoints == 0 {
            return None;
        }
        let classes = target_classes(self.scene, self.details(id));
        let &attacker = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_ATTACK)
                && self
                    .details(unit)
                    .is_some_and(|attacker| can_attack(self.scene, attacker, relation, &classes))
        })?;
        Some(Action {
            cursor: "action-attack".to_string(),
            tooltip: String::new(),
            command: Some(Command::Attack {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some(("order_attack", attacker)),
        })
    }

    /// "capture"
    fn capture(&self) -> Option<Action> {
        let (id, _, bits, relation) = self.target()?;
        if bits & COMPONENT_CAPTURABLE == 0 {
            return None;
        }
        let classes = target_classes(self.scene, self.details(id));
        let &attacker = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_ATTACK)
                && self
                    .details(unit)
                    .is_some_and(|attacker| can_capture(self.scene, attacker, relation, &classes))
        })?;
        Some(Action {
            cursor: "action-capture".to_string(),
            tooltip: String::new(),
            command: Some(Command::Attack {
                flags: self.flags | ALLOW_CAPTURE,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some(("order_attack", attacker)),
        })
    }

    /// "repair": builders on a friendly foundation or damaged building.
    fn repair(&self) -> Option<Action> {
        let (id, dot, bits, relation) = self.target()?;
        if !matches!(relation, Relation::Own | Relation::Ally)
            || (bits & COMPONENT_FOUNDATION == 0
                && (bits & COMPONENT_REPAIRABLE == 0 || dot.hitpoints == u8::MAX))
        {
            return None;
        }
        let &builder = self
            .able
            .iter()
            .find(|&&unit| self.has(unit, COMPONENT_BUILDER))?;
        Some(Action {
            cursor: "action-repair".to_string(),
            tooltip: String::new(),
            command: Some(Command::Repair {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some((
                if bits & COMPONENT_FOUNDATION != 0 {
                    "order_build"
                } else {
                    "order_repair"
                },
                builder,
            )),
        })
    }

    /// "garrison": into an own or mutually allied holder whose List the
    /// unit matches; any, from a hook that does not send the List.
    fn garrison(&self) -> Option<Action> {
        let (id, dot, bits, _) = self.target()?;
        if bits & COMPONENT_GARRISON_HOLDER == 0 || !self.own_or_mutual_ally(dot.owner) {
            return None;
        }
        let list = self
            .details(id)
            .map_or(0, |details| details.garrison_classes);
        let &unit = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_GARRISONABLE)
                && (list == 0
                    || string(self.scene, list).is_some_and(|list| {
                        matches_class_list(&target_classes(self.scene, self.details(unit)), list)
                    }))
        })?;
        Some(Action {
            cursor: "action-garrison".to_string(),
            tooltip: String::new(),
            command: Some(Command::Garrison {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some(("order_garrison", unit)),
        })
    }

    /// "occupy-turret": an own or mutually allied holder with a point the
    /// unit may take, full or not.
    fn occupy_turret(&self) -> Option<Action> {
        let (id, dot, bits, _) = self.target()?;
        if bits & COMPONENT_TURRET_HOLDER == 0 || !self.own_or_mutual_ally(dot.owner) {
            return None;
        }
        let points = self.details(id).map_or(0, |details| details.turret_classes);
        let &unit = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_TURRETABLE)
                && (points == 0
                    || string(self.scene, points).is_some_and(|points| {
                        let classes = target_classes(self.scene, self.details(unit));
                        points.split(';').any(|allowed| {
                            allowed.trim().is_empty() || matches_class_list(&classes, allowed)
                        })
                    }))
        })?;
        Some(Action {
            cursor: "action-occupy-turret".to_string(),
            tooltip: String::new(),
            command: Some(Command::OccupyTurret {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some(("order_garrison", unit)),
        })
    }

    /// "guard": an own or allied entity with Guard, other than the unit.
    fn guard(&self) -> Option<Action> {
        let (id, _, bits, relation) = self.target()?;
        if bits & COMPONENT_GUARD == 0 || !matches!(relation, Relation::Own | Relation::Ally) {
            return None;
        }
        let &unit = self.able.iter().find(|&&unit| {
            unit != id
                && self.has(unit, COMPONENT_UNIT_AI)
                && self.command_flags(unit) & Commands::CAN_GUARD != 0
        })?;
        Some(Action {
            cursor: "action-guard".to_string(),
            tooltip: String::new(),
            command: Some(Command::Guard {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some(("order_guard", unit)),
        })
    }

    /// "remove-guard", wherever the mouse is.
    fn remove_guard(&self) -> Option<Action> {
        let &unit = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_UNIT_AI) && self.command_flags(unit) & Commands::GUARDING != 0
        })?;
        Some(Action {
            cursor: "action-remove-guard".to_string(),
            tooltip: String::new(),
            command: Some(Command::RemoveGuard {
                entities: self.own.to_vec(),
            }),
            sound: Some(("order_guard", unit)),
        })
    }

    /// "attack-move" to the point, in no formation.
    fn attack_move(&self) -> Option<Action> {
        let &unit = self
            .able
            .iter()
            .find(|&&unit| self.has(unit, COMPONENT_UNIT_AI))?;
        Some(Action {
            cursor: "action-attack-move".to_string(),
            tooltip: String::new(),
            command: self.point().map(|[x, z]| Command::AttackWalk {
                flags: self.flags,
                entities: self.own.to_vec(),
                x,
                z,
            }),
            sound: Some(("order_attack_move", unit)),
        })
    }

    /// `allowCapture`: `HotkeyIsPressed("session.capture")`.
    fn capture_flag(&self) -> u8 {
        if self.target.hotkeys & HOTKEY_CAPTURE != 0 {
            ALLOW_CAPTURE
        } else {
            0
        }
    }

    /// "patrol" to the point, in the default formation.
    fn patrol(&self) -> Option<Action> {
        let &unit = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_UNIT_AI)
                && self.command_flags(unit) & Commands::CAN_PATROL != 0
        })?;
        Some(Action {
            cursor: "action-patrol".to_string(),
            tooltip: String::new(),
            command: self.point().map(|[x, z]| Command::Patrol {
                flags: self.flags | self.capture_flag(),
                entities: self.own.to_vec(),
                x,
                z,
                target: self.target.entity.unwrap_or(0),
                formation: self.formation.to_string(),
            }),
            sound: Some(("order_patrol", unit)),
        })
    }

    /// "call-to-arms", which plays no sound.
    fn call_to_arms(&self) -> Option<Action> {
        self.able
            .iter()
            .find(|&&unit| self.has(unit, COMPONENT_UNIT_AI))?;
        Some(Action {
            cursor: "action-attack".to_string(),
            tooltip: String::new(),
            command: self.point().map(|[x, z]| Command::CallToArms {
                flags: self.flags | self.capture_flag(),
                entities: self.own.to_vec(),
                x,
                z,
            }),
            sound: None,
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Relation {
    Own,
    Ally,
    Neutral,
    Enemy,
    Gaia,
    /// No owner, which no player check passes.
    None,
}

impl Relation {
    /// `isEnemy`, which gaia is to every player.
    fn enemy(self) -> bool {
        matches!(self, Relation::Enemy | Relation::Gaia)
    }
}

/// The disabled "set second" and rally route's tooltip, in
/// `g_DisabledTags`.
fn too_close() -> String {
    tooltip::colored(
        &lang::translate("This market is too close to the origin market."),
        "255 140 0",
    )
}

/// `isEnemy` of `player` towards `owner`, gaia included.
pub(crate) fn is_enemy(scene: &Scene, player: i32, owner: i32) -> bool {
    relation(scene, player, owner).enemy()
}

/// `player`'s stance towards `owner`.
fn relation(scene: &Scene, player: i32, owner: i32) -> Relation {
    if owner == player {
        return Relation::Own;
    }
    if owner < 0 {
        return Relation::None;
    }
    if owner == 0 {
        return Relation::Gaia;
    }
    let stance = usize::try_from(player)
        .ok()
        .and_then(|player| scene.players.get(player))
        .and_then(|player| player.stances.get(owner as usize));
    match stance {
        Some(Stance::Ally) => Relation::Ally,
        Some(Stance::Neutral) => Relation::Neutral,
        _ => Relation::Enemy,
    }
}

fn position(dot: &Dot) -> [f32; 2] {
    [dot.x as f32 / 16.0, dot.z as f32 / 16.0]
}

/// The type the first resource `id` carries, as `resourceCarrying[0]`.
fn carried(scene: &Scene, id: u32) -> Option<&str> {
    let dot = scene.dots.get(&id)?;
    match &entity_groups(scene, id, dot.template)[detail::CARRYING] {
        Some(Group::Carrying(carried)) => carried
            .iter()
            .find(|carried| carried.amount > 0)
            .and_then(|carried| string(scene, carried.kind)),
        _ => None,
    }
}

/// The gather action's `resource`, the specific type the gatherer has a
/// rate for, else the generic one; none when it cannot gather the supply.
/// Empty when the stream does not say (an older hook), which gathers
/// anything with the default cursor.
fn gathered<'a>(
    scene: &'a Scene,
    gatherer: &TemplateDetails,
    supply: &TemplateDetails,
    can_gather: bool,
) -> Option<&'a str> {
    if !can_gather {
        return None;
    }
    if gatherer.gather_rates == 0 || supply.supply_type == 0 {
        return Some("");
    }
    let rates = string(scene, gatherer.gather_rates)?;
    let kind = string(scene, supply.supply_type)?;
    let (generic, specific) = kind.split_once('.').unwrap_or((kind, ""));
    let rate = |key: &str| rates.split_whitespace().any(|rate| rate == key);
    if rate(kind) {
        Some(specific)
    } else if rate(generic) {
        Some(generic)
    } else {
        None
    }
}

/// `GetIdentityClasses`: Classes and VisibleClasses together.
fn target_classes<'a>(scene: &'a Scene, details: Option<&TemplateDetails>) -> Vec<&'a str> {
    let Some(details) = details else {
        return Vec::new();
    };
    [details.classes, details.visible_classes]
        .into_iter()
        .filter_map(|index| string(scene, index))
        .flat_map(str::split_whitespace)
        .collect()
}

/// The attacker's attack types and each one's RestrictedClasses.
fn attack_types<'a>(scene: &'a Scene, attacker: &TemplateDetails) -> Vec<(&'a str, String)> {
    string(scene, attacker.attack_types)
        .unwrap_or_default()
        .split_whitespace()
        .map(|token| {
            let (kind, restricted) = token.split_once(':').unwrap_or((token, ""));
            (kind, restricted.replace(',', " "))
        })
        .collect()
}

/// `Attack.CanAttack(target, ["!Capture"])`, without the height check.
/// An older hook does not name the types: any enemy then.
fn can_attack(
    scene: &Scene,
    attacker: &TemplateDetails,
    relation: Relation,
    classes: &[&str],
) -> bool {
    if attacker.attack_types == 0 {
        return relation.enemy();
    }
    let types = attack_types(scene, attacker);
    if classes.contains(&"Domestic") && types.iter().any(|(kind, _)| *kind == "Slaughter") {
        return true;
    }
    relation.enemy()
        && types.iter().any(|(kind, restricted)| {
            matches!(*kind, "Melee" | "Ranged") && !matches_class_list(classes, restricted)
        })
}

/// `Attack.CanAttack(target, ["Capture"])`, taking an enemy owner for
/// enemy capture points.
fn can_capture(
    scene: &Scene,
    attacker: &TemplateDetails,
    relation: Relation,
    classes: &[&str],
) -> bool {
    if attacker.attack_types == 0 {
        return relation.enemy();
    }
    relation.enemy()
        && attack_types(scene, attacker)
            .iter()
            .any(|(kind, restricted)| {
                *kind == "Capture" && !matches_class_list(classes, restricted)
            })
}

/// Whether the entity trains anything, as `trainer.entities.length`:
/// with autorallypoint held, vanilla lets a selected building that trains
/// units set its rally point on itself.
fn trains_units(scene: &Scene, id: u32) -> bool {
    let Some(dot) = scene.dots.get(&id) else {
        return false;
    };
    match &entity_groups(scene, id, dot.template)[detail::TRAINABLE] {
        Some(Group::List(index)) => scene
            .details
            .lists
            .get(*index as usize)
            .is_some_and(|list| !list.is_empty()),
        _ => false,
    }
}

/// "set-rallypoint": on a resource gather, from a market on another the
/// default trade route, on a friendly foundation or damaged building
/// repair, on an enemy attack, else walk, which Ctrl (session.attackmove)
/// makes an attack-walk. Garrisoning, turrets and treasures are not
/// ordered, so those show the walk's arrow. None on a selected entity
/// itself, which the engine unsets the rally point on, unless Ctrl
/// (session.autorallypoint) is held and it trains units.
fn rally(
    scene: &Scene,
    own: &[u32],
    player: i32,
    target: &Target,
    target_dot: Option<(u32, &Dot)>,
    flags: u8,
    data: &DataFiles,
) -> Option<Action> {
    let ctrl = target.hotkeys & HOTKEY_GARRISON != 0;
    let mut resource = (String::new(), String::new());
    let mut source = 0;
    let mut tooltip = String::new();
    let (cursor, order, rally_target, point) = match target_dot {
        Some((id, _)) if own.contains(&id) && !(ctrl && trains_units(scene, id)) => return None,
        Some((id, dot)) => {
            let details = template_details(scene, dot.template);
            let bits = details.map_or(0, |details| details.components);
            let relation = relation(scene, player, dot.owner);
            let friendly = matches!(relation, Relation::Own | Relation::Ally);
            let (cursor, order, rally_target) = if bits & COMPONENT_RESOURCE_SUPPLY != 0 {
                let kind = details
                    .and_then(|details| string(scene, details.supply_type))
                    .unwrap_or_default();
                let specific = kind.split_once('.').map_or("", |(_, specific)| specific);
                let template = scene.templates.get(dot.template as usize);
                resource = (kind.to_string(), template.cloned().unwrap_or_default());
                let cursor = if specific.is_empty() {
                    String::new()
                } else {
                    format!("action-gather-{specific}")
                };
                (cursor, "gather", id)
            } else if let Some((market, gain)) = rally_trade_gain(scene, own, player, id, data) {
                if gain.trader_gain == 0.0 {
                    return Some(Action {
                        cursor: "action-setup-trade-route-disabled".to_string(),
                        tooltip: too_close(),
                        command: None,
                        sound: None,
                    });
                }
                tooltip = format!(
                    "{}\n{}",
                    lang::translate("Right-click to establish a default route for new traders."),
                    trade::gain_line(scene, Some(&gain))
                );
                source = market;
                ("action-setup-trade-route".to_string(), "trade", id)
            } else if friendly
                && (bits & COMPONENT_FOUNDATION != 0
                    || (bits & COMPONENT_REPAIRABLE != 0 && dot.hitpoints < 255))
            {
                ("action-repair".to_string(), "repair", id)
            } else if relation.enemy() {
                if dot.hitpoints > 0 {
                    ("action-attack".to_string(), "attack", id)
                } else if bits & COMPONENT_CAPTURABLE != 0 {
                    ("action-capture".to_string(), "attack", id)
                } else {
                    return None;
                }
            } else if ctrl {
                ("action-attack-move".to_string(), "attack-walk", 0)
            } else {
                (String::new(), "walk", 0)
            };
            (cursor, order, rally_target, Some(position(dot)))
        }
        None => {
            if ctrl {
                (
                    "action-attack-move".to_string(),
                    "attack-walk",
                    0,
                    target.point,
                )
            } else {
                (String::new(), "walk", 0, target.point)
            }
        }
    };
    Some(Action {
        cursor,
        tooltip,
        command: point.map(|[x, z]| Command::Rally {
            flags,
            entities: own.to_vec(),
            x,
            z,
            order: order.to_string(),
            target: rally_target,
            resource_type: resource.0,
            resource_template: resource.1,
            source,
        }),
        // unit_actions.js plays nothing on setting a rally point.
        sound: None,
    })
}
