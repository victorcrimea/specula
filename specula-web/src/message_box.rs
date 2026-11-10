//! The session's message box (`SessionMessageBox` and mod/gui/msgbox) in
//! the page's `#message-box`, as `DeleteSelectionConfirmation` opens it:
//! "Delete", No and Yes. The engine pauses a single player game under it
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
    /// The entities the open box deletes on Yes, as the selection was when
    /// it opened.
    deleting: Option<Vec<u32>>,
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
            deleting: None,
        };
        message_box.root.hidden(true);
        Ok(message_box)
    }

    pub fn buttons(&self) -> (&HtmlElement, &HtmlElement) {
        (&self.no, &self.yes)
    }

    pub fn is_open(&self) -> bool {
        self.deleting.is_some()
    }

    /// `DeleteSelectionConfirmation.display`; the labels are translated now,
    /// so a language change while it is closed needs nothing.
    pub fn confirm_delete(&mut self, entities: Vec<u32>) {
        self.title.text(&lang::translate("Delete"));
        self.caption
            .text(&lang::translate("Destroy everything currently selected?"));
        self.no_label.text(&lang::translate("No"));
        self.yes_label.text(&lang::translate("Yes"));
        self.deleting = Some(entities);
        self.root.hidden(false);
    }

    /// No, or the cancel hotkey (Escape).
    pub fn cancel(&mut self) {
        self.deleting = None;
        self.root.hidden(true);
    }

    /// Yes: what to delete.
    pub fn accept(&mut self) -> Option<Vec<u32>> {
        self.root.hidden(true);
        self.deleting.take()
    }
}
