//! The session's pause overlay (`session/PauseOverlay.js`, `.xml`): while
//! any player has the game paused, a dark veil over the world with "Game
//! Paused", for the player who paused it "Click to Resume Game", and who
//! paused. It also lists each player's pause time left, and counts down a
//! seat whose budget ran out, which vanilla has not: budgets are D41's.
//! It sits below the panels in the page, so only the world is blocked.

use web_sys::Document;
use web_sys::Element;
use web_sys::HtmlElement;

use crate::lang;
use crate::panel;
use crate::stream::LedgerEntry;
use crate::stream::Player;

pub struct PauseOverlay {
    root: HtmlElement,
    resume: HtmlElement,
    by: HtmlElement,
    countdown: HtmlElement,
    budgets: HtmlElement,
    document: Document,
    /// What the overlay was last built for, so a turn that changes nothing
    /// leaves the page alone.
    key: String,
}

impl PauseOverlay {
    pub fn new(document: &Document) -> Result<Self, String> {
        Ok(PauseOverlay {
            root: crate::element(document, "pause-overlay")?,
            resume: crate::element(document, "pause-resume")?,
            by: crate::element(document, "pause-by")?,
            countdown: crate::element(document, "pause-countdown")?,
            budgets: crate::element(document, "pause-budgets")?,
            document: document.clone(),
            key: String::new(),
        })
    }

    pub fn root(&self) -> &HtmlElement {
        &self.root
    }

    /// The player's name in their colour, as the "Paused by" line shows it.
    fn player_span(&self, player: &Player) -> Option<Element> {
        let span = self.document.create_element("span").ok()?;
        span.set_text_content(Some(&player.name));
        let _ = span.set_attribute(
            "style",
            &format!("color: {}", panel::css_color(player.color, 1.0)),
        );
        Some(span)
    }

    /// `elapsed_ms`: how long after the ledger the page now is, which a
    /// charged budget and a countdown run down by.
    pub fn update(
        &mut self,
        paused_by: &[u32],
        players: &[Player],
        own: Option<usize>,
        ledger: &[LedgerEntry],
        elapsed_ms: f64,
    ) {
        let own_paused = own.is_some_and(|own| paused_by.contains(&(own as u32)));
        let mut key = format!("{}", u8::from(own_paused));
        for player in paused_by {
            let (name, color) = players
                .get(*player as usize)
                .map_or(("", 0), |player| (player.name.as_str(), player.color));
            key.push_str(&format!("|{player}:{name}:{color:x}"));
        }
        // Whole seconds make the list rebuild at most once a second.
        for entry in ledger {
            let (budget, countdown) = crate::ledger_left(entry, elapsed_ms);
            key.push_str(&format!(
                "|{}:{}:{}:{}",
                entry.player,
                entry.flags,
                (budget / 1000.0).ceil() as u64,
                (countdown / 1000.0).ceil() as u64,
            ));
        }
        if key == self.key {
            return;
        }
        self.key = key;
        self.root.set_hidden(paused_by.is_empty());
        self.resume.set_hidden(!own_paused);
        self.by.set_text_content(None);
        let template = lang::translate("Paused by %(players)s");
        let (before, after) = template
            .split_once("%(players)s")
            .unwrap_or((template.as_str(), ""));
        let separator = lang::translate_context("Separator for a list of players", ", ");
        let _ = self.by.append_with_str_1(before);
        for (index, player) in paused_by.iter().enumerate() {
            if index > 0 {
                let _ = self.by.append_with_str_1(&separator);
            }
            let Some(player) = players.get(*player as usize) else {
                continue;
            };
            if let Some(span) = self.player_span(player) {
                let _ = self.by.append_child(&span);
            }
        }
        let _ = self.by.append_with_str_1(after);
        self.update_countdown(players, ledger, elapsed_ms);
        self.update_budgets(players, ledger, elapsed_ms);
    }

    fn update_countdown(&mut self, players: &[Player], ledger: &[LedgerEntry], elapsed_ms: f64) {
        self.countdown.set_text_content(None);
        let mut any = false;
        for entry in ledger {
            let (_, countdown) = crate::ledger_left(entry, elapsed_ms);
            if countdown <= 0.0 {
                continue;
            }
            let Some(player) = players.get(entry.player as usize) else {
                continue;
            };
            let Ok(line) = self.document.create_element("div") else {
                continue;
            };
            any = true;
            let template =
                lang::translate("%(player)s is out of pause time; their pause ends in %(time)s.");
            let (before, rest) = template
                .split_once("%(player)s")
                .unwrap_or((template.as_str(), ""));
            let (middle, after) = rest.split_once("%(time)s").unwrap_or((rest, ""));
            let _ = line.append_with_str_1(before);
            if let Some(span) = self.player_span(player) {
                let _ = line.append_child(&span);
            }
            let _ = line.append_with_str_1(middle);
            let _ =
                line.append_with_str_1(&crate::format_minutes((countdown / 1000.0).ceil() as u64));
            let _ = line.append_with_str_1(after);
            let _ = self.countdown.append_child(&line);
        }
        self.countdown.set_hidden(!any);
    }

    fn update_budgets(&mut self, players: &[Player], ledger: &[LedgerEntry], elapsed_ms: f64) {
        self.budgets.set_text_content(None);
        self.budgets.set_hidden(ledger.is_empty());
        if ledger.is_empty() {
            return;
        }
        if let Ok(header) = self.document.create_element("div") {
            header.set_text_content(Some(&lang::translate("Pause time left")));
            let _ = self.budgets.append_child(&header);
        }
        for entry in ledger {
            let Some(player) = players.get(entry.player as usize) else {
                continue;
            };
            let Ok(line) = self.document.create_element("div") else {
                continue;
            };
            if entry.flags & crate::stream::LEDGER_CHARGED != 0 {
                let _ = line.set_attribute("class", "charged");
            }
            let (budget, _) = crate::ledger_left(entry, elapsed_ms);
            let time = crate::format_minutes((budget / 1000.0).ceil() as u64);
            let text = if entry.flags & crate::stream::LEDGER_AWAY != 0 {
                lang::format(
                    &lang::translate("%(player)s (disconnected): %(time)s"),
                    &[("player", &player.name), ("time", &time)],
                )
            } else {
                lang::format(
                    &lang::translate("%(player)s: %(time)s"),
                    &[("player", &player.name), ("time", &time)],
                )
            };
            line.set_text_content(Some(&text));
            let _ = self.budgets.append_child(&line);
        }
        if mutual(ledger)
            && let Ok(line) = self.document.create_element("div")
        {
            line.set_text_content(Some(&lang::translate("Mutual pause: nobody is charged.")));
            let _ = self.budgets.append_child(&line);
        }
    }
}

/// The server's mutual pause (D42) as the ledger shows it: every connected
/// seat holds a pause, and at least one does; away seats do not break it.
fn mutual(ledger: &[LedgerEntry]) -> bool {
    let mut any = false;
    for entry in ledger {
        if entry.flags & crate::stream::LEDGER_AWAY != 0 {
            continue;
        }
        if entry.flags & crate::stream::LEDGER_HOLDS == 0 {
            return false;
        }
        any = true;
    }
    any
}
