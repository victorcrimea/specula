//! The command row at the bottom of the details panel
//! (`g_SelectionPanels.Command` and `g_EntityCommands` in
//! selection_panels.js and unit_actions.js): up to six 32 px buttons, which
//! only the commands the viewer is allowed to see make. A pure observer is
//! allowed only focus-rally and the shared dropsite lock, so those are all
//! Specula shows today; the others are here for the playable client, which
//! passes the player it controls.

use web_sys::Document;
use web_sys::HtmlElement;

use crate::detail;
use crate::detail::Commands;
use crate::detail::Group;
use crate::detail::Groups;
use crate::lang;
use crate::panel::Out;
use crate::panel::View;
use crate::panel::create;
use crate::panel::entity_groups;
use crate::panel::template_details;
use crate::pick;
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

/// What a command shows: `getInfo`'s result.
struct Info {
    /// The key of the icon in `session.css`.
    icon: &'static str,
    tooltip: String,
    count: u32,
    enabled: bool,
}

/// A selected entity.
struct Selected {
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

/// `g_EntityCommands`, in its order: name-less, as only the order and the
/// allowed players matter here.
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
    add(
        PLAYER | ALLY,
        (unloadable > 0).then(|| Info {
            icon: "command-garrison-out",
            tooltip: tip(
                "Unload All",
                "Order all units to leave the selected entities.",
            ),
            count: unloadable,
            enabled: true,
        }),
    );

    // unload-all-turrets
    let turrets: u32 = selected
        .iter()
        .filter(|entity| context.has_component(entity, COMPONENT_TURRET_HOLDER))
        .filter(|entity| context.controls(entity.owner))
        .map(|entity| match &entity.groups[detail::COMMANDS] {
            Some(Group::Commands(commands)) => commands.turrets,
            _ => 0,
        })
        .sum();
    add(
        PLAYER | ALLY,
        (turrets > 0).then(|| Info {
            icon: "command-garrison-out",
            tooltip: tip(
                "Unload Turrets",
                "Order all units to leave the selected turret points.",
            ),
            count: turrets,
            enabled: true,
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
                icon: "command-delete",
                tooltip: tip("Self-Destruct", "Destroy the selected entities."),
                count: 0,
                enabled: true,
            }
        } else {
            let mut unique: Vec<String> = Vec::new();
            for reason in reasons.into_iter().flatten() {
                if !unique.contains(&reason) {
                    unique.push(reason);
                }
            }
            Info {
                icon: "command-delete-disabled",
                tooltip: unique.join("\n"),
                count: 0,
                enabled: false,
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
            icon: "stop",
            tooltip: tip("Abort", "Cancel the current orders for the selected units."),
            count: 0,
            enabled: true,
        }),
    );

    // call-to-arms
    add(
        PLAYER,
        any(&|entity| ATTACK_CLASSES.iter().any(|class| context.has_class(entity, class))).then(
            || Info {
                icon: "command-call-to-arms",
                tooltip: tip(
                    "Attack",
                    "Send the selected units on attack move to the specified location after dropping resources.",
                ),
                count: 0,
                enabled: true,
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
            icon: "command-garrison",
            tooltip: tip(
                "Garrison",
                "Order the selected units to garrison in a structure or another unit.",
            ),
            count: 0,
            enabled: true,
        }),
    );

    // occupy-turret
    add(
        PLAYER,
        any(&|entity| {
            context.has_component(entity, COMPONENT_TURRETABLE) && context.holder(entity) == 0
        })
        .then(|| Info {
            icon: "command-occupy-turret",
            tooltip: tip(
                "Occupy Turret",
                "Order the selected units to occupy a turret point.",
            ),
            count: 0,
            enabled: true,
        }),
    );

    // leave-turret
    add(
        PLAYER,
        any(&|entity| {
            context.has_component(entity, COMPONENT_TURRETABLE)
                && context.holder(entity) != 0
                && context.flags(entity) & Commands::EJECTABLE != 0
        })
        .then(|| Info {
            icon: "command-leave-turret",
            tooltip: tip(
                "Unload",
                "Order the selected units to leave their turret points.",
            ),
            count: 0,
            enabled: true,
        }),
    );

    // repair
    add(
        PLAYER,
        any(&|entity| context.has_component(entity, COMPONENT_BUILDER)).then(|| Info {
            icon: "repair",
            tooltip: tip(
                "Repair",
                "Order the selected units to repair a structure, ship, or siege engine.",
            ),
            count: 0,
            enabled: true,
        }),
    );

