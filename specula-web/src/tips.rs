//! The tooltips that read template and technology data: the JSON of
//! `GetTemplateData` and of the session GUI's `GetTechnologyData` that the
//! detail stream's data table names (`detail::DataKey`), fetched from
//! `/data/<hash>` when first needed, and a port of the parts of
//! gui/common/tooltips.js and gui/session/selection_panels.js that turn it
//! into the engine's markup. English only: `translate` is the identity.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use crate::lang::translate;
use crate::lang::translate_context;
use crate::lang::translate_plural;
use crate::tooltip::body_font;
use crate::tooltip::header_font;
use crate::tooltip::objection_font;
use crate::tooltip::resource_icon;
use crate::tooltip::tags;
use crate::tooltip::unit_font;

/// tooltips.js' `g_Indent`.
const INDENT: &str = "    ";
/// `g_ResourceData.GetCodes()` for the public mod, by `order`.
pub(crate) const RESOURCES: [&str; 4] = ["food", "wood", "stone", "metal"];
/// Each resource's `subtypes` in simulation/data/resources/, ruins left
/// out as `getGatherTooltip` does.
const SUBTYPES: [(&str, &[&str]); 4] = [
    ("food", &["fish", "fruit", "grain", "meat"]),
    ("wood", &["tree"]),
    ("stone", &["rock"]),
    ("metal", &["ore"]),
];
/// simulation/data/damage_types/: code, name, order; `sort` puts types
/// without metadata last.
const DAMAGE_TYPES: [(&str, &str, u32); 5] = [
    ("Hack", "Hack", 1),
    ("Pierce", "Pierce", 2),
    ("Crush", "Crush", 3),
    ("Fire", "Fire", 4),
    ("Poison", "Poison", 5),
];
/// simulation/data/status_effects/: code, statusName, applierTooltip.
const STATUS_EFFECTS: [(&str, &str, &str); 2] = [
    ("Burning", "Burning", "This unit causes fire damage."),
    ("Poisoned", "Poisoned", "This unit causes poison damage."),
];

enum File {
    Loading,
    Ready(JsValue),
    Failed,
}

/// The data table's JSON files by hash, fetched on first use.
#[derive(Clone, Default)]
pub struct DataFiles {
    files: Rc<RefCell<HashMap<u64, File>>>,
    loaded: Rc<Cell<bool>>,
}

impl DataFiles {
    /// The parsed JSON of `hash`, `None` until it has loaded.
    pub fn get(&self, hash: u64) -> Option<JsValue> {
        if let Some(file) = self.files.borrow().get(&hash) {
            return match file {
                File::Ready(value) => Some(value.clone()),
                File::Loading | File::Failed => None,
            };
        }
        self.files.borrow_mut().insert(hash, File::Loading);
        let files = Rc::clone(&self.files);
        let loaded = Rc::clone(&self.loaded);
        wasm_bindgen_futures::spawn_local(async move {
            let file = match fetch_json(hash).await {
                Some(value) => File::Ready(value),
                None => File::Failed,
            };
            files.borrow_mut().insert(hash, file);
            loaded.set(true);
        });
        None
    }

    /// Whether a file arrived since the last call, so the tooltips that
    /// waited for it are made again.
    pub fn take_loaded(&self) -> bool {
        self.loaded.replace(false)
    }
}

async fn fetch_json(hash: u64) -> Option<JsValue> {
    let window = web_sys::window()?;
    // A seat gets only the files its own stream named.
    let base = crate::match_base();
    let prefix = if crate::seated() { "/play" } else { "" };
    let response: web_sys::Response =
        JsFuture::from(window.fetch_with_str(&format!("{base}{prefix}/data/{hash:016x}")))
            .await
            .ok()?
            .dyn_into()
            .ok()?;
    if !response.ok() {
        return None;
    }
    let text = JsFuture::from(response.text().ok()?).await.ok()?;
    js_sys::JSON::parse(&text.as_string()?).ok()
}

