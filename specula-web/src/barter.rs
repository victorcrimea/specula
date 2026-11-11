//! The barter buttons, a port of gui/session/trade/ (`BarterButton`,
//! `BarterButtonManager`), which the engine makes once for the selection
//! panels and once for the trade dialog; each has its own sell choice.

use std::cell::Cell;

use specula_assets::command::Command;
use web_sys::Document;
use web_sys::HtmlElement;

use crate::lang;
use crate::left::Tile;
use crate::panel::create;
use crate::panel::string_index;
use crate::stream::Scene;
use crate::tooltip;

/// `g_ResourceData.GetBarterableCodes()`, in the order of the resources'
/// data files.
pub(crate) const CODES: [&str; 4] = ["food", "wood", "stone", "metal"];
/// `BarterButton.prototype.BarterResourceSellQuantity`.
pub(crate) const QUANTITY: u32 = 100;
/// `BarterButton.prototype.Multiplier`, under session.massbarter.
pub(crate) const MULTIPLIER: u32 = 5;
/// A buy button under its sell button, `BarterButton.xml`.
const BUY_TOP: usize = 42;

/// A sell button, with the buy button under it.
struct Item {
    sell: Tile,
    buy: Tile,
}

pub(crate) struct BarterButtons {
    items: Vec<Item>,
    /// `selectedResource`, an index of `CODES`, and whether the page
    /// should fill the buttons again.
    selected: Cell<usize>,
    redraw: Cell<bool>,
    /// `controlsPlayer(viewedPlayer)` at the last update, so a buy press
    /// orders.
    controls: bool,
}

impl BarterButtons {
    /// `count` items in `parent`, each a `class` box placed by `position`.
    pub(crate) fn new(
        document: &Document,
        parent: &HtmlElement,
        count: usize,
        class: &str,
        position: &dyn Fn(usize) -> String,
    ) -> Result<Self, String> {
        let mut items = Vec::new();
        for i in 0..count {
            let item: HtmlElement = create(document, "div")?;
            let _ = item.set_attribute("class", class);
            let _ = item.set_attribute("style", &position(i));
            parent
                .append_child(&item)
                .map_err(|_| "cannot add a barter item")?;
            items.push(Item {
                sell: Tile::new(document, &item, "left: 0; top: 0")?,
                buy: Tile::new(document, &item, &format!("left: 0; top: {BUY_TOP}px"))?,
            });
        }
        Ok(BarterButtons {
            items,
            selected: Cell::new(0),
            redraw: Cell::new(false),
            controls: false,
        })
    }

    /// The sell buttons, for the page to wire to `sell_clicked`.
    pub(crate) fn sell_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.items.iter().map(|item| &item.sell.tile.element)
    }

    /// The buy buttons, for the page to wire to `buy_clicked`.
    pub(crate) fn buy_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.items.iter().map(|item| &item.buy.tile.element)
    }

    /// `setSelectedResource`.
    pub(crate) fn sell_clicked(&self, index: usize) {
        if index < CODES.len() {
            self.selected.set(index);
            self.redraw.set(true);
        }
    }

    /// `BarterButton.buy`, for a viewer who controls the player; `mass`
    /// is session.massbarter held.
    pub(crate) fn buy_clicked(&self, index: usize, mass: bool) -> Option<Command> {
        let buy = CODES.get(index)?;
        let sell = CODES[self.selected_index()];
        (self.controls && *buy != sell).then(|| Command::Barter {
            sell: sell.to_string(),
            buy: buy.to_string(),
            amount: amount(mass),
        })
    }

    /// Whether a press changed what the buttons show, once.
    pub(crate) fn take_redraw(&self) -> bool {
        self.redraw.replace(false)
    }

    fn selected_index(&self) -> usize {
        self.selected.get().min(CODES.len() - 1)
    }

    /// `BarterButtonManager.update` for `player`, `controls` when the
    /// viewer commands it.
    pub(crate) fn update(&mut self, scene: &Scene, player: i32, controls: bool, mass: bool) {
        self.controls = controls && player >= 1;
        let amount = f64::from(amount(mass));
        let selected_code = CODES[self.selected_index()];
        let stock = |code: &str| {
            let index = string_index(scene, code)?;
            let economy = usize::try_from(player)
                .ok()
                .and_then(|player| scene.economy.get(player))?;
            economy
                .resources
                .iter()
                .find(|stock| stock.code == index)
                .map(|stock| f64::from(stock.amount))
        };
        let prices = usize::try_from(player)
            .ok()
            .and_then(|player| scene.details.players.get(player))
            .map(|player| player.prices.as_slice())
            .unwrap_or_default();
        let price = |code: &str| {
            let index = string_index(scene, code)?;
            prices
                .iter()
                .find(|(resource, _, _)| *resource == index)
                .map(|(_, buy, sell)| (f64::from(*buy), f64::from(*sell)))
        };
        // `neededResourcesSell` and `neededResourcesBuy`.
        let short = |code: &str| stock(code).is_some_and(|stock| stock < amount);
        let buy_short = short(selected_code);
        let amount_text = amount.to_string();
        for (index, item) in self.items.iter_mut().enumerate() {
            let Some(&code) = CODES.get(index) else {
                item.sell.tile.hidden(true);
                item.buy.tile.hidden(true);
                continue;
            };
            if player < 1 {
                item.sell.tile.hidden(true);
                item.buy.tile.hidden(true);
                continue;
            }
            let is_selected = code == selected_code;
            let name = lang::translate_context(
                "withinSentence",
                tooltip::resource_data(code).map_or(code, |(name, _)| name),
            );
            item.sell.show(&format!("resource-{code}"), is_selected);
            item.sell.tint.style(tint(is_selected, short(code)));
            item.sell.tile.class(String::from(if is_selected {
                "item selected"
            } else {
                "item"
            }));
            item.sell.caption.text(&lang::format(
                &lang::translate_context("sell action", "-%(amount)s"),
                &[("amount", &amount_text)],
            ));
            item.sell.tile.tooltip(&lang::format(
                &lang::translate("Sell %(resource)s"),
                &[("resource", &name)],
            ));

            item.buy.show(&format!("resource-{code}"), false);
            item.buy.tint.style(tint(false, buy_short));
            // Math.round of the amount bought.
            let bought = match (price(selected_code), price(code)) {
                (Some((_, sell)), Some((buy, _))) if buy > 0.0 => {
                    (sell / buy * amount + 0.5).floor()
                }
                _ => 0.0,
            };
            item.buy.caption.text(&lang::format(
                &lang::translate_context("buy action", "+%(amount)s"),
                &[("amount", &bought.to_string())],
            ));
            item.buy.tile.tooltip(&lang::format(
                &lang::translate("Buy %(resource)s"),
                &[("resource", &name)],
            ));
            item.buy.tile.hidden(is_selected);
        }
    }
}

/// `amountToSell`.
fn amount(mass: bool) -> u32 {
    if mass {
        QUANTITY * MULTIPLIER
    } else {
        QUANTITY
    }
}

/// `barterButton`'s sprite modifiers: `SelectedModifier`, then
/// `DisabledModifier` over it.
fn tint(selected: bool, short: bool) -> String {
    let mut layers = Vec::new();
    if short {
        layers.push("linear-gradient(rgba(255, 0, 0, 0.314), rgba(255, 0, 0, 0.314))");
    }
    if selected {
        layers.push("linear-gradient(rgba(0, 0, 0, 0.392), rgba(0, 0, 0, 0.392))");
    }
    if layers.is_empty() {
        String::new()
    } else {
        format!("background: {}", layers.join(", "))
    }
}
