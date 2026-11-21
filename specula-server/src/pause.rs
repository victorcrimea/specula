//! The pause ledger: which human seats hold a pause, what each has left of
//! its pause budget, and the pauser set the engine is told (D41 to D43).
//! Pauses are unlimited in number but cost at least 5 s each, so a
//! pause-unpause cycle cannot be free; a mutual pause (every active seat
//! holds one and all are connected) costs nothing; a seat away from the
//! game pauses it at its own cost; and a budget that runs out ends that
//! seat's pause after a 5 s countdown.
//!
//! Charging is wall time and happens when the server polls, so a budget can
//! run out, and a countdown end, up to a poll late. Budgets are never
//! persisted: a server restart starts them afresh (D21).
//!
//! This is pure state: no locks and no I/O. `Play` takes inputs from the
//! seat table and the feed, advances the ledger, and writes the engine's
//! pauser set and the viewers' ledger from what it answers.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::time::Duration;
use std::time::Instant;

/// Each human seat's pause time (D41).
pub const BUDGET: Duration = Duration::from_secs(5 * 60);
/// The least a voluntary pause costs, however short (D41).
pub const MIN_CHARGE: Duration = Duration::from_secs(5);
/// How long a pause lasts after its budget ran out (D41).
pub const COUNTDOWN: Duration = Duration::from_secs(5);
/// How often a ledger with a running charge or countdown is resent, so a
/// viewer who joins mid-pause is at most this far off.
pub const RESEND: Duration = Duration::from_secs(1);
/// A voluntary pause, held by the seat itself.
pub const FLAG_HOLDS: u8 = 1;
/// No command socket open for long enough (M25's 5 s rule).
pub const FLAG_AWAY: u8 = 2;
/// The seat's budget is being spent now.
pub const FLAG_CHARGED: u8 = 4;

/// One seat's ledger line, as the stream carries it.
#[derive(Clone)]
pub struct Entry {
    pub player: u8,
    /// Pause time left as of the report; a charged one runs down in real
    /// time after it.
    pub budget_ms: u32,
    pub flags: u8,
    /// How long until the seat's pause ends after its budget ran out, 0 for
    /// none.
    pub countdown_ms: u32,
}

/// What `Play` knows of a seat, which the ledger does not.
pub struct Input {
    pub player: u8,
    pub connected: bool,
    pub active: bool,
}

/// A change worth a log line.
pub enum Event {
    Disconnected(u8),
    Reconnected(u8),
    Spent(u8),
}

pub struct Ledger {
    /// Human seats by player number.
    accounts: BTreeMap<u8, Account>,
    /// The last advance; None before the first.
    last: Option<Instant>,
    /// Hold order, for "earliest holder".
    next_order: u64,
    /// The pauser set last sent to the engine.
    engine: BTreeSet<u8>,
    /// Shape and time of the last report.
    reported: Option<(Shape, Instant)>,
}

/// A report's shape: player, flags and whether a countdown runs, per entry.
type Shape = Vec<(u8, u8, bool)>;

struct Account {
    budget: Duration,
    hold: Option<Hold>,
    /// Starts true; the launch has not heard from the pages yet.
    connected: bool,
    /// Starts true; the feed has not reported a defeat yet.
    active: bool,
    /// When the seat's pause ends after its budget ran out.
    countdown: Option<Instant>,
}

struct Hold {
    order: u64,
    /// What this hold has been charged so far, for `MIN_CHARGE`.
    charged: Duration,
    /// It spent a moment in a mutual pause, which costs nothing.
    mutual: bool,
}

impl Ledger {
    pub fn new(players: &[u8]) -> Ledger {
        Ledger {
            accounts: players
                .iter()
                .map(|player| {
                    (
                        *player,
                        Account {
                            budget: BUDGET,
                            hold: None,
                            connected: true,
                            active: true,
                            countdown: None,
                        },
                    )
                })
                .collect(),
            last: None,
            next_order: 0,
            engine: BTreeSet::new(),
            reported: None,
        }
    }

    /// Whether the account keeps the game paused: a hold of its own, or an
    /// automatic pause while it is away with budget left or until its
    /// countdown ends (D43).
    fn holds(&self, account: &Account) -> bool {
        account.active
            && (account.hold.is_some()
                || (!account.connected
                    && (!account.budget.is_zero() || account.countdown.is_some())))
    }

    /// Whether every active seat holds a pause and all are connected: no
    /// seat is charged then (D42).
    fn mutual(&self) -> bool {
        let mut any = false;
        for account in self.accounts.values().filter(|account| account.active) {
            any = true;
            if !account.connected || account.hold.is_none() {
                return false;
            }
        }
        any
    }

    /// The seats whose budget `advance` spends: every away seat with budget
    /// left, all at once, else the earliest voluntary holder.
    fn charged(&self) -> Vec<u8> {
        if self.mutual() {
            return Vec::new();
        }
        let away: Vec<u8> = self
            .accounts
            .iter()
            .filter(|(_, account)| {
                account.active && !account.connected && !account.budget.is_zero()
            })
            .map(|(player, _)| *player)
            .collect();
        if !away.is_empty() {
            return away;
        }
        let mut earliest: Option<(u64, u8)> = None;
        for (player, account) in &self.accounts {
            if !account.active || account.budget.is_zero() {
                continue;
            }
            let Some(hold) = &account.hold else {
                continue;
            };
            if earliest.is_none_or(|(order, _)| hold.order < order) {
                earliest = Some((hold.order, *player));
            }
        }
        earliest.map(|(_, player)| player).into_iter().collect()
    }