/// A member of a JSON object, undefined when it is not there.
pub fn field(value: &JsValue, key: &str) -> JsValue {
    if !value.is_object() {
        return JsValue::UNDEFINED;
    }
    js_sys::Reflect::get(value, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

/// A text member of a JSON object, empty when it is not there.
pub fn text_field(value: &JsValue, key: &str) -> String {
    text(&field(value, key))
}

/// A number member of a JSON object, 0 when it is not there.
pub fn number_field(value: &JsValue, key: &str) -> f64 {
    plus(&field(value, key))
}

/// `+value`, 0 for what is not a number.
fn plus(value: &JsValue) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_string().and_then(|text| text.trim().parse().ok()))
        .unwrap_or(0.0)
}

fn text(value: &JsValue) -> String {
    value.as_string().unwrap_or_default()
}

/// A string the engine runs through `translate` when it loads the data
/// (`translateObjectKeys`), or when it shows it.
fn translated(value: &JsValue) -> String {
    translate(&text(value))
}

fn keys(value: &JsValue) -> Vec<String> {
    if !value.is_object() {
        return Vec::new();
    }
    js_sys::Object::keys(value.unchecked_ref::<js_sys::Object>())
        .iter()
        .filter_map(|key| key.as_string())
        .collect()
}

fn items(value: &JsValue) -> Vec<JsValue> {
    if !js_sys::Array::is_array(value) {
        return Vec::new();
    }
    js_sys::Array::from(value).iter().collect()
}

/// JavaScript's `Math.round`, which rounds halves up.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// JavaScript's `Number.prototype.toFixed`.
fn to_fixed(value: f64, digits: usize) -> String {
    let scale = 10f64.powi(digits as i32);
    format!("{:.*}", digits, js_round(value * scale) / scale)
}

/// A number as JavaScript prints it, for the plain values a template has.
fn js_number(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e21 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// `+value.toFixed(digits)`, printed.
fn rounded(value: f64, digits: usize) -> String {
    js_number(to_fixed(value, digits).parse().unwrap_or(value))
}

fn comma_font(text: &str) -> String {
    tags(text, &[("font", "sans-12")])
}

/// `getSecondsString`.
fn seconds_string(seconds: f64) -> String {
    format!(
        "{} {}",
        js_number(seconds),
        unit_font(&translate_plural("second", "seconds", seconds))
    )
}

fn damage_name(code: &str) -> String {
    let name = DAMAGE_TYPES
        .iter()
        .find(|(known, _, _)| *known == code)
        .map_or(code, |(_, name, _)| name);
    translate_context("damage type", name)
}

/// `g_DamageTypesMetadata.sort`.
fn sort_damage_types(mut codes: Vec<String>) -> Vec<String> {
    let order = |code: &str| {
        DAMAGE_TYPES
            .iter()
            .find(|(known, _, _)| *known == code)
            .map_or(u32::MAX, |(_, _, order)| *order)
    };
    codes.sort_by_key(|code| order(code));
    codes
}

fn status_name(code: &str) -> String {
    let name = STATUS_EFFECTS
        .iter()
        .find(|(known, _, _)| *known == code)
        .map_or(code, |(_, name, _)| name);
    translate_context("status effect", name)
}

/// `getEntityNames` at the default gui.session.howtoshownames 0.
pub fn entity_names(template: &JsValue) -> String {
    let name = field(template, "name");
    crate::tooltip::entity_names(
        &translated(&field(&name, "generic")),
        &translated(&field(&name, "specific")),
    )
}

/// The category of a template's `trainingRestrictions` or
/// `buildRestrictions`, when it has them.
pub fn restriction_category(template: &JsValue, key: &str) -> Option<String> {
    let restrictions = field(template, key);
    restrictions
        .is_object()
        .then(|| text(&field(&restrictions, "category")))
}

/// A template with an empty `cost`, for a tooltip that gives its own costs.
pub fn empty_cost() -> JsValue {
    let template = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&template, &"cost".into(), &js_sys::Object::new());
    template.into()
}

