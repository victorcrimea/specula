//! The chat lines the session GUI builds from the engine's typed
//! notifications (`gui/session/messages.js`, `chat/ChatMessageFormat*.js`)
//! and the time panel's texts (`TimeNotificationOverlay.js`), as pieces the
//! page turns into elements. Pure formatting: no DOM here.
//!
//! The engine's strings are msgids, so the baked catalogs translate them;
//! none of them is a Specula string of its own.

use js_sys::Array;
use js_sys::Object;
use js_sys::Reflect;
use wasm_bindgen::JsValue;

use crate::chat::Channel;
use crate::chat_window::Attack;
use crate::diplomacy_window::time_to_string;
use crate::lang;
use crate::panel::css_color;
use crate::tooltip;

/// `resources/` order, as `g_ResourceData.GetCodes()`.
const RESOURCES: [&str; 4] = ["food", "wood", "stone", "metal"];
/// The first stand-in for an argument's pieces; the nth is this plus n.
const MARK_BASE: u32 = 0xE010;
const WHITE: u32 = 0xFFFF_FFFF;

pub enum Piece {
    Text(String),
    Name {
        text: String,
        color: String,
    },
    /// The CSS variable of the icon's texture.
    Icon(&'static str),
}

/// What the history filters keep: the engine's game and network
/// notification classes, or a player's message on a channel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Game,
    Network,
    Chat(Channel),
}

