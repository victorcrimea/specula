//! The Barter & Trade dialog, a port of gui/session/trade/ (`TradeDialog`,
//! its `BarterPanel` and `TradePanel`, `TradeButtonManager`,
//! `TraderStatusText`) into the page's `#trade-window`, in the engine's
//! pixels.
//!
//! As in the engine it opens only for a player perspective, and only the
//! player of `/play`, viewing itself, can barter or change the trade
//! shares; for anyone else it shows them.

use specula_assets::command::Command;
use web_sys::Document;
use web_sys::HtmlElement;

use crate::barter;
use crate::barter::BarterButtons;
use crate::command_row;
use crate::diplomacy::Perspective;
use crate::lang;
use crate::panel::Out;
use crate::panel::create;
use crate::panel::string_index;
use crate::stream::Scene;
use crate::tooltip;

/// `tradeDialogPanel`'s 200 x 388 px, which `onPlayersInit` widens by the
/// wider of the barter and trade rows.
const WIDTH: usize = 200;
const HEIGHT: usize = 388;
/// `barterResources`' and `tradeResources`' children, 58 px wide, a pixel
/// apart (`setPanelObjectPosition`).
const ITEM_WIDTH: usize = 58;
const ITEM_STEP: usize = ITEM_WIDTH + 1;
/// `g_ResourceData.GetTradableCodes()`, in the order of the resources'
/// data files.
const TRADE_CODES: [&str; 4] = ["food", "wood", "stone", "metal"];
/// `TradeButton.prototype.AmountStep`.
const AMOUNT_STEP: u8 = 5;
/// `TraderStatusText.prototype.IdleTraderTextTags`.
const IDLE_COLOR: &str = "orange";

/// What one update reads.
pub struct View<'a> {
    pub scene: &'a Scene,
    pub perspective: Perspective,
    /// The player the viewer commands on `/play`, `None` for an observer.
    pub player: Option<i32>,
    /// session.massbarter (Shift) is held.
    pub mass_barter: bool,
}

/// One `tradeResource[n]`.
struct TradeItem {
    root: Out,
    button: Out,
    selection: Out,
    text: Out,
    up: Out,
    down: Out,
}

