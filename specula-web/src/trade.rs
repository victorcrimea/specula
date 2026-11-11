//! The trade routes' rules as the session GUI reads them: a port of
//! `globalscripts/Trade.js`, `Market.CalculateTraderGain`,
//! `Trader.CalculateGain`, `Trader.CanTrade`,
//! `GuiInterface.GetTradingDetails` and `getTradingTooltip`, from what the
//! streams carry. Positions are the visual stream's 1/16 m, so a gain can
//! differ from the engine's only right at a rounding edge.

use crate::detail;
use crate::detail::Group;
use crate::lang;
use crate::orders::is_enemy;
use crate::panel::entity_groups;
use crate::panel::template_details;
use crate::pick;
use crate::stream::COMPONENT_FOUNDATION;
use crate::stream::COMPONENT_MARKET;
use crate::stream::COMPONENT_MARKET_LAND;
use crate::stream::COMPONENT_MARKET_NAVAL;
use crate::stream::COMPONENT_TRADER;
use crate::stream::Scene;

/// `CalculateTraderGain`'s result; a market's gain is `None` where the
/// engine leaves it undefined (both markets of one owner).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gain {
    pub trader_gain: f64,
    pub market1_gain: Option<f64>,
    pub market2_gain: Option<f64>,
    pub market1_owner: i32,
    pub market2_owner: i32,
    pub trader_owner: i32,
}

/// What `GetTradingDetails` reports for a trader and a market; the gains
/// of the first two are `GetGoods().amount`, here worked out again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Details {
    /// The trader's first market; `both` when it also has a second.
    IsFirst {
        both: bool,
        gain: Option<Gain>,
    },
    IsSecond {
        gain: Option<Gain>,
    },
    SetFirst,
    /// The trader's gain from its first market to this one.
    SetSecond {
        gain: Gain,
    },
}

/// `TradeGain`: the part of the gain that depends on the distance.
fn trade_gain(distance_squared: f64, map_size: f64) -> f64 {
    distance_squared / (1.0 + 0.25 * distance_squared.sqrt() / map_size)
}

/// `TradeGainNormalization`.
fn normalization(map_size: f64) -> f64 {
    (1024.0 / map_size).sqrt() / trade_gain(10000.0, map_size)
}

/// `Math.round`, for the non-negative gains.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// An entity's detail group of `slot`.
fn group(scene: &Scene, id: u32, slot: usize) -> Option<Group> {
    let dot = scene.dots.get(&id)?;
    entity_groups(scene, id, dot.template)[slot].clone()
}

/// The trader a gain is for: the entity, or a template's `GainMultiplier`
/// with the first market's owner, as the GUI's rally route asks.
#[derive(Clone, Copy)]
pub enum For {
    Entity(u32),
    Template(f64),
}

/// `Market.CalculateTraderGain(second, ...)` of market `first`, then for
/// an entity `Trader.CalculateGain`'s garrison multiplier; `None` where
/// either returns null.
pub fn calculate_gain(scene: &Scene, first: u32, second: u32, trader: For) -> Option<Gain> {
    let market = |id: u32| {
        let dot = scene.dots.get(&id)?;
        let details = template_details(scene, dot.template)?;
        // `QueryOwnerInterface` finds no player for an unowned entity.
        (details.components & COMPONENT_MARKET != 0 && dot.owner >= 0).then_some(dot)
    };
    let (first_dot, second_dot) = (market(first)?, market(second)?);
    let map_size = f64::from(scene.map_size);
    if map_size <= 0.0 {
        return None;
    }
    let (multiplier, trader_owner) = match trader {
        For::Entity(id) => match group(scene, id, detail::TRADE_ROUTE)? {
            Group::TradeRoute { multiplier, .. } => {
                (f64::from(multiplier), scene.dots.get(&id)?.owner)
            }
            _ => return None,
        },
        // `!traderTemplate.GainMultiplier`.
        For::Template(0.0) => return None,
        For::Template(multiplier) => (multiplier, first_dot.owner),
    };
    let gain_multiplier = normalization(map_size) * multiplier;
    let dx = (f64::from(first_dot.x) - f64::from(second_dot.x)) / 16.0;
    let dz = (f64::from(first_dot.z) - f64::from(second_dot.z)) / 16.0;
    let trader_gain = js_round(gain_multiplier * trade_gain(dx * dx + dz * dz, map_size));
    let bonus = |id: u32| match group(scene, id, detail::MARKET) {
        Some(Group::MarketBonus(bonus)) => f64::from(bonus),
        _ => 0.0,
    };
    let international = first_dot.owner != second_dot.owner;
    let mut gain = Gain {
        trader_gain,
        market1_gain: international.then(|| js_round(trader_gain * bonus(first))),
        market2_gain: international.then(|| js_round(trader_gain * bonus(second))),
        market1_owner: first_dot.owner,
        market2_owner: second_dot.owner,
        trader_owner,
    };
    if let For::Entity(id) = trader
        && let Some(Group::TradeGarrison(garrison_gain)) = group(scene, id, detail::TRADE_GARRISON)
        && let Some(Group::Garrison { entities, .. }) = group(scene, id, detail::GARRISON)
    {
        let traders = entities
            .iter()
            .filter(|garrisoned| {
                template_details(scene, garrisoned.template)
                    .is_some_and(|details| details.components & COMPONENT_TRADER != 0)
            })
            .count();
        let scale = 1.0 + f64::from(garrison_gain) * traders as f64;
        // A gain of 0 stays as it is, as `if (gain.traderGain)` skips it.
        let apply = |value: f64| js_round(scale * value);
        if gain.trader_gain != 0.0 {
            gain.trader_gain = apply(gain.trader_gain);
        }
        gain.market1_gain = gain
            .market1_gain
            .map(|value| if value != 0.0 { apply(value) } else { value });
        gain.market2_gain = gain
            .market2_gain
            .map(|value| if value != 0.0 { apply(value) } else { value });
    }
    Some(gain)
}

