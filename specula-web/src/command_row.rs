//! The command row at the bottom of the details panel
//! (`g_SelectionPanels.Command` and `g_EntityCommands` in
//! selection_panels.js and unit_actions.js): up to six 32 px buttons, which
//! only the commands the viewer is allowed to see make. A pure observer is
//! allowed only focus-rally and the shared dropsite lock; the player of
//! `/play` sees the rest, all of which act. Their hotkeys go through
//! `CommandRow::hotkey`.

use specula_assets::command::Command;
use web_sys::Document;
use web_sys::HtmlElement;

use crate::detail;
use crate::detail::Commands;
use crate::detail::Group;
use crate::detail::Groups;
use crate::lang;
use crate::orders::Preselect;
use crate::panel::Out;
use crate::panel::Request;
use crate::panel::View;
use crate::panel::create;
use crate::panel::entity_groups;
use crate::panel::template_details;
use crate::pick;
use crate::predict::Predictions;
use crate::stream::COMPONENT_BUILDER;
use crate::stream::COMPONENT_GARRISON_HOLDER;
use crate::stream::COMPONENT_GARRISONABLE;
use crate::stream::COMPONENT_MARKET;
use crate::stream::COMPONENT_RALLY_POINT;
use crate::stream::COMPONENT_RESOURCE_SUPPLY;
use crate::stream::COMPONENT_TRAINER;
use crate::stream::COMPONENT_TURRET_HOLDER;
use crate::stream::COMPONENT_TURRETABLE;
use crate::stream::COMPONENT_UNIT_AI;
use crate::stream::Scene;
use crate::stream::Stance;
use crate::stream::TEMPLATE_UNDELETABLE;
use crate::tooltip;

/// `g_SelectionPanels.Command.getMaxNumberOfItems`.
const MAX_COMMANDS: usize = 6;
/// A button and its margin, `size.bottom + 1`.
const COMMAND_STEP: f64 = 33.0;
/// `call-to-arms`' classes.
const ATTACK_CLASSES: [&str; 4] = ["Soldier", "Warship", "Siege", "Healer"];

/// `allowedPlayers` of a command.
const PLAYER: u8 = 1;
const ALLY: u8 = 2;
const OBSERVER: u8 = 4;

/// `g_HotkeyTags`.
const HOTKEY_COLOR: &str = "255 251 131";

/// What a command shows: `getInfo`'s result.
struct Info {
    /// Its key in `g_EntityCommands`.
    name: &'static str,
    /// The key of the icon in `session.css`.
    icon: &'static str,
    tooltip: String,
    count: u32,
    enabled: bool,
    /// What `execute` does.
    press: Option<Press>,
}

/// What a command's `execute` does.
#[derive(Clone)]
enum Press {
    /// Orders, for the player of `/play`; some commands post two.
    Orders(Vec<Command>),
    /// Waits for a left click on the view.
    Preselect(Preselect),
    /// `Engine.CameraMoveTo`, x and z in metres.
    Focus([f32; 2]),
    /// `g_TradeDialog.toggle`.
    ToggleTrade,
}

impl Press {
    /// The orders, none when they would command nothing.
    fn orders(orders: Vec<Command>) -> Option<Press> {
        let orders: Vec<Command> = orders
            .into_iter()
            .filter(|order| !commands_none(order))
            .collect();
        (!orders.is_empty()).then_some(Press::Orders(orders))
    }

    fn order(order: Command) -> Option<Press> {
        Press::orders(vec![order])
    }
}

/// An order to an empty list, which `PostNetworkCommand` would still send
/// but which does nothing.
fn commands_none(order: &Command) -> bool {
    match order {
        Command::UnloadAll { holders } | Command::UnloadAllByOwner { holders } => {
            holders.is_empty()
        }
        Command::UnloadTurrets { entities }
        | Command::LeaveTurret { entities }
        | Command::BackToWork { entities }
        | Command::RemoveGuard { entities }
        | Command::DropsiteSharing { entities, .. }
        | Command::Autoqueue { entities, .. }
        | Command::Stop { entities, .. }
        | Command::Delete { entities } => entities.is_empty(),
        _ => false,
    }
}