/// `#trade-window`.
pub struct TradeWindow {
    panel: Out,
    /// `tradeButton` in the top bar, which an older page lacks.
    button: Option<Out>,
    title: HtmlElement,
    close: HtmlElement,
    /// The fixed captions and the English each shows.
    labels: Vec<(HtmlElement, &'static str)>,
    no_markets: Out,
    barter_cells: Out,
    barter_help: Out,
    barter: BarterButtons,
    trade: Vec<TradeItem>,
    trade_help: Out,
    status: Out,
    status_markup: String,
    /// `TradeButtonManager.selectedResource`, an index of `TRADE_CODES`.
    selected: usize,
    /// `tradingGoods`: a player's own copy, which "assumes the simulation
    /// state will always follow the GUI", with the match it is of; an
    /// observer reads the stream's each time.
    goods: Option<(u64, [u8; TRADE_CODES.len()])>,
    /// `controlsPlayer(g_ViewedPlayer)` at the last update.
    controls: bool,
    /// Whether the captions need setting, as the language changed.
    labelled: bool,
}

impl TradeWindow {
    pub fn new(document: &Document) -> Result<Self, String> {
        let width = WIDTH + ITEM_WIDTH * barter::CODES.len().max(TRADE_CODES.len());
        let mut panel = Out::find(document, "trade-window")?;
        panel.style(format!(
            "--dialog-width: {width}px; --dialog-height: {HEIGHT}px"
        ));
        let barter_section: HtmlElement = crate::element(document, "trade-barter")?;
        let trade_section: HtmlElement = crate::element(document, "trade-trade")?;
        let mut labels = Vec::new();

        // BarterPanel.xml.
        let header = text(
            document,
            &barter_section,
            "left: 0; top: 0; right: 0; height: 32px",
            "",
        )?;
        labels.push((header, "Barter"));
        line(document, &barter_section)?;
        let area = block(
            document,
            &barter_section,
            "left: 0; top: 38px; right: 0; height: 84px",
        )?;
        let sell = text(
            document,
            &area,
            "left: 0; top: 0; width: 100px; height: 41px",
            "",
        )?;
        labels.push((sell, "Sell:"));
        let buy = text(
            document,
            &area,
            "left: 0; bottom: 0; width: 100px; height: 41px",
            "",
        )?;
        labels.push((buy, "Buy:"));
        let no_markets = text(
            document,
            &area,
            "left: 112px; top: 0; right: 8px; bottom: 0",
            "center",
        )?;
        labels.push((no_markets.clone(), "No Markets Available"));
        let barter_cells = block(document, &area, "left: 112px; top: 0; right: 0; bottom: 0")?;
        let barter = BarterButtons::new(
            document,
            &barter_cells,
            barter::CODES.len(),
            "barter",
            &|i| format!("left: {}px; top: 0", i * ITEM_STEP),
        )?;
        let barter_help = help(document, &area, "right: 0; top: 30px")?;

        // TradePanel.xml.
        let header = text(
            document,
            &trade_section,
            "left: 0; top: 0; right: 0; height: 32px",
            "",
        )?;
        labels.push((header, "Trade"));
        line(document, &trade_section)?;
        let area = block(
            document,
            &trade_section,
            "left: 0; top: 38px; right: 0; height: 32px",
        )?;
        let resources = text(
            document,
            &area,
            "left: 0; top: 0; width: 100px; bottom: 0",
            "",
        )?;
        labels.push((resources, "Resources:"));
        let cells = block(document, &area, "left: 112px; top: 0; right: 0; bottom: 0")?;
        let mut trade = Vec::new();
        for (i, code) in TRADE_CODES.iter().enumerate() {
            let root = block(
                document,
                &cells,
                &format!(
                    "left: {}px; top: 0; width: {ITEM_WIDTH}px; height: 32px",
                    i * ITEM_STEP
                ),
            )?;
            let button: HtmlElement = create(document, "button")?;
            let _ = button.set_attribute("type", "button");
            let _ = button.set_attribute("class", "good stone");
            root.append_child(&button)
                .map_err(|_| "cannot build a trade button")?;
            let icon = block(
                document,
                &button,
                &format!("inset: 0; background-image: var(--gui-resource-{code})"),
            )?;
            let _ = icon.set_attribute("class", "icon");
            let selection = block(document, &button, "inset: 0")?;
            let _ = selection.set_attribute("class", "selection");
            let caption = text(document, &button, "inset: 0", "center")?;
            let arrow = |class: &str| -> Result<HtmlElement, String> {
                let arrow: HtmlElement = create(document, "button")?;
                let _ = arrow.set_attribute("type", "button");
                let _ = arrow.set_attribute("class", class);
                root.append_child(&arrow)
                    .map_err(|_| "cannot build a trade arrow")?;
                Ok(arrow)
            };
            trade.push(TradeItem {
                root: Out::new(root.clone()),
                button: Out::new(button),
                selection: Out::new(selection),
                text: Out::new(caption),
                up: Out::new(arrow("arrow up")?),
                down: Out::new(arrow("arrow down")?),
            });
        }
        let trade_help = help(document, &area, "right: 0; top: 4px")?;
        let status = block(
            document,
            &trade_section,
            "left: 8px; top: 88px; right: 0; bottom: 0",
        )?;
        let _ = status.set_attribute("class", "block status sans-bold-stroke-14");

        let mut window = TradeWindow {
            panel,
            button: Out::find(document, "top-trade").ok(),
            title: crate::element(document, "trade-title")?,
            close: crate::element(document, "trade-close")?,
            labels,
            no_markets: Out::new(no_markets),
            barter_cells: Out::new(barter_cells),
            barter_help: Out::new(barter_help),
            barter,
            trade,
            trade_help: Out::new(trade_help),
            status: Out::new(status),
            status_markup: String::new(),
            selected: 0,
            goods: None,
            controls: false,
            labelled: false,
        };
        window.panel.hidden(true);
        Ok(window)
    }

    pub fn close_button(&self) -> &HtmlElement {
        &self.close
    }

    /// The barter sell and buy buttons, for the page to wire.
    pub fn barter(&self) -> &BarterButtons {
        &self.barter
    }

    /// The trade buttons and their up and down arrows, for the page to
    /// wire.
    pub fn trade_buttons(&self) -> Vec<(HtmlElement, HtmlElement, HtmlElement)> {
        self.trade
            .iter()
            .map(|item| {
                (
                    item.button.element.clone(),
                    item.up.element.clone(),
                    item.down.element.clone(),
                )
            })
            .collect()
    }

