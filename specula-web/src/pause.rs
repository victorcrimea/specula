//! The session's pause overlay (`session/PauseOverlay.js`, `.xml`): while
//! any player has the game paused, a dark veil over the world with "Game
//! Paused", for the player who paused it "Click to Resume Game", and who
//! paused. It sits below the panels in the page, so only the world is
//! blocked.

use web_sys::Document;
use web_sys::HtmlElement;

use crate::lang;
use crate::panel;
use crate::stream::Player;

pub struct PauseOverlay {
    root: HtmlElement,
    resume: HtmlElement,
    by: HtmlElement,
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
            document: document.clone(),
            key: String::new(),
        })
    }

    pub fn root(&self) -> &HtmlElement {
        &self.root
    }

    /// `rebuild`: `own` is the viewer's seat, whose pause a click undoes.
    pub fn update(&mut self, paused_by: &[u32], players: &[Player], own: Option<usize>) {
        let own_paused = own.is_some_and(|own| paused_by.contains(&(own as u32)));
        let mut key = format!("{}", u8::from(own_paused));
        for player in paused_by {
            let (name, color) = players
                .get(*player as usize)
                .map_or(("", 0), |player| (player.name.as_str(), player.color));
            key.push_str(&format!("|{player}:{name}:{color:x}"));
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
            let Ok(span) = self.document.create_element("span") else {
                continue;
            };
            span.set_text_content(Some(&player.name));
            let _ = span.set_attribute(
                "style",
                &format!("color: {}", panel::css_color(player.color, 1.0)),
            );
            let _ = self.by.append_child(&span);
        }
        let _ = self.by.append_with_str_1(after);
    }
}