    /// The pause time a seat has left, None for a seat not in the ledger.
    pub fn budget(&self, player: u8) -> Option<Duration> {
        self.accounts.get(&player).map(|account| account.budget)
    }

    /// Charges for `elapsed`, then applies what the seat table and the feed
    /// now say.
    pub fn advance(&mut self, now: Instant, inputs: &[Input]) -> Vec<Event> {
        let elapsed = self
            .last
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        self.last = Some(now);
        let mut events = Vec::new();
        // Charged with the state of the last advance: the elapsed time
        // passed under it, so a seat that has just reconnected still pays
        // for that interval, and stops paying only from this advance on.
        if self.mutual() {
            for account in self.accounts.values_mut() {
                if let Some(hold) = &mut account.hold {
                    hold.mutual = true;
                }
            }
        } else {
            for player in self.charged() {
                let Some(account) = self.accounts.get_mut(&player) else {
                    continue;
                };
                account.budget = account.budget.saturating_sub(elapsed);
                if let Some(hold) = &mut account.hold {
                    hold.charged += elapsed;
                }
                if account.budget.is_zero() {
                    account.countdown = Some(now + COUNTDOWN);
                    events.push(Event::Spent(player));
                }
            }
        }
        for input in inputs {
            let Some(account) = self.accounts.get_mut(&input.player) else {
                continue;
            };
            if !input.active {
                if account.active {
                    account.active = false;
                    account.hold = None;
                    account.countdown = None;
                }
                continue;
            }
            account.active = true;
            if account.connected != input.connected {
                account.connected = input.connected;
                if input.connected {
                    events.push(Event::Reconnected(input.player));
                    // In an automatic pause the countdown belongs to the
                    // absence, which just ended; a voluntary hold keeps it.
                    if account.hold.is_none() {
                        account.countdown = None;
                    }
                } else {
                    events.push(Event::Disconnected(input.player));
                }
            }
        }
        for account in self.accounts.values_mut() {
            if account.countdown.is_some_and(|at| at <= now) {
                account.countdown = None;
                account.hold = None;
            }
        }
        events
    }

    /// A seat holds a pause of its own; a second press does nothing.
    pub fn hold(&mut self, player: u8) -> Result<(), &'static str> {
        let order = self.next_order;
        let Some(account) = self.accounts.get_mut(&player) else {
            return Err("your seat is out of the game");
        };
        if !account.active {
            return Err("your seat is out of the game");
        }
        if account.hold.is_some() {
            return Ok(());
        }
        if account.budget.is_zero() {
            return Err("no pause time left");
        }
        account.hold = Some(Hold {
            order,
            charged: Duration::ZERO,
            mutual: false,
        });
        self.next_order += 1;
        Ok(())
    }

    /// A seat lets go of its hold. The least charge applies to a pause that
    /// never shared a mutual pause; an automatic pause never gets it either
    /// way, as it has no hold to release.
    pub fn release(&mut self, player: u8) {
        let Some(account) = self.accounts.get_mut(&player) else {
            return;
        };
        let Some(hold) = account.hold.take() else {
            return;
        };
        if !hold.mutual {
            account.budget = account
                .budget
                .saturating_sub(MIN_CHARGE.saturating_sub(hold.charged));
        }
        account.countdown = None;
    }

    /// The pauser set changes the engine has not been told yet.
    pub fn engine_changes(&mut self) -> Vec<(u8, bool)> {
        let holders: BTreeSet<u8> = self
            .accounts
            .iter()
            .filter(|(_, account)| self.holds(account))
            .map(|(player, _)| *player)
            .collect();
        let mut changes = Vec::new();
        for player in self.engine.difference(&holders) {
            changes.push((*player, false));
        }
        for player in holders.difference(&self.engine) {
            changes.push((*player, true));
        }
        self.engine = holders;
        changes
    }

    /// The ledger to send, when the shape changed, a charged budget or a
    /// countdown needs its periodic refresh, or nothing was sent yet.
    pub fn report(&mut self, now: Instant) -> Option<Vec<Entry>> {
        let charged = self.charged();
        let mut entries = Vec::new();
        let mut shape = Vec::new();
        let mut resend = false;
        for (player, account) in &self.accounts {
            if !account.active {
                continue;
            }
            let mut flags = 0;
            if account.hold.is_some() {
                flags |= FLAG_HOLDS;
            }
            if !account.connected {
                flags |= FLAG_AWAY;
            }
            if charged.contains(player) {
                flags |= FLAG_CHARGED;
                resend = true;
            }
            let counting = account.countdown.is_some();
            if counting {
                resend = true;
            }
            let countdown_ms = account.countdown.map_or(0, |at| {
                at.saturating_duration_since(now)
                    .as_millis()
                    .min(u128::from(u32::MAX)) as u32
            });
            entries.push(Entry {
                player: *player,
                budget_ms: account.budget.as_millis().min(u128::from(u32::MAX)) as u32,
                flags,
                countdown_ms,
            });
            shape.push((*player, flags, counting));
        }
        let due = match &self.reported {
            None => true,
            Some((reported, at)) => {
                *reported != shape || (resend && now.saturating_duration_since(*at) >= RESEND)
            }
        };
        if !due {
            return None;
        }
        self.reported = Some((shape, now));
        Some(entries)
    }
}