    pub fn is_open(&self) -> bool {
        !self.panel.element.hidden()
    }

    /// `open` refuses, as the engine's, without a viewed player.
    pub fn set_open(&mut self, open: bool, perspective: Perspective) {
        let open = open && matches!(perspective, Perspective::Player(id) if id >= 1);
        self.panel.hidden(!open);
    }

    /// The language changed, so every label is set again.
    pub fn invalidate(&mut self) {
        self.labelled = false;
        self.status_markup.clear();
    }

    /// `TradeButtonManager.selectResource`: with session.fulltradeswap
    /// (`full`), the resource's share becomes 100%, which is ordered.
    pub fn trade_clicked(&mut self, index: usize, full: bool) -> Option<Command> {
        if !self.controls || index >= TRADE_CODES.len() {
            return None;
        }
        self.selected = index;
        if !full {
            return None;
        }
        let (_, goods) = self.goods.as_mut()?;
        goods.fill(0);
        goods[index] = 100;
        Some(trading_goods(goods))
    }

    /// `tradeArrowUp` (`up`) or `tradeArrowDn` of resource `index`:
    /// `changeResourceAmount`, moving a step from the selected resource
    /// to it or back.
    pub fn arrow_clicked(&mut self, index: usize, up: bool) -> Option<Command> {
        if !self.controls || index >= TRADE_CODES.len() || index == self.selected {
            return None;
        }
        let selected = self.selected;
        let (_, goods) = self.goods.as_mut()?;
        if up {
            let amount = AMOUNT_STEP.min(goods[selected]);
            goods[selected] -= amount;
            goods[index] += amount;
        } else {
            let amount = AMOUNT_STEP.min(goods[index]);
            goods[selected] += amount;
            goods[index] -= amount;
        }
        Some(trading_goods(goods))
    }