/// The keys of the command row's hotkeys and the idle worker button's, as
/// `formatHotkeyCombinations`
/// writes the engine's defaults (`config/default.cfg`); none for the
/// unassigned ones.
fn hotkey_keys(hotkey: &str) -> Option<&'static str> {
    Some(match hotkey {
        "session.unload" | "session.unloadturrets" => "U",
        "session.kill" => "Delete, Backspace",
        "session.stop" => "H",
        "session.garrison" | "session.occupyturret" => "Ctrl",
        "session.repair" => "J",
        "session.backtowork" => "Y",
        "session.guard" => "G",
        "session.patrol" => "P",
        "session.gui.barter.toggle" => "Ctrl+B",
        "session.massbarter" | "session.fulltradeswap" => "Shift",
        // `GetScancodeKeyNames` gives SDL's names, "." and "Keypad .".
        "selection.idleworker" => "., Keypad .",
        _ => return None,
    })
}

/// `colorizeHotkey("%(hotkey)s" + " ", hotkey)`.
pub(crate) fn hotkey_prefix(hotkey: &str) -> String {
    format!("{} ", hotkey_text(hotkey))
}

/// `colorizeHotkey("%(hotkey)s", hotkey)`.
pub(crate) fn hotkey_text(hotkey: &str) -> String {
    let key = match hotkey_keys(hotkey) {
        Some(key) => key.to_string(),
        None => lang::format(
            &lang::translate("Unassigned hotkey: %(hotkeyName)s"),
            &[("hotkeyName", hotkey)],
        ),
    };
    tooltip::colored(&format!("\\[{key}]"), HOTKEY_COLOR)
}

/// A selected entity.
struct Selected {
    id: u32,
    owner: i32,
    template: u32,
    groups: Groups,
}

/// What a command reads.
struct Context<'a> {
    scene: &'a Scene,
    selected: &'a [Selected],
    /// The player the viewer controls, `None` for an observer.
    player: Option<i32>,
    predicted: &'a Predictions,
}