/// A template's translated generic and specific names.
pub fn name_parts(template: &JsValue) -> (String, String) {
    let name = field(template, "name");
    (
        translated(&field(&name, "generic")),
        translated(&field(&name, "specific")),
    )
}

/// `getEntityNamesFormatted`.
pub fn entity_names_formatted(template: &JsValue) -> String {
    let name = field(template, "name");
    let generic = translated(&field(&name, "generic"));
    let specific = translated(&field(&name, "specific"));
    if specific.is_empty() {
        return tags(&generic, &[("font", "sans-bold-16")]);
    }
    let mut chars = specific.chars();
    let first = chars.next().map(String::from).unwrap_or_default();
    let primary = tags(&first, &[("font", "sans-bold-16")])
        + &tags(&chars.as_str().to_uppercase(), &[("font", "sans-bold-12")]);
    if generic.is_empty() || generic == specific {
        return primary;
    }
    format!(
        "{primary} ({})",
        tags(&generic, &[("font", "sans-bold-16")])
    )
}

/// `getEntityTooltip`.
pub fn entity_tooltip(template: &JsValue) -> String {
    let tooltip = translated(&field(template, "tooltip"));
    if tooltip.is_empty() {
        String::new()
    } else {
        body_font(&tooltip)
    }
}

/// `getVisibleEntityClassesFormatted`.
pub fn visible_classes(template: &JsValue) -> String {
    let classes: Vec<String> = items(&field(template, "visibleIdentityClasses"))
        .iter()
        .map(translated)
        .collect();
    if classes.is_empty() {
        return String::new();
    }
    format!(
        "{} {}",
        header_font(&translate("Classes:")),
        body_font(&classes.join(&translate(", ")))
    )
}