pub struct Notice {
    pub pieces: Vec<Piece>,
    pub filter: Filter,
    /// Where a click on the line takes the camera.
    pub focus: Option<Attack>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FlareSetting {
    Never,
    Observer,
    Always,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PhaseSetting {
    None,
    Completed,
    All,
}

/// `gui.session.notifications.*` other than the attack's.
#[derive(Clone, Copy)]
pub struct Settings {
    pub tribute: bool,
    pub barter: bool,
    pub flare: FlareSetting,
    pub phase: PhaseSetting,
}

pub struct Context<'a> {
    /// The seat's player; none for an observer.
    pub own: Option<u8>,
    /// `g_IsObserver`: the `/` page and an unseated `/play`.
    pub observer: bool,
    /// By player index, gaia first.
    pub names: Vec<&'a str>,
    pub palette: &'a [u32],
    pub settings: Settings,
}

fn get(object: &JsValue, key: &str) -> JsValue {
    Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn text(object: &JsValue, key: &str) -> Option<String> {
    get(object, key).as_string()
}

fn number(object: &JsValue, key: &str) -> Option<f64> {
    get(object, key).as_f64()
}

fn first_player(object: &JsValue) -> Option<i64> {
    array(&get(object, "players"))?
        .get(0)
        .as_f64()
        .map(|player| player as i64)
}

fn array(value: &JsValue) -> Option<Array> {
    Array::is_array(value).then(|| Array::from(value))
}

/// A number the way `sprintf("%s")` prints it: whole ones without a
/// fraction.
fn shown_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// `sprintf` over pieces: each `%(key)s` of the translated template is
/// replaced by that argument's pieces. A private-use character stands for
/// the argument while `lang::format` runs, so a name that holds `%(` cannot
/// be read as a placeholder.
fn format(template: &str, args: &[(&str, Vec<Piece>)]) -> Vec<Piece> {
    let marks: Vec<String> = (0..args.len() as u32)
        .filter_map(|index| char::from_u32(MARK_BASE + index))
        .map(String::from)
        .collect();
    let values: Vec<(&str, &str)> = args
        .iter()
        .zip(&marks)
        .map(|((key, _), mark)| (*key, mark.as_str()))
        .collect();
    let shown = lang::format(template, &values);
    let mut out = Vec::new();
    let mut pending = String::new();
    let flush = |pending: &mut String, out: &mut Vec<Piece>| {
        if !pending.is_empty() {
            out.push(Piece::Text(std::mem::take(pending)));
        }
    };
    for c in shown.chars() {
        let index = (c as u32)
            .checked_sub(MARK_BASE)
            .map(|index| index as usize);
        let Some(arg) = index.and_then(|index| args.get(index)) else {
            pending.push(c);
            continue;
        };
        flush(&mut pending, &mut out);
        out.extend(arg.1.iter().map(copy));
    }
    flush(&mut pending, &mut out);
    out
}

fn copy(piece: &Piece) -> Piece {
    match piece {
        Piece::Text(text) => Piece::Text(text.clone()),
        Piece::Name { text, color } => Piece::Name {
            text: text.clone(),
            color: color.clone(),
        },
        Piece::Icon(name) => Piece::Icon(name),
    }
}

fn plain(text: impl Into<String>) -> Vec<Piece> {
    vec![Piece::Text(text.into())]
}

fn name(text: &str, color: u32) -> Vec<Piece> {
    vec![Piece::Name {
        text: text.to_string(),
        color: css_color(color, 1.0),
    }]
}

/// `colorizePlayernameByID`: the player's name in its colour, "Unknown
/// Player" in white when it has none.
fn player(ctx: &Context, id: i64) -> Vec<Piece> {
    let index = usize::try_from(id).ok();
    let shown = index
        .and_then(|index| ctx.names.get(index))
        .filter(|shown| !shown.is_empty());
    let color = index
        .and_then(|index| ctx.palette.get(index))
        .copied()
        .unwrap_or(WHITE);
    match shown {
        Some(shown) => name(shown, color),
        None => name(&lang::translate("Unknown Player"), color),
    }
}

/// `getLocalizedResourceAmounts`.
fn amounts(resources: &JsValue) -> String {
    let mut parts: Vec<String> = RESOURCES
        .iter()
        .filter_map(|code| {
            let amount = number(resources, code).filter(|amount| *amount != 0.0)?;
            let kind = lang::translate_context(
                "withinSentence",
                tooltip::resource_data(code).map_or(code, |(name, _)| name),
            );
            Some(lang::format(
                &lang::translate("%(amount)s %(resourceType)s"),
                &[("amount", &shown_number(amount)), ("resourceType", &kind)],
            ))
        })
        .collect();
    if parts.len() < 2 {
        return parts.join("");
    }
    let last = parts.pop().unwrap_or_default();
    lang::format(
        &lang::translate("%(previousAmounts)s and %(lastAmount)s"),
        &[
            ("previousAmounts", &parts.join(&lang::translate(", "))),
            ("lastAmount", &last),
        ],
    )
}

/// The message's `parameters` as pieces: `translateObjectKeys` for the keys
/// named by `translate_keys` (an array, or an object of contexts), then
/// `colorizePlayernameParameters`.
fn parameters(
    params: &JsValue,
    translate_keys: &JsValue,
    ctx: &Context,
) -> Vec<(String, Vec<Piece>)> {
    if !params.is_object() {
        return Vec::new();
    }
    let context_of = |key: &str| -> Option<String> {
        if let Some(list) = array(translate_keys) {
            return list
                .iter()
                .any(|item| item.as_string().as_deref() == Some(key))
                .then(String::new);
        }
        if translate_keys.is_object() {
            return get(translate_keys, key)
                .as_string()
                .or_else(|| (!get(translate_keys, key).is_undefined()).then(String::new));
        }
        None
    };
    Object::keys(&Object::from(params.clone()))
        .iter()
        .filter_map(|key| key.as_string())
        .map(|key| {
            let value = get(params, &key);
            let pieces = if key.starts_with("_player_") {
                player(ctx, value.as_f64().map_or(-1, |id| id as i64))
            } else if let Some(shown) = value.as_string() {
                match context_of(&key) {
                    Some(context) if context.is_empty() => plain(lang::translate(&shown)),
                    Some(context) => plain(lang::translate_context(&context, &shown)),
                    None => plain(shown),
                }
            } else if let Some(value) = value.as_f64() {
                plain(shown_number(value))
            } else {
                plain("")
            };
            (key, pieces)
        })
        .collect()
}

/// `format` with arguments that own their keys.
fn format_owned(template: &str, args: &[(String, Vec<Piece>)]) -> Vec<Piece> {
    let borrowed: Vec<(&str, Vec<Piece>)> = args
        .iter()
        .map(|(key, pieces)| (key.as_str(), pieces.iter().map(copy).collect()))
        .collect();
    format(template, &borrowed)
}

fn game(pieces: Vec<Piece>) -> Option<Notice> {
    Some(Notice {
        pieces,
        filter: Filter::Game,
        focus: None,
    })
}

/// The notice of one event the feed carried (kind 7), None for what the
/// viewer's settings or role leave out.
pub fn event(json: &JsValue, ctx: &Context) -> Option<Notice> {
    let kind = text(json, "type")?;
    let source = first_player(json)?;
    match kind.as_str() {
        "defeat" | "won" => finished(json, ctx),
        "phase" => phase(json, source, ctx),
        "tribute" => {
            let donator = number(json, "donator")? as i64;
            let own = ctx.own.map(i64::from);
            let template = if Some(source) == own {
                "%(player)s has sent you %(amounts)s."
            } else if Some(donator) == own {
                "You have sent %(player2)s %(amounts)s."
            } else if ctx.settings.tribute {
                "%(player)s has sent %(player2)s %(amounts)s."
            } else {
                return None;
            };
            game(format(
                &lang::translate(template),
                &[
                    ("player", player(ctx, donator)),
                    ("player2", player(ctx, source)),
                    ("amounts", plain(amounts(&get(json, "amounts")))),
                ],
            ))
        }
        "barter" => {
            if !ctx.observer || !ctx.settings.barter {
                return None;
            }
            let one = |resource: &str, amount: &str| {
                let object = Object::new();
                if let Some(code) = text(json, resource) {
                    let _ = Reflect::set(&object, &code.into(), &get(json, amount));
                }
                plain(amounts(&object))
            };
            game(format(
                &lang::translate("%(player)s bartered %(amountGiven)s for %(amountGained)s."),
                &[
                    ("player", player(ctx, source)),
                    ("amountGiven", one("resourceGiven", "amountGiven")),
                    ("amountGained", one("resourceGained", "amountGained")),
                ],
            ))
        }
        "diplomacy" => diplomacy(json, source, ctx),
        "aichat" => aichat(json, source, ctx),
        "network" => network(json, ctx),
        _ => None,
    }
}

/// `ChatMessageFormatSimulation.playerstate`.
fn finished(json: &JsValue, ctx: &Context) -> Option<Notice> {
    let allies: Vec<i64> = array(&get(json, "allies"))?
        .iter()
        .filter_map(|ally| ally.as_f64())
        .map(|ally| ally as i64)
        .collect();
    let message = get(json, "message");
    if let Some(message) = message.as_string() {
        return game(format(
            &lang::translate(&message),
            &[("player", player(ctx, *allies.first()?))],
        ));
    }
    let singular = text(&message, "message")?;
    let plural = text(&message, "pluralMessage")?;
    let count = number(&message, "pluralCount")?;
    let (last, rest) = allies.split_last()?;
    let mut listed = Vec::new();
    for (index, ally) in rest.iter().enumerate() {
        if index > 0 {
            listed.push(Piece::Text(lang::translate(", ")));
        }
        listed.extend(player(ctx, *ally));
    }
    game(format(
        &lang::translate_plural(&singular, &plural, count),
        &[("players", listed), ("lastPlayer", player(ctx, *last))],
    ))
}

/// `ChatMessageFormatSimulation.phase`.
fn phase(json: &JsValue, source: i64, ctx: &Context) -> Option<Notice> {
    let state = text(json, "phaseState")?;
    let template = match (state.as_str(), ctx.settings.phase) {
        (_, PhaseSetting::None) => return None,
        ("completed", _) => "%(player)s has reached the %(phaseName)s.",
        ("started", PhaseSetting::All) => "%(player)s is advancing to the %(phaseName)s.",
        ("aborted", PhaseSetting::All) => "The %(phaseName)s of %(player)s has been aborted.",
        _ => return None,
    };
    let names = get(json, "phaseNames");
    let generic = text(&names, "generic").map(|generic| lang::translate(&generic));
    let specific = text(&names, "specific").map(|specific| lang::translate(&specific));
    let phase_name = tooltip::entity_names(
        generic.as_deref().unwrap_or_default(),
        specific.as_deref().unwrap_or_default(),
    );
    game(format(
        &lang::translate(template),
        &[
            ("player", player(ctx, source)),
            ("phaseName", plain(phase_name)),
        ],
    ))
}

/// `ChatMessageFormatSimulation.diplomacy`.
fn diplomacy(json: &JsValue, source: i64, ctx: &Context) -> Option<Notice> {
    let target = number(json, "targetPlayer")? as i64;
    let status = text(json, "status")?;
    let own = ctx.own.map(i64::from);
    let (kind, active) = if ctx.observer {
        ("observer", false)
    } else if own == Some(source) {
        ("active", true)
    } else if own == Some(target) {
        ("passive", false)
    } else {
        return None;
    };
    let template = match (kind, status.as_str()) {
        ("active", "ally") => "You are now allied with %(player)s.",
        ("active", "enemy") => "You are now at war with %(player)s.",
        ("active", "neutral") => "You are now neutral with %(player)s.",
        ("passive", "ally") => "%(player)s is now allied with you.",
        ("passive", "enemy") => "%(player)s is now at war with you.",
        ("passive", "neutral") => "%(player)s is now neutral with you.",
        ("observer", "ally") => "%(player)s is now allied with %(player2)s.",
        ("observer", "enemy") => "%(player)s is now at war with %(player2)s.",
        ("observer", "neutral") => "%(player)s is now neutral with %(player2)s.",
        _ => return None,
    };
    let (first, second) = if active {
        (target, source)
    } else {
        (source, target)
    };
    game(format(
        &lang::translate(template),
        &[
            ("player", player(ctx, first)),
            ("player2", player(ctx, second)),
        ],
    ))
}

/// `ChatMessageFormatPlayer.parse` for a line the AI sent; the server has
/// resolved the addressee into `channel`.
fn aichat(json: &JsValue, sender: i64, ctx: &Context) -> Option<Notice> {
    let message = text(json, "message")?;
    if message.is_empty() {
        return None;
    }
    let channel = match text(json, "channel")?.as_str() {
        "allies" => Channel::Allies,
        "enemies" => Channel::Enemies,
        "observers" => Channel::Observers,
        "private" => Channel::Private,
        _ => Channel::Everyone,
    };
    let shown = lang::translate(&message);
    let body = if get(json, "translateParameters").is_undefined() {
        plain(shown)
    } else {
        format_owned(
            &shown,
            &parameters(
                &get(json, "parameters"),
                &get(json, "translateParameters"),
                ctx,
            ),
        )
    };
    let context = match channel {
        Channel::Everyone => None,
        Channel::Allies => Some("Ally"),
        Channel::Enemies => Some("Enemy"),
        Channel::Observers => Some("Observer"),
        Channel::Private => Some("Private"),
    };
    let user_tag = format(
        &lang::translate("<%(user)s>"),
        &[("user", player(ctx, sender))],
    );
    let mut args = vec![("userTag", user_tag), ("message", body)];
    let template = match context {
        Some(context) => {
            args.push((
                "context",
                plain(lang::translate_context("chat message context", context)),
            ));
            "(%(context)s) %(userTag)s %(message)s"
        }
        None => "%(userTag)s %(message)s",
    };
    Some(Notice {
        pieces: format(&lang::translate(template), &args),
        filter: Filter::Chat(channel),
        focus: None,
    })
}

/// `ChatMessageFormatNetwork`: a seat that left or rejoined, a kick or ban.
fn network(json: &JsValue, ctx: &Context) -> Option<Notice> {
    let event = text(json, "event")?;
    let who = number(json, "player").map(|player| player as i64);
    let pieces = match event.as_str() {
        "left" | "rejoined" => {
            let template = if event == "left" {
                "%(player)s has left the game."
            } else {
                "%(player)s has rejoined the game."
            };
            format(&lang::translate(template), &[("player", player(ctx, who?))])
        }
        "kicked" | "banned" => {
            let template = if event == "banned" {
                "%(username)s has been banned"
            } else {
                "%(username)s has been kicked"
            };
            let shown = text(json, "name").filter(|shown| !shown.is_empty());
            let shown = shown.unwrap_or_else(|| lang::translate("Unknown Player"));
            // `colorizePlayernameHelper`: the player's colour when the name
            // is a player's, white otherwise.
            let color = who
                .and_then(|who| usize::try_from(who).ok())
                .and_then(|who| ctx.palette.get(who))
                .copied()
                .unwrap_or(WHITE);
            format(
                &lang::translate(template),
                &[("username", name(&shown, color))],
            )
        }
        _ => return None,
    };
    // The engine's kick message has no guid, so its history files it with
    // the game notifications; the others have one.
    let filter = if matches!(event.as_str(), "kicked" | "banned") {
        Filter::Game
    } else {
        Filter::Network
    };
    Some(Notice {
        pieces,
        filter,
        focus: None,
    })
}

/// `ChatMessageFormatSimulation.flare`: a flare's chat line, which a click
/// follows to the flare's place. `name` is an observer's chat nickname.
pub fn flare(
    sender: u8,
    name_of_observer: Option<&str>,
    x: f32,
    z: f32,
    ctx: &Context,
) -> Option<Notice> {
    match ctx.settings.flare {
        FlareSetting::Never => return None,
        FlareSetting::Observer if !ctx.observer => return None,
        _ => {}
    }
    let who = if sender != 0 {
        player(ctx, i64::from(sender))
    } else {
        let shown = name_of_observer
            .filter(|shown| !shown.is_empty())
            .map_or_else(|| lang::translate("Unknown Player"), str::to_string);
        name(&shown, WHITE)
    };
    Some(Notice {
        pieces: format(
            &lang::translate("%(icon)s%(player)s has sent a flare."),
            &[
                ("icon", vec![Piece::Icon("--gui-icon-focusflare")]),
                ("player", who),
            ],
        ),
        filter: Filter::Game,
        focus: Some(Attack {
            target: 0,
            unit: false,
            x,
            z,
        }),
    })
}

/// `TimeNotificationOverlay.rebuild` for one note: the engine's text with
/// its parameters, and the time left.
pub fn time_line(note: &JsValue, remaining_ms: f64, ctx: &Context) -> Vec<Piece> {
    let Some(message) = text(note, "message") else {
        return Vec::new();
    };
    let message = if get(note, "translateMessage").is_truthy() {
        lang::translate(&message)
    } else {
        message
    };
    let mut args = parameters(
        &get(note, "parameters"),
        &get(note, "translateParameters"),
        ctx,
    );
    args.retain(|(key, _)| key != "time");
    args.push(("time".to_string(), plain(time_to_string(remaining_ms))));
    format_owned(&message, &args)
}