impl Context<'_> {
    fn controls(&self, owner: i32) -> bool {
        self.player == Some(owner)
    }

    /// `allowedPlayersCheck`, which looks at the first entity.
    fn allowed(&self, allowed: u8) -> bool {
        let Some(first) = self.selected.first() else {
            return false;
        };
        if allowed & OBSERVER != 0 && self.player.is_none() {
            return true;
        }
        if allowed & PLAYER != 0 && self.controls(first.owner) {
            return true;
        }
        allowed & ALLY != 0
            && self.player.is_some_and(|player| {
                usize::try_from(player)
                    .ok()
                    .and_then(|player| self.scene.players.get(player))
                    .and_then(|player| {
                        usize::try_from(first.owner)
                            .ok()
                            .and_then(|owner| player.stances.get(owner))
                    })
                    == Some(&Stance::Ally)
            })
    }

    fn has_component(&self, entity: &Selected, bit: u32) -> bool {
        template_details(self.scene, entity.template)
            .is_some_and(|details| details.components & bit != 0)
    }

    fn has_class(&self, entity: &Selected, class: &str) -> bool {
        template_details(self.scene, entity.template)
            .is_some_and(|details| pick::has_class(details, &self.scene.strings, class))
    }

    fn flags(&self, entity: &Selected) -> u32 {
        match &entity.groups[detail::COMMANDS] {
            Some(Group::Commands(commands)) => commands.flags,
            _ => 0,
        }
    }

    fn holder(&self, entity: &Selected) -> u32 {
        match &entity.groups[detail::HOLDER] {
            Some(Group::Holder(holder)) => *holder,
            _ => 0,
        }
    }

    /// Whether the dropsite is shared, as predicted before the stream says.
    fn shared(&self, entity: &Selected) -> bool {
        self.predicted
            .dropsite_shared(entity.id)
            .unwrap_or(self.flags(entity) & Commands::SHARED != 0)
    }

    /// The player's entities on `holder`'s turret points, which
    /// `unloadAllTurrets` finds through the points and here through each
    /// occupant's holder.
    fn own_turrets_on(&self, holder: u32) -> Vec<u32> {
        let Some(player) = self.player else {
            return Vec::new();
        };
        self.scene
            .dots
            .iter()
            .filter(|(_, dot)| dot.owner == player)
            .filter(|(_, dot)| {
                template_details(self.scene, dot.template)
                    .is_some_and(|details| details.components & COMPONENT_TURRETABLE != 0)
            })
            .filter(|&(&id, dot)| {
                matches!(
                    &entity_groups(self.scene, id, dot.template)[detail::HOLDER],
                    Some(Group::Holder(on)) if *on == holder
                )
            })
            .map(|(&id, _)| id)
            .collect()
    }

    fn queue<'a>(&self, entity: &'a Selected) -> Option<&'a detail::Queue> {
        match &entity.groups[detail::QUEUE] {
            Some(Group::Queue(queue)) => Some(queue),
            _ => None,
        }
    }

    /// `isUndeletable`: why the entity cannot be deleted.
    fn undeletable(&self, entity: &Selected) -> Option<String> {
        // KillBeforeGather is the fauna's, which share the class Animal.
        if self.has_component(entity, COMPONENT_RESOURCE_SUPPLY) && self.has_class(entity, "Animal")
        {
            return Some(lang::translate(
                "The entity has to be killed before it can be gathered from",
            ));
        }
        if let Some(Group::Capture { points, max }) = &entity.groups[detail::CAPTURE]
            && let Some(own) = usize::try_from(entity.owner)
                .ok()
                .and_then(|owner| points.get(owner))
            && f64::from(*own) < f64::from(*max) / 2.0
        {
            return Some(lang::translate(
                "You cannot destroy this entity as you own less than half the capture points",
            ));
        }
        if template_details(self.scene, entity.template)
            .is_some_and(|details| details.flags & TEMPLATE_UNDELETABLE != 0)
        {
            return Some(lang::translate("This entity is undeletable"));
        }
        None
    }
}

/// A title and its body line, as the commands' tooltips make them.
fn tip(title: &str, body: &str) -> String {
    format!(
        "{}\n{}",
        lang::translate(title),
        tooltip::body_font(&lang::translate(body))
    )
}

/// `tip` after its hotkey.
fn hotkey_tip(hotkey: &str, title: &str, body: &str) -> String {
    format!("{}{}", hotkey_prefix(hotkey), tip(title, body))
}

