//! The session's message box (`SessionMessageBox` and mod/gui/msgbox) in
//! the page's `#message-box`, as `DeleteSelectionConfirmation`,
//! `ResignConfirmation` and `QuitConfirmationMenu` open it: a title, No and
//! Yes. The engine pauses a single player game under it
//! (`implicitPause`); the game of `/play` is a network game, which it does
//! not pause either.

use web_sys::Document;
use web_sys::HtmlElement;

use crate::lang;
use crate::panel::Out;

pub struct MessageBox {
    root: Out,
    title: Out,
    caption: Out,
    no: HtmlElement,
    yes: HtmlElement,
    no_label: Out,
    yes_label: Out,
    /// What the open box does on Yes.
    pending: Option<Pending>,
}

/// What a message box asks to confirm.
pub enum Pending {
    /// The entities to delete, as the selection was when it opened.
    Delete(Vec<u32>),
    Resign,
    Exit,
    /// The seat's game is decided: Continue watching (No, Escape) or View
    /// summary (Yes). `won` is the outcome.
    Result {
        won: bool,
    },
}

impl MessageBox {
    pub fn new(document: &Document) -> Result<Self, String> {
        let mut message_box = MessageBox {
            root: Out::find(document, "message-box")?,
            title: Out::find(document, "message-box-title")?,
            caption: Out::find(document, "message-box-caption")?,
            no: crate::element(document, "message-box-no")?,
            yes: crate::element(document, "message-box-yes")?,
            no_label: Out::find(document, "message-box-no-label")?,
            yes_label: Out::find(document, "message-box-yes-label")?,
            pending: None,
        };
        message_box.root.hidden(true);
        Ok(message_box)
    }

    pub fn buttons(&self) -> (&HtmlElement, &HtmlElement) {
        (&self.no, &self.yes)
    }

    pub fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    /// `DeleteSelectionConfirmation.display`; the labels are translated now,
    /// so a language change while it is closed needs nothing.
    pub fn confirm_delete(&mut self, entities: Vec<u32>) {
        self.open(
            "Delete",
            "Destroy everything currently selected?",
            Pending::Delete(entities),
        );
    }

    /// `ResignConfirmation.display`.
    pub fn confirm_resign(&mut self) {
        self.open(
            "Confirmation",
            "Are you sure you want to resign?",
            Pending::Resign,
        );
    }

    /// The Exit question, in `QuitConfirmationMenu.MultiplayerClient`'s
    /// words. Vanilla's single player game exits at once; here the game
    /// ends for every viewer of the broadcast too, so it asks first.
    pub fn confirm_exit(&mut self) {
        self.open(
            "Confirmation",
            "Are you sure you want to quit?",
            Pending::Exit,
        );
    }

    /// The original's end of game offer: no Quit, as Exit is in the menu.
    pub fn offer_result(&mut self, won: bool) {
        self.open_with(
            if won { "VICTORIOUS!" } else { "DEFEATED!" },
            if won {
                "You have won the battle!"
            } else {
                "You have been defeated\u{2026}"
            },
            ("Continue watching", "View summary"),
            Pending::Result { won },
        );
    }

    /// Whether the open box is the result offer.
    pub fn is_offer(&self) -> bool {
        matches!(self.pending, Some(Pending::Result { .. }))
    }

    fn open(&mut self, title: &str, caption: &str, pending: Pending) {
        self.open_with(title, caption, ("No", "Yes"), pending);
    }

    fn open_with(&mut self, title: &str, caption: &str, buttons: (&str, &str), pending: Pending) {
        self.title.text(&lang::translate(title));
        self.caption.text(&lang::translate(caption));
        self.no_label.text(&lang::translate(buttons.0));
        self.yes_label.text(&lang::translate(buttons.1));
        self.pending = Some(pending);
        self.root.hidden(false);
    }

    /// No, or the cancel hotkey (Escape): what was open.
    pub fn cancel(&mut self) -> Option<Pending> {
        self.root.hidden(true);
        self.pending.take()
    }

    /// Yes: what to do.
    pub fn accept(&mut self) -> Option<Pending> {
        self.root.hidden(true);
        self.pending.take()
    }
}
