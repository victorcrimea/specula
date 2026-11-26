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
    /// Vanilla's ReturnQuestion, after Exit: Yes resigns, No (I will return)
    /// goes to the observers' view.
    ReturnQuestion,
    /// The host's End game: Yes ends it for everyone.
    EndGame,
    /// Claim victory now: Yes resigns the seats that have left.
    Claim,
    /// The popup that offers the claim the first time it is available.
    ClaimOffer,
    /// Kick the human seat of this player number.
    Kick(u8),
    /// Ban the kicked human seat of this player number for good (D54).
    Ban(u8),
    /// Ban the observer member (D54).
    BanMember(usize),
    /// The seat was kicked; the box only says so, and OK reloads the page.
    Kicked,
    /// Another page took the seat over (D64); the one button reloads this
    /// page, which takes it back.
    Displaced,
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
        self.open_text(
            &lang::translate(title),
            &lang::translate(caption),
            (&lang::translate(buttons.0), &lang::translate(buttons.1)),
            pending,
        );
    }

    /// The same, for a caption that is already translated and formatted.
    fn open_text(&mut self, title: &str, caption: &str, buttons: (&str, &str), pending: Pending) {
        self.title.text(title);
        self.caption.text(caption);
        self.no_label.text(buttons.0);
        self.yes_label.text(buttons.1);
        self.set_single(false);
        self.pending = Some(pending);
        self.root.hidden(false);
    }

    /// Hides the No button, for a box with one answer.
    fn set_single(&self, single: bool) {
        self.no.set_hidden(single);
    }

    /// Vanilla's ReturnQuestion, after the Exit question: "I will return"
    /// (No) leaves for the observers' view, "I resign" (Yes) resigns.
    pub fn confirm_return(&mut self) {
        self.open_with(
            "Confirmation",
            "Do you want to resign or will you return soon?",
            ("I will return", "I resign"),
            Pending::ReturnQuestion,
        );
    }

    /// The host's End game, which ends the game for every player.
    pub fn confirm_end_game(&mut self) {
        self.open(
            "Confirmation",
            "Are you sure you want to end the game for every player?",
            Pending::EndGame,
        );
    }

    /// The claim, naming the seats it resigns; `names` is their list.
    pub fn confirm_claim(&mut self, names: &str) {
        let caption = lang::format(
            &lang::translate("Claim victory? %(players)s will be resigned."),
            &[("players", names)],
        );
        self.open_text(
            &lang::translate("Claim victory"),
            &caption,
            (&lang::translate("No"), &lang::translate("Yes")),
            Pending::Claim,
        );
    }

    /// The popup that a claim is now available; `names` are the seats that
    /// left.
    pub fn offer_claim(&mut self, names: &str) {
        let caption = lang::format(
            &lang::translate("%(players)s left the game. Do you want to claim victory?"),
            &[("players", names)],
        );
        self.open_text(
            &lang::translate("Claim victory"),
            &caption,
            (&lang::translate("Wait"), &lang::translate("Claim victory")),
            Pending::ClaimOffer,
        );
    }

    /// Kicking the human seat of `player`, named `name`.
    pub fn confirm_kick(&mut self, player: u8, name: &str) {
        let caption = lang::format(
            &lang::translate("Kick %(player)s? They cannot play this match again."),
            &[("player", name)],
        );
        self.open_text(
            &lang::translate("Confirmation"),
            &caption,
            (&lang::translate("No"), &lang::translate("Yes")),
            Pending::Kick(player),
        );
    }

    /// Banning player `player`, named `name` (D54).
    pub fn confirm_ban(&mut self, player: u8, name: &str) {
        let caption = lang::format(
            &lang::translate("Ban %(player)s? They cannot play or chat in this match again."),
            &[("player", name)],
        );
        self.open_text(
            &lang::translate("Confirmation"),
            &caption,
            (&lang::translate("No"), &lang::translate("Yes")),
            Pending::Ban(player),
        );
    }

    /// Banning observer `member`, named `name`, from the match's chat
    /// (D54).
    pub fn confirm_ban_member(&mut self, member: usize, name: &str) {
        let caption = lang::format(
            &lang::translate("Ban %(player)s from this match's chat?"),
            &[("player", name)],
        );
        self.open_text(
            &lang::translate("Confirmation"),
            &caption,
            (&lang::translate("No"), &lang::translate("Yes")),
            Pending::BanMember(member),
        );
    }

    /// The seat was kicked: one answer, which reloads the page.
    pub fn notify_kicked(&mut self) {
        self.open_text(
            &lang::translate("Confirmation"),
            &lang::translate("You have been kicked."),
            (&lang::translate("OK"), &lang::translate("OK")),
            Pending::Kicked,
        );
        self.set_single(true);
    }

    /// Another page holds the seat now (D64): one answer, which reloads
    /// the page and so takes the seat back.
    pub fn notify_displaced(&mut self) {
        let label = lang::translate("Take control here");
        self.open_text(
            &lang::translate("Controlled elsewhere"),
            &lang::translate("This seat is now being controlled from another window or computer."),
            (&label, &label),
            Pending::Displaced,
        );
        self.set_single(true);
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