/// `g_EntityCommands`, in its order.
fn infos(context: &Context) -> Vec<Info> {
    let selected = context.selected;
    let scene = context.scene;
    let mut infos = Vec::new();
    let mut add = |allowed: u8, info: Option<Info>| {
        if let Some(info) = info.filter(|_| context.allowed(allowed)) {
            infos.push(info);
        }
    };
    let any = |test: &dyn Fn(&Selected) -> bool| selected.iter().any(test);
    let ids = |test: &dyn Fn(&Selected) -> bool| -> Vec<u32> {
        selected
            .iter()
            .filter(|entity| test(entity))
            .map(|entity| entity.id)
            .collect()
    };

    // unload-all
    let unloadable: u32 = selected
        .iter()
        .filter(|entity| context.has_component(entity, COMPONENT_GARRISON_HOLDER))
        .map(|entity| match &entity.groups[detail::GARRISON] {
            Some(Group::Garrison { entities, .. }) if context.controls(entity.owner) => {
                entities.len() as u32
            }
            Some(Group::Garrison { entities, .. }) => entities
                .iter()
                .filter(|garrisoned| {
                    garrisoned
                        .owner
                        .is_some_and(|owner| context.controls(owner))
                })
                .count() as u32,
            _ => 0,
        })
        .sum();
    // `unloadAll`: the own holders emptied, the others' of the player's.
    let holders = |own: bool| {
        ids(&|entity| {
            context.has_component(entity, COMPONENT_GARRISON_HOLDER)
                && context.controls(entity.owner) == own
        })
    };
    add(
        PLAYER | ALLY,
        (unloadable > 0).then(|| Info {
            name: "unload-all",
            icon: "command-garrison-out",
            tooltip: hotkey_tip(
                "session.unload",
                "Unload All",
                "Order all units to leave the selected entities.",
            ),
            count: unloadable,
            enabled: true,
            press: Press::orders(vec![
                Command::UnloadAll {
                    holders: holders(true),
                },
                Command::UnloadAllByOwner {
                    holders: holders(false),
                },
            ]),
        }),
    );

    // unload-all-turrets: the ejectable occupants of the own holders, and
    // the player's own on the others'.
    let mut turrets = 0;
    let mut own_turret_holders = Vec::new();
    let mut ejectables = Vec::new();
    for entity in selected
        .iter()
        .filter(|entity| context.has_component(entity, COMPONENT_TURRET_HOLDER))
    {
        if context.controls(entity.owner) {
            own_turret_holders.push(entity.id);
            turrets += match &entity.groups[detail::COMMANDS] {
                Some(Group::Commands(commands)) => commands.turrets,
                _ => 0,
            };
        } else {
            let occupants = context.own_turrets_on(entity.id);
            turrets += occupants.len() as u32;
            ejectables.extend(occupants);
        }
    }
    add(
        PLAYER | ALLY,
        (turrets > 0).then(|| Info {
            name: "unload-all-turrets",
            icon: "command-garrison-out",
            tooltip: hotkey_tip(
                "session.unloadturrets",
                "Unload Turrets",
                "Order all units to leave the selected turret points.",
            ),
            count: turrets,
            enabled: true,
            press: Press::orders(vec![
                Command::LeaveTurret {
                    entities: ejectables,
                },
                Command::UnloadTurrets {
                    entities: own_turret_holders,
                },
            ]),
        }),
    );

    // delete
    let reasons: Vec<Option<String>> = selected
        .iter()
        .map(|entity| context.undeletable(entity))
        .collect();
    add(
        PLAYER,
        Some(if reasons.iter().any(Option::is_none) {
            Info {
                name: "delete",
                icon: "command-delete",
                tooltip: hotkey_tip(
                    "session.kill",
                    "Self-Destruct",
                    "Destroy the selected entities.",
                ),
                count: 0,
                enabled: true,
                press: Press::order(Command::Delete {
                    entities: ids(&|entity| context.undeletable(entity).is_none()),
                }),
            }
        } else {
            let mut unique: Vec<String> = Vec::new();
            for reason in reasons.into_iter().flatten() {
                if !unique.contains(&reason) {
                    unique.push(reason);
                }
            }
            Info {
                name: "delete",
                icon: "command-delete-disabled",
                tooltip: unique.join("\n"),
                count: 0,
                enabled: false,
                press: None,
            }
        }),
    );

    // stop
    let idle = |entity: &Selected| {
        !context.has_component(entity, COMPONENT_UNIT_AI)
            || matches!(
                &entity.groups[detail::UNIT_AI],
                Some(Group::UnitAi { idle: true, .. })
            )
            || context.flags(entity) & Commands::GUARDING != 0
    };
    add(
        PLAYER,
        (!selected.iter().all(idle)).then(|| Info {
            name: "stop",
            icon: "stop",
            tooltip: hotkey_tip(
                "session.stop",
                "Abort",
                "Cancel the current orders for the selected units.",
            ),
            count: 0,
            enabled: true,
            press: Press::order(Command::Stop {
                flags: 0,
                entities: ids(&|_| true),
            }),
        }),
    );

    // call-to-arms
    add(
        PLAYER,
        any(&|entity| ATTACK_CLASSES.iter().any(|class| context.has_class(entity, class))).then(
            || Info {
                name: "call-to-arms",
                icon: "command-call-to-arms",
                tooltip: hotkey_tip(
                    "session.calltoarms",
                    "Attack",
                    "Send the selected units on attack move to the specified location after dropping resources.",
                ),
                count: 0,
                enabled: true,
                press: Some(Press::Preselect(Preselect::CallToArms)),
            },
        ),
    );

    // garrison
    add(
        PLAYER,
        any(&|entity| {
            context.has_component(entity, COMPONENT_GARRISONABLE) && context.holder(entity) == 0
        })
        .then(|| Info {
            name: "garrison",
            icon: "command-garrison",
            tooltip: hotkey_tip(
                "session.garrison",
                "Garrison",
                "Order the selected units to garrison in a structure or another unit.",
            ),
            count: 0,
            enabled: true,
            press: Some(Press::Preselect(Preselect::Garrison)),
        }),
    );

    // occupy-turret
    add(
        PLAYER,
        any(&|entity| {
            context.has_component(entity, COMPONENT_TURRETABLE) && context.holder(entity) == 0
        })
        .then(|| Info {
            name: "occupy-turret",
            icon: "command-occupy-turret",
            tooltip: hotkey_tip(
                "session.occupyturret",
                "Occupy Turret",
                "Order the selected units to occupy a turret point.",
            ),
            count: 0,
            enabled: true,
            press: Some(Press::Preselect(Preselect::OccupyTurret)),
        }),
    );

    // leave-turret
    let on_turret = |entity: &Selected| {
        context.has_component(entity, COMPONENT_TURRETABLE) && context.holder(entity) != 0
    };
    add(
        PLAYER,
        any(&|entity| on_turret(entity) && context.flags(entity) & Commands::EJECTABLE != 0).then(
            || Info {
                name: "leave-turret",
                icon: "command-leave-turret",
                tooltip: tip(
                    "Unload",
                    "Order the selected units to leave their turret points.",
                ),
                count: 0,
                enabled: true,
                press: Press::order(Command::LeaveTurret {
                    entities: ids(&on_turret),
                }),
            },
        ),
    );

    // repair
    add(
        PLAYER,
        any(&|entity| context.has_component(entity, COMPONENT_BUILDER)).then(|| Info {
            name: "repair",
            icon: "repair",
            tooltip: hotkey_tip(
                "session.repair",
                "Repair",
                "Order the selected units to repair a structure, ship, or siege engine.",
            ),
            count: 0,
            enabled: true,
            press: Some(Press::Preselect(Preselect::Repair)),
        }),
    );

    // focus-rally: the first rally point, else the first entity.
    let focus = selected
        .iter()
        .find_map(|entity| match &entity.groups[detail::RALLY] {
            Some(Group::Rally { points, .. }) => points.first().map(|point| {
                [
                    point.x as f32 / detail::RALLY_UNITS_PER_METRE,
                    point.z as f32 / detail::RALLY_UNITS_PER_METRE,
                ]
            }),
            _ => None,
        })
        .or_else(|| {
            selected.iter().find_map(|entity| {
                let dot = scene.dots.get(&entity.id)?;
                Some([dot.x as f32 / 16.0, dot.z as f32 / 16.0])
            })
        });
    add(
        PLAYER | OBSERVER,
        any(&|entity| context.has_component(entity, COMPONENT_RALLY_POINT)).then(|| Info {
            name: "focus-rally",
            icon: "command-focus-rally",
            tooltip: hotkey_tip(
                "camera.rallypointfocus",
                "Focus on Rally Point",
                "Center the view on the selected rally point.",
            ),
            count: 0,
            enabled: true,
            press: focus.map(Press::Focus),
        }),
    );

    // back-to-work
    let working = |entity: &Selected| context.flags(entity) & Commands::WORK_ORDERS != 0;
    add(
        PLAYER,
        any(&working).then(|| Info {
            name: "back-to-work",
            icon: "command-back-to-work",
            tooltip: hotkey_tip(
                "session.backtowork",
                "Back to Work",
                "Order the selected units to resume their work.",
            ),
            count: 0,
            enabled: true,
            press: Press::order(Command::BackToWork {
                entities: ids(&working),
            }),
        }),
    );

    // add-guard and remove-guard
    let can_guard = |entity: &Selected| {
        context.has_component(entity, COMPONENT_UNIT_AI)
            && context.flags(entity) & Commands::CAN_GUARD != 0
            && context.flags(entity) & Commands::GUARDING == 0
    };
    add(
        PLAYER,
        any(&can_guard).then(|| Info {
            name: "add-guard",
            icon: "command-add-guard",
            tooltip: hotkey_tip(
                "session.guard",
                "Guard",
                "Order the selected units to guard a structure or unit.",
            ),
            count: 0,
            enabled: true,
            press: Some(Press::Preselect(Preselect::Guard)),
        }),
    );
    let guarding = |entity: &Selected| {
        context.has_component(entity, COMPONENT_UNIT_AI)
            && context.flags(entity) & Commands::GUARDING != 0
    };
    add(
        PLAYER,
        any(&guarding).then(|| Info {
            name: "remove-guard",
            icon: "command-remove-guard",
            tooltip: tip("Abort Guard", "Order the selected units to stop guarding."),
            count: 0,
            enabled: true,
            press: Press::order(Command::RemoveGuard {
                entities: ids(&guarding),
            }),
        }),
    );

    // select-trading-goods
    add(
        PLAYER,
        any(&|entity| context.has_component(entity, COMPONENT_MARKET)).then(|| Info {
            name: "select-trading-goods",
            icon: "command-economics",
            tooltip: hotkey_tip(
                "session.gui.barter.toggle",
                "Barter & Trade",
                "Open the dialog for managing resource trading and bartering.",
            ),
            count: 0,
            enabled: true,
            press: Some(Press::ToggleTrade),
        }),
    );

    // patrol
    add(
        PLAYER,
        any(&|entity| context.flags(entity) & Commands::CAN_PATROL != 0).then(|| Info {
            name: "patrol",
            icon: "command-patrol",
            tooltip: hotkey_tip(
                "session.patrol",
                "Patrol",
                "Order to repeatedly go to a point and come back, attacking all enemies along the way.",
            ),
            count: 0,
            enabled: true,
            press: Some(Press::Preselect(Preselect::Patrol)),
        }),
    );

    // share-dropsite and is-dropsite-shared
    let sharable: Vec<&Selected> = selected
        .iter()
        .filter(|entity| context.flags(entity) & Commands::SHARABLE != 0)
        .collect();
    let has_allies = |owner: i32| {
        usize::try_from(owner)
            .ok()
            .and_then(|owner| scene.players.get(owner).map(|player| (owner, player)))
            .is_some_and(|(owner, _)| {
                (0..scene.players.len())
                    .any(|other| other != owner && crate::panel::mutual_allies(scene, owner, other))
            })
    };
    let shared = |entity: &Selected| context.shared(entity);
    add(
        PLAYER,
        (!sharable.is_empty() && selected.iter().any(|entity| has_allies(entity.owner))).then(
            || {
                let locked = sharable.iter().any(|entity| !shared(entity));
                // `shared` is whether any was locked.
                let press = Press::order(Command::DropsiteSharing {
                    shared: locked,
                    entities: sharable.iter().map(|entity| entity.id).collect(),
                });
                if locked {
                    Info {
                        name: "share-dropsite",
                        icon: "command-locked",
                        tooltip: tip(
                            "Share Dropsite",
                            "Allow allies to use this dropsite, now locked.",
                        ),
                        count: 0,
                        enabled: true,
                        press,
                    }
                } else {
                    Info {
                        name: "share-dropsite",
                        icon: "command-unlocked",
                        tooltip: tip(
                            "Lock Dropsite",
                            "Prevent allies from using this dropsite, now shared.",
                        ),
                        count: 0,
                        enabled: true,
                        press,
                    }
                }
            },
        ),
    );
    add(
        ALLY | OBSERVER,
        (!sharable.is_empty() && !sharable.iter().all(|entity| context.controls(entity.owner)))
            .then(|| {
                if sharable.iter().all(|entity| shared(entity)) {
                    Info {
                        name: "is-dropsite-shared",
                        icon: "command-unlocked",
                        tooltip: tip(
                            "Shared Dropsite",
                            if context.player.is_none() {
                                "Allies are allowed to use this dropsite."
                            } else {
                                "You are allowed to use this dropsite."
                            },
                        ),
                        count: 0,
                        enabled: false,
                        press: None,
                    }
                } else {
                    Info {
                        name: "is-dropsite-shared",
                        icon: "command-locked",
                        tooltip: tip("Locked Dropsite", "The use of this dropsite is prohibited."),
                        count: 0,
                        enabled: false,
                        press: None,
                    }
                }
            }),
    );

    // autoqueue-on and autoqueue-off
    let autoqueue = |entity: &Selected| {
        context.queue(entity).is_some_and(|queue| {
            context
                .predicted
                .autoqueue(entity.id)
                .unwrap_or(queue.autoqueue)
        })
    };
    // `turnAutoQueueOn` and `turnAutoQueueOff`: the trainers it changes.
    let switched = |on: bool| {
        Press::order(Command::Autoqueue {
            on,
            entities: ids(&|entity| {
                context.has_component(entity, COMPONENT_TRAINER)
                    && context.queue(entity).is_some()
                    && autoqueue(entity) != on
            }),
        })
    };
    add(
        PLAYER,
        any(&|entity| {
            context.has_component(entity, COMPONENT_TRAINER)
                && context.queue(entity).is_some()
                && !autoqueue(entity)
        })
        .then(|| Info {
            name: "autoqueue-on",
            icon: "command-autoqueue-on",
            tooltip: hotkey_tip(
                "session.queueunit.autoqueueon",
                "Activate Auto-Queue",
                "Activate the production auto-queue for the selected structures.",
            ),
            count: 0,
            enabled: true,
            press: switched(true),
        }),
    );
    add(
        PLAYER,
        any(&autoqueue).then(|| Info {
            name: "autoqueue-off",
            icon: "command-autoqueue-off",
            tooltip: hotkey_tip(
                "session.queueunit.autoqueueoff",
                "Deactivate Auto-Queue",
                "Deactivate the production auto-queue for the selected structures.",
            ),
            count: 0,
            enabled: true,
            press: switched(false),
        }),
    );
    infos
}