    // focus-rally
    add(
        PLAYER | OBSERVER,
        any(&|entity| context.has_component(entity, COMPONENT_RALLY_POINT)).then(|| Info {
            icon: "command-focus-rally",
            tooltip: tip(
                "Focus on Rally Point",
                "Center the view on the selected rally point.",
            ),
            count: 0,
            enabled: true,
        }),
    );

    // back-to-work
    add(
        PLAYER,
        any(&|entity| context.flags(entity) & Commands::WORK_ORDERS != 0).then(|| Info {
            icon: "command-back-to-work",
            tooltip: tip(
                "Back to Work",
                "Order the selected units to resume their work.",
            ),
            count: 0,
            enabled: true,
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
            icon: "command-add-guard",
            tooltip: tip(
                "Guard",
                "Order the selected units to guard a structure or unit.",
            ),
            count: 0,
            enabled: true,
        }),
    );
    add(
        PLAYER,
        any(&|entity| context.flags(entity) & Commands::GUARDING != 0).then(|| Info {
            icon: "command-remove-guard",
            tooltip: tip("Abort Guard", "Order the selected units to stop guarding."),
            count: 0,
            enabled: true,
        }),
    );

    // select-trading-goods
    add(
        PLAYER,
        any(&|entity| context.has_component(entity, COMPONENT_MARKET)).then(|| Info {
            icon: "command-economics",
            tooltip: tip(
                "Barter & Trade",
                "Open the dialog for managing resource trading and bartering.",
            ),
            count: 0,
            enabled: true,
        }),
    );

    // patrol
    add(
        PLAYER,
        any(&|entity| context.flags(entity) & Commands::CAN_PATROL != 0).then(|| Info {
            icon: "command-patrol",
            tooltip: tip(
                "Patrol",
                "Order to repeatedly go to a point and come back, attacking all enemies along the way.",
            ),
            count: 0,
            enabled: true,
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
    add(
        PLAYER,
        (!sharable.is_empty() && selected.iter().any(|entity| has_allies(entity.owner))).then(
            || {
                if sharable
                    .iter()
                    .any(|entity| context.flags(entity) & Commands::SHARED == 0)
                {
                    Info {
                        icon: "command-locked",
                        tooltip: tip(
                            "Share Dropsite",
                            "Allow allies to use this dropsite, now locked.",
                        ),
                        count: 0,
                        enabled: true,
                    }
                } else {
                    Info {
                        icon: "command-unlocked",
                        tooltip: tip(
                            "Lock Dropsite",
                            "Prevent allies from using this dropsite, now shared.",
                        ),
                        count: 0,
                        enabled: true,
                    }
                }
            },
        ),
    );
    add(
        ALLY | OBSERVER,
        (!sharable.is_empty() && !sharable.iter().all(|entity| context.controls(entity.owner)))
            .then(|| {
                if sharable
                    .iter()
                    .all(|entity| context.flags(entity) & Commands::SHARED != 0)
                {
                    Info {
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
                    }
                } else {
                    Info {
                        icon: "command-locked",
                        tooltip: tip("Locked Dropsite", "The use of this dropsite is prohibited."),
                        count: 0,
                        enabled: false,
                    }
                }
            }),
    );

    // autoqueue-on and autoqueue-off
    let autoqueue = |entity: &Selected| context.queue(entity).is_some_and(|queue| queue.autoqueue);
    add(
        PLAYER,
        any(&|entity| {
            context.has_component(entity, COMPONENT_TRAINER)
                && context.queue(entity).is_some()
                && !autoqueue(entity)
        })
        .then(|| Info {
            icon: "command-autoqueue-on",
            tooltip: tip(
                "Activate Auto-Queue",
                "Activate the production auto-queue for the selected structures.",
            ),
            count: 0,
            enabled: true,
        }),
    );
    add(
        PLAYER,
        any(&autoqueue).then(|| Info {
            icon: "command-autoqueue-off",
            tooltip: tip(
                "Deactivate Auto-Queue",
                "Deactivate the production auto-queue for the selected structures.",
            ),
            count: 0,
            enabled: true,
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
                    owner: dot.owner,
                    template: dot.template,
                    groups: entity_groups(scene, id, dot.template),
                })
            })
            .collect();
        let context = Context {
            scene,
            selected: &selected,
            player: None,
        };
        let mut infos = infos(&context);
        infos.truncate(MAX_COMMANDS);
        self.root.hidden(infos.is_empty());
        let total = infos.len() as f64;
        for (index, button) in self.buttons.iter_mut().enumerate() {
            let Some(info) = infos.get(index) else {
                button.button.hidden(true);
                continue;
            };
            button.button.style(format!(
                "left: calc(50% + {}px)",
                (index as f64 - total / 2.0) * COMMAND_STEP
            ));
            button.button.class(String::from(if info.enabled {
                "command"
            } else {
                "command disabled"
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
}