    /// `updatePanels` while it is open; a perspective without a viewed
    /// player closes it, as `onViewedPlayerChange`.
    pub fn update(&mut self, view: &View) {
        // `TradeDialogButton.onViewedPlayerChange` and `onHotkeyChange`.
        if let Some(button) = &mut self.button {
            button.hidden(!matches!(view.perspective, Perspective::Player(id) if id >= 1));
            button.tooltip(&format!(
                "{}{}",
                command_row::hotkey_prefix("session.gui.barter.toggle"),
                lang::translate("Barter & Trade")
            ));
        }
        if !self.is_open() {
            return;
        }
        let Perspective::Player(viewed) = view.perspective else {
            self.panel.hidden(true);
            return;
        };
        let scene = view.scene;
        let player = scene.details.players.get(viewed);
        let id = viewed as i32;
        if !self.labelled {
            lang::set_text(&self.title, "Barter & Trade");
            lang::set_text(&self.close, "Close");
            for (element, english) in &self.labels {
                lang::set_text(element, english);
            }
            self.labelled = true;
        }

        // `BarterPanel.update`.
        let can_barter = player.is_some_and(|player| player.can_barter);
        self.barter
            .update(scene, id, view.player == Some(id), view.mass_barter);
        self.no_markets.hidden(can_barter);
        self.barter_cells.hidden(!can_barter);
        self.barter_help.hidden(!can_barter);
        self.barter_help.tooltip(&lang::format(
            &lang::translate(
                "Start by selecting the resource you wish to sell from the upper row. For each time the lower buttons are pressed, %(quantity)s of the upper resource will be sold for the displayed quantity of the lower. Press and hold %(hotkey)s to temporarily multiply the traded amount by %(multiplier)s.",
            ),
            &[
                ("quantity", &barter::QUANTITY.to_string()),
                ("hotkey", &command_row::hotkey_text("session.massbarter")),
                ("multiplier", &barter::MULTIPLIER.to_string()),
            ],
        ));

        // `TradeButtonManager.update`.
        self.controls = view.player == Some(id);
        let streamed = streamed_goods(scene, viewed);
        if !self.controls {
            self.goods = streamed.map(|goods| (scene.match_id, goods));
        } else if self
            .goods
            .is_none_or(|(match_id, _)| match_id != scene.match_id)
        {
            self.goods = streamed.map(|goods| (scene.match_id, goods));
        }
        let goods = self.goods.map(|(_, goods)| goods).unwrap_or_default();
        let enabled = self.controls;
        let selected_amount = goods[self.selected];
        for (index, item) in self.trade.iter_mut().enumerate() {
            let is_selected = index == self.selected;
            let amount = goods[index];
            item.root.hidden(false);
            item.text.text(
                &lang::format(
                    &lang::translate_context("trading good ratio", "%(amount)s%%"),
                    &[("amount", &amount.to_string())],
                )
                .replace("%%", "%"),
            );
            item.button.class(String::from(if enabled {
                "good stone"
            } else {
                "good stone disabled"
            }));
            item.selection.hidden(!enabled || !is_selected);
            item.up
                .hidden(!enabled || is_selected || amount == 100 || selected_amount == 0);
            item.down
                .hidden(!enabled || is_selected || amount == 0 || selected_amount == 100);
        }
        self.trade_help.tooltip(
            &lang::format(
                &lang::translate(
                    "Select one type of goods you want to modify by clicking on it, and then use the arrows of the other types to modify their shares. You can also press %(hotkey)s while selecting one type of goods to bring its share to 100%%.",
                ),
                &[("hotkey", &command_row::hotkey_text("session.fulltradeswap"))],
            )
            .replace("%%", "%"),
        );

        // `TraderStatusText.update`.
        let traders = player.map(|player| player.traders).unwrap_or_default();
        let markup = format!("{}\n\n{}", land_text(traders), ship_text(traders));
        if markup != self.status_markup {
            self.status
                .element
                .set_inner_html(&tooltip::to_html(&markup));
            self.status_markup = markup;
        }
    }
}

/// The viewed player's shares from the stream, in `TRADE_CODES` order;
/// `None` when it sends none, as for another player than a seat's own.
fn streamed_goods(scene: &Scene, player: usize) -> Option<[u8; TRADE_CODES.len()]> {
    let streamed = &scene.details.players.get(player)?.trading_goods;
    if streamed.is_empty() {
        return None;
    }
    let mut goods = [0; TRADE_CODES.len()];
    for (slot, code) in goods.iter_mut().zip(TRADE_CODES) {
        let index = string_index(scene, code);
        *slot = streamed
            .iter()
            .find(|(resource, _)| Some(*resource) == index)
            .map_or(0, |(_, percent)| *percent);
    }
    Some(goods)
}

/// `setTradingGoods`.
fn trading_goods(goods: &[u8; TRADE_CODES.len()]) -> Command {
    Command::SetTradingGoods {
        goods: TRADE_CODES
            .iter()
            .zip(goods)
            .map(|(code, percent)| (code.to_string(), *percent))
            .collect(),
    }
}

/// `TraderStatusTextLand.getText`.
fn land_text(traders: [u16; 5]) -> String {
    let [total, active, garrisoned, _, _] = traders.map(f64::from);
    let inactive = (total - active - garrisoned).max(0.0);
    let count = |value: f64| (value as u32).to_string();
    let active_string = lang::format(
        &lang::translate_plural(
            "There is %(numberTrading)s land trader trading",
            "There are %(numberTrading)s land traders trading",
            active,
        ),
        &[("numberTrading", &count(active))],
    );
    let inactive_string = lang::format(
        &if active > 0.0 || garrisoned > 0.0 {
            lang::translate_plural(
                "%(numberOfLandTraders)s inactive",
                "%(numberOfLandTraders)s inactive",
                inactive,
            )
        } else {
            lang::translate_plural(
                "%(numberOfLandTraders)s land trader inactive",
                "%(numberOfLandTraders)s land traders inactive",
                inactive,
            )
        },
        &[("numberOfLandTraders", &count(inactive))],
    );
    let garrisoned_string = lang::format(
        &if active > 0.0 || inactive > 0.0 {
            lang::translate_plural(
                "%(numberGarrisoned)s garrisoned on a trading merchant ship",
                "%(numberGarrisoned)s garrisoned on a trading merchant ship",
                garrisoned,
            )
        } else {
            lang::translate_plural(
                "There is %(numberGarrisoned)s land trader garrisoned on a trading merchant ship",
                "There are %(numberGarrisoned)s land traders garrisoned on a trading merchant ship",
                garrisoned,
            )
        },
        &[("numberGarrisoned", &count(garrisoned))],
    );
    let message = match (active > 0.0, garrisoned > 0.0, inactive > 0.0) {
        (true, true, false) => {
            lang::translate("%(openingTradingString)s, and %(garrisonedString)s.")
        }
        (true, true, true) => lang::translate(
            "%(openingTradingString)s, %(garrisonedString)s, and %(inactiveString)s.",
        ),
        (true, false, false) => lang::translate("%(openingTradingString)s."),
        (true, false, true) => lang::translate("%(openingTradingString)s, and %(inactiveString)s."),
        (false, true, false) => lang::translate("%(openingGarrisonedString)s."),
        (false, true, true) => {
            lang::translate("%(openingGarrisonedString)s, and %(inactiveString)s.")
        }
        (false, false, true) => lang::translate_plural(
            "There is %(inactiveString)s.",
            "There are %(inactiveString)s.",
            inactive,
        ),
        (false, false, false) => lang::translate("There are no land traders."),
    };
    lang::format(
        &message,
        &[
            ("openingTradingString", &active_string),
            ("openingGarrisonedString", &garrisoned_string),
            ("garrisonedString", &garrisoned_string),
            (
                "inactiveString",
                &tooltip::colored(&inactive_string, IDLE_COLOR),
            ),
        ],
    )
}

/// `TraderStatusTextShip.getText`.
fn ship_text(traders: [u16; 5]) -> String {
    let [_, _, _, total, active] = traders.map(f64::from);
    let inactive = (total - active).max(0.0);
    let count = |value: f64| (value as u32).to_string();
    let active_string = lang::format(
        &lang::translate_plural(
            "There is %(numberTrading)s merchant ship trading",
            "There are %(numberTrading)s merchant ships trading",
            active,
        ),
        &[("numberTrading", &count(active))],
    );
    let inactive_string = lang::format(
        &if active > 0.0 {
            lang::translate_plural(
                "%(numberOfShipTraders)s inactive",
                "%(numberOfShipTraders)s inactive",
                inactive,
            )
        } else {
            lang::translate_plural(
                "%(numberOfShipTraders)s merchant ship inactive",
                "%(numberOfShipTraders)s merchant ships inactive",
                inactive,
            )
        },
        &[("numberOfShipTraders", &count(inactive))],
    );
    let message = match (active > 0.0, inactive > 0.0) {
        (true, true) => lang::translate("%(openingTradingString)s, and %(inactiveString)s."),
        (true, false) => lang::translate("%(openingTradingString)s."),
        (false, true) => lang::translate_plural(
            "There is %(inactiveString)s.",
            "There are %(inactiveString)s.",
            inactive,
        ),
        (false, false) => lang::translate("There are no merchant ships."),
    };
    lang::format(
        &message,
        &[
            ("openingTradingString", &active_string),
            (
                "inactiveString",
                &tooltip::colored(&inactive_string, IDLE_COLOR),
            ),
        ],
    )
}

/// An absolutely placed box of `parent`.
fn block(document: &Document, parent: &HtmlElement, style: &str) -> Result<HtmlElement, String> {
    let block: HtmlElement = create(document, "div")?;
    let _ = block.set_attribute("class", "block");
    let _ = block.set_attribute("style", style);
    parent
        .append_child(&block)
        .map_err(|_| "cannot build the trade window")?;
    Ok(block)
}

/// A `ModernLeftLabelText` (or with `align`, `ModernLabelText`) caption.
fn text(
    document: &Document,
    parent: &HtmlElement,
    style: &str,
    align: &str,
) -> Result<HtmlElement, String> {
    let text = block(document, parent, style)?;
    let _ = text.set_attribute("class", &format!("text sans-bold-stroke-14 {align}"));
    Ok(text)
}

/// `ModernGoldLine` at `0 28 100% 29`.
fn line(document: &Document, parent: &HtmlElement) -> Result<(), String> {
    let line = block(
        document,
        parent,
        "left: 0; top: 28px; right: 0; height: 1px",
    )?;
    let _ = line.set_attribute("class", "block line");
    Ok(())
}

/// A disabled `StoneButton` of 24 x 24 px with `iconInfoWhite`, whose
/// tooltip explains.
fn help(document: &Document, parent: &HtmlElement, style: &str) -> Result<HtmlElement, String> {
    let help = block(
        document,
        parent,
        &format!("{style}; width: 24px; height: 24px"),
    )?;
    let _ = help.set_attribute("class", "block help stone");
    let _ = help.set_attribute(tooltip::STYLE_ATTRIBUTE, "sessionToolTipBold");
    Ok(help)
}