/// One `unitCommandButton`.
struct Button {
    button: Out,
    icon: Out,
    count: Out,
}

pub(crate) struct CommandRow {
    root: Out,
    buttons: Vec<Button>,
    /// What a press of each shown button does.
    presses: Vec<Option<Press>>,
    /// Every command the viewer may use on the selection, shown or past
    /// the sixth, by name, for the hotkeys.
    named: Vec<(&'static str, Press)>,
}

impl CommandRow {
    /// `None` on a page from before the command row.
    pub(crate) fn find(document: &Document) -> Option<Result<Self, String>> {
        let row = crate::element::<HtmlElement>(document, "details-command-cells").ok()?;
        Some(Self::new(document, &row))
    }

    fn new(document: &Document, row: &HtmlElement) -> Result<Self, String> {
        let mut buttons = Vec::new();
        for _ in 0..MAX_COMMANDS {
            let button: HtmlElement = create(document, "div")?;
            let _ = button.set_attribute("class", "command");
            let _ = button.set_attribute(tooltip::STYLE_ATTRIBUTE, "sessionToolTipBottomBold");
            let icon: HtmlElement = create(document, "div")?;
            let _ = icon.set_attribute("class", "icon");
            let count: HtmlElement = create(document, "div")?;
            let _ = count.set_attribute("class", "text center sans-bold-stroke-14");
            for child in [&icon, &count] {
                button
                    .append_child(child)
                    .map_err(|_| "cannot build a command button")?;
            }
            row.append_child(&button)
                .map_err(|_| "cannot add a command button")?;
            let mut button = Button {
                button: Out::new(button),
                icon: Out::new(icon),
                count: Out::new(count),
            };
            button.button.hidden(true);
            buttons.push(button);
        }
        Ok(CommandRow {
            root: Out::new(row.clone()),
            buttons,
            presses: vec![None; MAX_COMMANDS],
            named: Vec::new(),
        })
    }