/// `Market.HasType` of `target`'s template: (land, naval).
pub fn market_types(scene: &Scene, target: u32) -> Option<(bool, bool)> {
    let dot = scene.dots.get(&target)?;
    let details = template_details(scene, dot.template)?;
    (details.components & COMPONENT_MARKET != 0).then_some((
        details.components & COMPONENT_MARKET_LAND != 0,
        details.components & COMPONENT_MARKET_NAVAL != 0,
    ))
}

/// `Trader.CanTrade`, for `player`'s trader.
fn can_trade(scene: &Scene, player: i32, trader: u32, target: u32) -> bool {
    let Some((land, naval)) = market_types(scene, target) else {
        return false;
    };
    let (Some(trader_dot), Some(target_dot)) = (scene.dots.get(&trader), scene.dots.get(&target))
    else {
        return false;
    };
    let foundation = template_details(scene, target_dot.template)
        .is_some_and(|details| details.components & COMPONENT_FOUNDATION != 0);
    let Some(trader_details) = template_details(scene, trader_dot.template) else {
        return false;
    };
    let has_class = |class: &str| pick::has_class(trader_details, &scene.strings, class);
    !foundation
        && ((has_class("Organic") && land) || (has_class("Ship") && naval))
        && target_dot.owner >= 0
        && !is_enemy(scene, player, target_dot.owner)
}

/// `GetTradingDetails` for `player`'s `trader` and `target`; `None` where
/// it returns null, and where the GUI would read the gain of a null
/// `CalculateGain` (a first market that is gone).
pub fn trading_details(scene: &Scene, player: i32, trader: u32, target: u32) -> Option<Details> {
    let Some(Group::TradeRoute { first, second, .. }) = group(scene, trader, detail::TRADE_ROUTE)
    else {
        return None;
    };
    if !can_trade(scene, player, trader, target) {
        return None;
    }
    let route_gain = || calculate_gain(scene, first, second, For::Entity(trader));
    Some(if first != 0 && target == first {
        let both = second != 0;
        Details::IsFirst {
            both,
            gain: both.then(route_gain).flatten(),
        }
    } else if second != 0 && target == second {
        Details::IsSecond { gain: route_gain() }
    } else if first == 0 {
        Details::SetFirst
    } else if second == 0 {
        Details::SetSecond {
            gain: calculate_gain(scene, first, target, For::Entity(trader))?,
        }
    } else {
        Details::SetFirst
    })
}

/// `tradingGainString`.
fn gain_string(scene: &Scene, gain: &str, owner: i32) -> String {
    let player = usize::try_from(owner)
        .ok()
        .and_then(|owner| scene.players.get(owner))
        .map_or("", |player| player.name.as_str());
    lang::format(
        &lang::translate("%(gain)s (%(player)s)"),
        &[("gain", gain), ("player", player)],
    )
}

/// `getTradingTooltip`: the trader owner's gain, with the markets' shares
/// that are also its, then the other owners' shares.
pub fn trading_tooltip(scene: &Scene, gain: Option<&Gain>) -> String {
    let Some(gain) = gain else {
        return String::new();
    };
    let markets = [
        (gain.market1_gain, gain.market1_owner),
        (gain.market2_gain, gain.market2_owner),
    ];
    let shown = |value: Option<f64>| value.filter(|value| *value != 0.0);
    let mut primary = gain.trader_gain.to_string();
    for (value, owner) in markets {
        if let Some(value) = shown(value)
            && owner == gain.trader_owner
        {
            primary.push_str(&lang::translate("+"));
            primary.push_str(&value.to_string());
        }
    }
    let mut tooltip = gain_string(scene, &primary, gain.trader_owner);
    for (value, owner) in markets {
        if let Some(value) = shown(value)
            && owner != gain.trader_owner
        {
            tooltip.push_str(&lang::translate_context(
                "Separation mark in an enumeration",
                ", ",
            ));
            tooltip.push_str(&gain_string(scene, &value.to_string(), owner));
        }
    }
    tooltip
}

/// `sprintf(translate("Gain: %(gain)s"), ...)`.
pub fn gain_line(scene: &Scene, gain: Option<&Gain>) -> String {
    lang::format(
        &lang::translate("Gain: %(gain)s"),
        &[("gain", &trading_tooltip(scene, gain))],
    )
}