/// `getAurasTooltip`, without a wall set's, whose long wall's data is not
/// sent.
pub fn auras_tooltip(template: &JsValue) -> String {
    let auras = field(template, "auras");
    keys(&auras)
        .iter()
        .map(|id| {
            let aura = field(&auras, id);
            let mut tooltip = format!(
                "{} {}",
                header_font(&format!("{}:", entity_names(&aura))),
                body_font(&translated(&field(&aura, "description")))
            );
            let radius = plus(&field(&aura, "radius"));
            if radius != 0.0 {
                tooltip.push_str(&format!(
                    " Range: {} {}",
                    js_number(radius),
                    unit_font(&translate_plural("meter", "meters", radius))
                ));
            }
            tooltip
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `getTreasureTooltip`.
pub fn treasure_tooltip(template: &JsValue) -> String {
    let resources = field(&field(template, "treasure"), "resources");
    let rewards: Vec<String> = RESOURCES
        .iter()
        .filter_map(|code| {
            let reward = field(&resources, code);
            reward
                .is_truthy()
                .then(|| format!("{} {}", resource_icon(code), js_number(plus(&reward))))
        })
        .collect();
    if rewards.is_empty() {
        return String::new();
    }
    format!(
        "{} {}",
        header_font(&translate("Reward:")),
        rewards.join("  ")
    )
}

/// `getHealthTooltip`.
pub fn health_tooltip(template: &JsValue) -> String {
    let health = field(template, "health");
    if !health.is_truthy() {
        return String::new();
    }
    format!(
        "{} {}",
        header_font(&translate("Health:")),
        js_number(plus(&health))
    )
}

/// `resistanceLevelToPercentageString`.
fn resistance_percentage(level: f64) -> String {
    format!(
        "{}%",
        js_number(100.0 - js_round(0.9f64.powf(level) * 100.0))
    )
}

fn resistance_part(level: f64, name: &str) -> String {
    format!(
        "{} {} [font=\"sans-10\"]({})[/font]",
        to_fixed(level, 1),
        unit_font(name),
        resistance_percentage(level)
    )
}

/// `getResistanceTooltip`.
pub fn resistance_tooltip(template: &JsValue) -> String {
    let resistance = field(template, "resistance");
    if !resistance.is_truthy() {
        return String::new();
    }
    let mut details = Vec::new();
    let damage = field(&resistance, "Damage");
    if damage.is_truthy() {
        details.push(format!(
            "{} {}",
            header_font(&translate("Damage:")),
            sort_damage_types(keys(&damage))
                .iter()
                .map(|code| resistance_part(plus(&field(&damage, code)), &damage_name(code)))
                .collect::<Vec<_>>()
                .join(&comma_font(", "))
        ));
    }
    let capture = field(&resistance, "Capture");
    if capture.is_truthy() {
        details.push(format!(
            "{} {}",
            header_font(&translate("Capture:")),
            resistance_part(plus(&capture), "Capture")
        ));
    }
    let status = field(&resistance, "ApplyStatus");
    if status.is_truthy() {
        details.push(format!(
            "{} {}",
            header_font(&translate("Status Effects:")),
            keys(&status)
                .iter()
                .map(|code| {
                    let effect = field(&status, code);
                    let block = plus(&field(&effect, "blockChance"));
                    let duration = plus(&field(&effect, "duration"));
                    let name = unit_font(&status_name(code));
                    let reduction = js_number(100.0 - duration * 100.0);
                    if block == 1.0 {
                        format!("Blocks {name}")
                    } else if block == 0.0 {
                        format!("{name} Duration reduction: {reduction}%")
                    } else if duration == 1.0 {
                        format!("{name} Blocks: {}%", js_number(block * 100.0))
                    } else {
                        format!(
                            "{name} Blocks: {}%, Duration reduction: {reduction}%",
                            js_number(block * 100.0)
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join(&comma_font(", "))
        ));
    }
    if details.is_empty() {
        return String::new();
    }
    format!(
        "{}\n{INDENT}{}",
        header_font(&translate("Resistance:")),
        details.join(&format!("\n{INDENT}"))
    )
}

/// `attackRateDetails`.
fn attack_rate_details(interval: f64, projectiles: Option<f64>) -> String {
    if interval == 0.0 {
        return String::new();
    }
    if projectiles == Some(0.0) {
        return "Garrison to fire arrows".to_string();
    }
    let mut rate = seconds_string(interval / 1000.0);
    let mut header = header_font(&translate("Interval:"));
    if let Some(count) = projectiles.filter(|count| *count > 1.0) {
        header = header_font(&translate("Rate:"));
        rate = format!(
            "{} {} / {rate}",
            js_number(count),
            unit_font(&translate_plural("arrow", "arrows", count))
        );
    }
    format!("{header} {rate}")
}

/// `rangeDetails`.
fn range_details(attack: &JsValue) -> String {
    if !field(attack, "maxRange").is_truthy() {
        return String::new();
    }
    let min = js_round(plus(&field(attack, "minRange")));
    let max = js_round(plus(&field(attack, "maxRange")));
    let real = field(attack, "elevationAdaptedRange");
    let relative = if real.is_truthy() {
        js_round(plus(&real) - max)
    } else {
        0.0
    };
    let label = header_font(&translate("Range:"));
    let unit = unit_font(&if min != 0.0 || relative != 0.0 {
        translate_plural("meter", "meters", 2.0)
    } else {
        translate_plural("meter", "meters", max)
    });
    let relative_text = if relative > 0.0 {
        format!("+{}", js_number(relative))
    } else {
        js_number(relative)
    };
    match (relative != 0.0, min != 0.0) {
        (true, true) => format!(
            "{label} {} to {} ({relative_text}) {unit}",
            js_number(min),
            js_number(max)
        ),
        (true, false) => format!("{label} {} ({relative_text}) {unit}", js_number(max)),
        (false, true) => format!("{label} {} to {} {unit}", js_number(min), js_number(max)),
        (false, false) => format!("{label} {} {unit}", js_number(max)),
    }
}

/// `damageDetails`.
fn damage_details(damage: &JsValue) -> String {
    if !damage.is_truthy() {
        return String::new();
    }
    let codes: Vec<String> = keys(damage)
        .into_iter()
        .filter(|code| field(damage, code).is_truthy())
        .collect();
    sort_damage_types(codes)
        .iter()
        .map(|code| {
            format!(
                "{} {}",
                to_fixed(plus(&field(damage, code)), 1),
                unit_font(&damage_name(code))
            )
        })
        .collect::<Vec<_>>()
        .join(&comma_font(", "))
}

/// `captureDetails`.
fn capture_details(capture: &JsValue) -> String {
    if !capture.is_truthy() {
        return String::new();
    }
    format!("{} {}", to_fixed(plus(capture), 1), unit_font("Capture"))
}

/// `applyStatusDetails`.
fn apply_status_details(status: &JsValue) -> String {
    if !status.is_truthy() {
        return String::new();
    }
    format!(
        "gives {}",
        keys(status)
            .iter()
            .map(|code| unit_font(&status_name(code)))
            .collect::<Vec<_>>()
            .join(&comma_font(", "))
    )
}

/// `attackEffectsDetails`.
fn attack_effects_details(attack: &JsValue) -> String {
    [
        capture_details(&field(attack, "Capture")),
        damage_details(&field(attack, "Damage")),
        apply_status_details(&field(attack, "ApplyStatus")),
    ]
    .into_iter()
    .filter(|effect| !effect.is_empty())
    .collect::<Vec<_>>()
    .join(&comma_font(", "))
}

/// `splashDetails`.
fn splash_details(splash: &JsValue) -> String {
    let mut tooltip = format!(
        "{}: {}",
        header_font(&format!("{} Splash", text(&field(splash, "shape")))),
        attack_effects_details(splash)
    );
    if field(splash, "friendlyFire").is_truthy() {
        tooltip.push_str(&comma_font(", "));
        tooltip.push_str("Friendly Fire: Yes");
    }
    tooltip
}

/// `getStatusEffectsTooltip` for an applier.
fn status_effects_tooltip(code: &str, effect: &JsValue) -> String {
    let mut attributes = Vec::new();
    if field(effect, "Damage").is_truthy() || field(effect, "Capture").is_truthy() {
        attributes.push(attack_effects_details(effect));
    }
    let interval = field(effect, "Interval");
    if interval.is_truthy() {
        attributes.push(attack_rate_details(plus(&interval), None));
    }
    let duration = field(effect, "Duration");
    if duration.is_truthy() {
        let elapsed = field(effect, "_timeElapsed");
        let left = if elapsed.is_truthy() {
            plus(&duration) - plus(&elapsed)
        } else {
            plus(&duration)
        };
        attributes.push(format!(
            "{}: {}",
            header_font(&translate("Duration")),
            seconds_string(left / 1000.0)
        ));
    }
    if let Some((_, _, applier)) = STATUS_EFFECTS.iter().find(|(known, _, _)| *known == code) {
        attributes.push((*applier).to_string());
    }
    let stackability = match text(&field(effect, "Stackability")).as_str() {
        "Extend" => "(extends)",
        "Replace" => "(replaces)",
        "Stack" => "(stacks)",
        _ => "",
    };
    format!(
        "{}: {} {stackability}",
        header_font(&status_name(code)),
        attributes.join(&comma_font(", "))
    )
}

/// `getAttackTooltip`.
pub fn attack_tooltip(template: &JsValue) -> String {
    let attacks = field(template, "attack");
    if !attacks.is_truthy() {
        return String::new();
    }
    let building_ai = field(template, "buildingAI");
    let projectiles = building_ai.is_truthy().then(|| {
        let count = plus(&field(&building_ai, "arrowCount"));
        if count != 0.0 {
            count
        } else {
            plus(&field(&building_ai, "defaultArrowCount"))
        }
    });
    let mut tooltips = Vec::new();
    for kind in keys(&attacks) {
        // Slaughter is used to kill animals, so do not show it.
        if kind == "Slaughter" {
            continue;
        }
        let attack = field(&attacks, &kind);
        let label = header_font(&text(&field(&field(&attack, "attackName"), "name")));
        let status = field(&attack, "ApplyStatus");
        let status_details: String = keys(&status)
            .iter()
            .map(|code| {
                format!(
                    "\n{INDENT}{INDENT}{}",
                    status_effects_tooltip(code, &field(&status, code))
                )
            })
            .collect();
        let splash = field(&attack, "splash");
        let splash_text = if splash.is_truthy() {
            format!("\n{INDENT}{INDENT}{}", splash_details(&splash))
        } else {
            String::new()
        };
        tooltips.push(format!(
            "{label}: {}, {}, {}{status_details}{splash_text}",
            attack_effects_details(&attack),
            range_details(&attack),
            attack_rate_details(plus(&field(&attack, "repeatTime")), projectiles)
        ));
    }
    format!(
        "{}\n{INDENT}{}",
        header_font(&translate("Attack:")),
        tooltips.join(&format!("\n{INDENT}"))
    )
}

/// `getHealerTooltip`.
pub fn healer_tooltip(template: &JsValue) -> String {
    let heal = field(template, "heal");
    if !heal.is_truthy() {
        return String::new();
    }
    let health: f64 = to_fixed(plus(&field(&heal, "health")), 1)
        .parse()
        .unwrap_or(0.0);
    let range: f64 = to_fixed(plus(&field(&heal, "range")), 0)
        .parse()
        .unwrap_or(0.0);
    let interval: f64 = to_fixed(plus(&field(&heal, "interval")) / 1000.0, 1)
        .parse()
        .unwrap_or(0.0);
    [
        format!(
            "{} {} {}",
            header_font(&translate("Heal:")),
            js_number(health),
            unit_font("Health")
        ),
        format!(
            "{} {} {}",
            header_font(&translate("Range:")),
            js_number(range),
            unit_font(&translate_plural("meter", "meters", range))
        ),
        format!(
            "{} {} {}",
            header_font(&translate("Interval:")),
            js_number(interval),
            unit_font(&translate_plural("second", "seconds", interval))
        ),
    ]
    .join(", ")
}

/// `getGarrisonTooltip`.
pub fn garrison_tooltip(template: &JsValue) -> String {
    let mut tooltips = Vec::new();
    let holder = field(template, "garrisonHolder");
    if holder.is_truthy() {
        tooltips.push(format!(
            "{}: {}",
            header_font(&translate("Garrison Limit")),
            js_number(plus(&field(&holder, "capacity")))
        ));
        let heal = field(&holder, "buffHeal");
        if heal.is_truthy() {
            tooltips.push(format!(
                "{} {} {} / {}",
                header_font(&translate("Heal:")),
                js_number(js_round(plus(&heal))),
                unit_font("Health"),
                unit_font("second")
            ));
        }
    }
    let garrisonable = field(template, "garrisonable");
    if garrisonable.is_truthy() {
        let extra = if holder.is_truthy() {
            plus(&field(&holder, "occupiedSlots"))
        } else {
            0.0
        };
        let size = plus(&field(&garrisonable, "size"));
        if size > 1.0 || extra != 0.0 {
            tooltips.push(format!(
                "{}: {} {}",
                header_font(&translate("Garrison Size")),
                js_number(size),
                if extra != 0.0 {
                    format!("+ {}", js_number(extra))
                } else {
                    String::new()
                }
            ));
        }
    }
    tooltips.join("\n")
}

/// `getTurretsTooltip`.
pub fn turrets_tooltip(template: &JsValue) -> String {
    let holder = field(template, "turretHolder");
    if !holder.is_truthy() {
        return String::new();
    }
    format!(
        "{}: {}",
        header_font(&translate("Turret Positions")),
        keys(&field(&holder, "turretPoints")).len()
    )
}

/// `getProjectilesTooltip`.
pub fn projectiles_tooltip(template: &JsValue) -> String {
    let holder = field(template, "garrisonHolder");
    let ai = field(template, "buildingAI");
    if !holder.is_truthy() || !ai.is_truthy() {
        return String::new();
    }
    let max = plus(&field(&ai, "maxArrowCount"));
    let default = plus(&field(&ai, "defaultArrowCount"));
    let multiplier = plus(&field(&ai, "garrisonArrowMultiplier"));
    let limit = (if max != 0.0 { max } else { f64::INFINITY })
        .min(default + js_round(multiplier * plus(&field(&holder, "capacity"))));
    if limit == 0.0 {
        return String::new();
    }
    [
        format!(
            "{}: {}",
            header_font(&translate("Projectile Limit")),
            js_number(limit)
        ),
        format!(
            "{}: {}",
            header_font(&translate("Default")),
            js_number(default)
        ),
        format!("{}: {}", header_font("Per Unit"), rounded(multiplier, 2)),
    ]
    .join(&comma_font(", "))
}

/// `getSpeedTooltip`.
pub fn speed_tooltip(template: &JsValue) -> String {
    let speed = field(template, "speed");
    if !speed.is_truthy() {
        return String::new();
    }
    let walk = plus(&field(&speed, "walk"));
    let run = plus(&field(&speed, "run"));
    if to_fixed(walk, 1).parse::<f64>() == Ok(0.0) && to_fixed(run, 1).parse::<f64>() == Ok(0.0) {
        return String::new();
    }
    let comma = comma_font(", ");
    format!(
        "{} {} {}{comma}{} {}{comma}{} {}",
        header_font(&translate("Speed:")),
        to_fixed(walk, 1),
        unit_font(&translate("Walk")),
        to_fixed(run, 1),
        unit_font(&translate("Run")),
        to_fixed(plus(&field(&speed, "acceleration")), 1),
        unit_font(&translate("Acceleration"))
    )
}

/// `getResourceDropsiteTooltip`.
pub fn dropsite_tooltip(template: &JsValue) -> String {
    let types = items(&field(&field(template, "resourceDropsite"), "types"));
    if types.is_empty() {
        return String::new();
    }
    format!(
        "{} {}",
        header_font(&translate("Dropsite for:")),
        types
            .iter()
            .map(|code| resource_icon(&text(code)))
            .collect::<Vec<_>>()
            .join("  ")
    )
}

/// `getGatherTooltip`.
pub fn gather_tooltip(template: &JsValue) -> String {
    let rates = field(template, "resourceGatherRates");
    if !rates.is_truthy() {
        return String::new();
    }
    let mut shown = Vec::new();
    for (code, subtypes) in SUBTYPES {
        for subtype in subtypes {
            let rate = plus(&field(&rates, &format!("{code}.{subtype}")));
            if rate > 0.0 {
                shown.push(format!(
                    "{} {}",
                    resource_icon(&format!("{code}_{subtype}")),
                    to_fixed(rate, 2)
                ));
            }
        }
    }
    if shown.is_empty() {
        return String::new();
    }
    format!(
        "{} {}",
        header_font(&translate("Gather Rates:")),
        shown.join("  ")
    )
}

/// `getLootTooltip`, with `carried` from the entity's own state.
pub fn loot_tooltip(template: &JsValue, carried: &[(String, f64)]) -> String {
    let loot = field(template, "loot");
    if !loot.is_truthy() && carried.is_empty() {
        return String::new();
    }
    let labels: Vec<String> = RESOURCES
        .iter()
        .chain(&["xp"])
        .filter_map(|code| {
            let amount = plus(&field(&loot, code))
                + carried
                    .iter()
                    .filter(|(kind, _)| kind == code)
                    .map(|(_, amount)| amount)
                    .sum::<f64>();
            (amount != 0.0).then(|| format!("{} {}", resource_icon(code), js_number(amount)))
        })
        .collect();
    if labels.is_empty() {
        return String::new();
    }
    format!("{} {}", header_font(&translate("Loot:")), labels.join("  "))
}

/// `getResourceTrickleTooltip` (`key` "resourceTrickle", `label` "Resource
/// Trickle:") and `getUpkeepTooltip` ("upkeep", "Upkeep:").
pub fn rates_tooltip(template: &JsValue, key: &str, label: &str) -> String {
    let data = field(template, key);
    let rates = field(&data, "rates");
    let shown: Vec<String> = RESOURCES
        .iter()
        .filter(|code| field(&rates, code).is_truthy())
        .map(|code| {
            format!(
                "{} {}",
                resource_icon(code),
                js_number(plus(&field(&rates, code)))
            )
        })
        .collect();
    if shown.is_empty() {
        return String::new();
    }
    format!(
        "{} {} / {}",
        header_font(label),
        shown.join("  "),
        seconds_string(plus(&field(&data, "interval")) / 1000.0)
    )
}

/// `getPopulationBonusTooltip`.
pub fn population_bonus_tooltip(template: &JsValue) -> String {
    let bonus = field(&field(template, "population"), "bonus");
    if !bonus.is_truthy() {
        return String::new();
    }
    format!(
        "{} {}",
        header_font(&translate("Population Bonus:")),
        js_number(plus(&bonus))
    )
}

/// `getEntityCostTooltip` for `count` of it, its time as one batch of one
/// (`GetBatchTime` is not sent), or `costs` instead of the template's
/// resources when given (a research list's, after the producer's
/// multiplier). Wall sets are left out.
pub fn cost_tooltip(template: &JsValue, count: f64, costs: Option<&[(String, f64)]>) -> String {
    let cost = field(template, "cost");
    if !cost.is_truthy() {
        return String::new();
    }
    let mut parts = Vec::new();
    for code in RESOURCES.iter().chain(&["population", "time"]) {
        let amount = if *code == "time" {
            (plus(&field(&cost, code))).ceil()
        } else if let Some(costs) = costs.filter(|_| *code != "population") {
            costs
                .iter()
                .filter(|(kind, _)| kind == code)
                .map(|(_, amount)| *amount)
                .sum::<f64>()
                .floor()
        } else {
            (plus(&field(&cost, code)) * count).floor()
        };
        if amount != 0.0 {
            parts.push(format!("{} {}", resource_icon(code), js_number(amount)));
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("{} {}", header_font(&translate("Cost:")), parts.join("  "))
}

/// `getNeededResourcesTooltip`.
pub fn needed_resources_tooltip(needed: &[(String, f64)]) -> String {
    if needed.is_empty() {
        return String::new();
    }
    format!(
        "{} {}",
        objection_font(&translate("Insufficient resources:")),
        needed
            .iter()
            .map(|(code, amount)| format!(
                "[font=\"sans-12\"]{}[/font] {}",
                resource_icon(code),
                js_number(amount.ceil())
            ))
            .collect::<Vec<_>>()
            .join("  ")
    )
}

/// `getRequirementsTooltip`; `technology` finds a technology's data for
/// the owner's civ.
pub fn requirements_tooltip(
    template: &JsValue,
    technology: &dyn Fn(&str) -> Option<JsValue>,
) -> String {
    let requirements = field(template, "requirements");
    let techs = text(&field(&field(&requirements, "Techs"), "_string"));
    if !techs.is_empty()
        && !techs.contains(' ')
        && !techs.starts_with('!')
        && let Some(data) = technology(&techs)
    {
        return objection_font(&format!("Requires {}", entity_names(&data)));
    }
    let tooltip = text(&field(&requirements, "Tooltip"));
    if tooltip.is_empty() {
        String::new()
    } else {
        objection_font(&tooltip)
    }
}

/// The detailed lines `setupButton` of the training panel adds with
/// showdetailedtooltips.
pub fn detailed_lines(template: &JsValue) -> Vec<String> {
    vec![
        health_tooltip(template),
        attack_tooltip(template),
        healer_tooltip(template),
        resistance_tooltip(template),
        garrison_tooltip(template),
        turrets_tooltip(template),
        projectiles_tooltip(template),
        speed_tooltip(template),
        dropsite_tooltip(template),
    ]
}

/// The JSON's `requirementsTooltip`, a technology's.
pub fn requirements_text(technology: &JsValue) -> String {
    translated(&field(technology, "requirementsTooltip"))
}