    /// `g_SelectionPanels.Command`: the commands the viewer may see for
    /// the selection, centred.
    pub(crate) fn show(&mut self, view: &View, entities: &[u32]) {
        let scene = view.scene;
        let selected: Vec<Selected> = entities
            .iter()
            .filter_map(|&id| {
                let dot = scene.dots.get(&id)?;
                Some(Selected {
                    id,
                    owner: dot.owner,
                    template: dot.template,
                    groups: entity_groups(scene, id, dot.template),
                })
            })
            .collect();
        let context = Context {
            scene,
            selected: &selected,
            player: view.player,
            predicted: view.predicted,
        };
        let mut infos = infos(&context);
        self.named = infos
            .iter()
            .filter(|info| info.enabled)
            .filter_map(|info| Some((info.name, info.press.clone()?)))
            .collect();
        infos.truncate(MAX_COMMANDS);
        self.root.hidden(infos.is_empty());
        let total = infos.len() as f64;
        self.presses.fill(None);
        for (index, button) in self.buttons.iter_mut().enumerate() {
            let Some(info) = infos.get_mut(index) else {
                button.button.hidden(true);
                continue;
            };
            self.presses[index] = info.press.take().filter(|_| info.enabled);
            button.button.style(format!(
                "left: calc(50% + {}px)",
                (index as f64 - total / 2.0) * COMMAND_STEP
            ));
            // An autoqueue or dropsite button drawn from an order not yet
            // confirmed.
            let predicted = match info.name {
                "autoqueue-on" | "autoqueue-off" => context
                    .selected
                    .iter()
                    .any(|entity| context.predicted.autoqueue(entity.id).is_some()),
                "share-dropsite" => context
                    .selected
                    .iter()
                    .any(|entity| context.predicted.dropsite_shared(entity.id).is_some()),
                _ => false,
            };
            button
                .button
                .class(String::from(match (info.enabled, predicted) {
                    (true, false) => "command",
                    (true, true) => "command predicted",
                    (false, _) => "command disabled",
                }));
            button
                .icon
                .style(format!("background-image: var(--gui-{})", info.icon));
            button.count.text(&if info.count > 0 {
                info.count.to_string()
            } else {
                String::new()
            });
            button.button.tooltip(&info.tooltip);
            button.button.hidden(false);
        }
    }

    pub(crate) fn buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.buttons.iter().map(|button| &button.button.element)
    }

    /// Button `index` was pressed: what it does, but delete without the
    /// session.noconfirmation key (Shift) asks first, as
    /// `DeleteSelectionConfirmation`.
    pub(crate) fn clicked(&self, index: usize, shift: bool) -> Option<Request> {
        request(self.presses.get(index)?.clone()?, shift)
    }

    /// A hotkey of command `name`, which acts as its button would, shown
    /// or not, and does nothing where there is no such button.
    pub(crate) fn hotkey(&self, name: &str, shift: bool) -> Option<Request> {
        let (_, press) = self.named.iter().find(|(named, _)| *named == name)?;
        request(press.clone(), shift)
    }
}

fn request(press: Press, shift: bool) -> Option<Request> {
    Some(match press {
        Press::Orders(orders) => match orders.as_slice() {
            [Command::Delete { entities }] if !shift => Request::ConfirmDelete(entities.clone()),
            _ => Request::Orders(orders),
        },
        Press::Preselect(preselect) => Request::Preselect(preselect),
        Press::Focus([x, z]) => Request::Focus { x, z },
        Press::ToggleTrade => Request::ToggleTrade,
    })
}
