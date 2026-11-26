//! One page per seat (D64): a seated cookie copied to a second computer
//! must not let a helper command or watch the same seat. Each `/play` page
//! names itself by a tab id it draws once; the newest page to open a seat
//! socket holds the seat, and every socket of an older page, or an older
//! socket of the same kind, is told to close.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use rocket::tokio::sync::oneshot;

/// The longest tab id accepted; the page sends 32 hex digits.
const TAB_MAX: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Command,
    View,
    Detail,
}

/// A page's own name for itself, as the page sent it.
pub fn valid_tab(tab: Option<&str>) -> Option<&str> {
    tab.filter(|tab| {
        !tab.is_empty() && tab.len() <= TAB_MAX && tab.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

struct Slot {
    serial: u64,
    close: oneshot::Sender<()>,
}

struct Holder {
    tab: String,
    slots: HashMap<Kind, Slot>,
}

#[derive(Default)]
struct Inner {
    next_serial: u64,
    holders: HashMap<String, Holder>,
}

/// A leaf lock: nothing else is locked while it is held.
#[derive(Default)]
pub struct Leases {
    inner: Mutex<Inner>,
}

/// Keeps a socket's slot while it lives. Its receiver fires once another
/// page or socket took the slot over.
pub struct Lease {
    leases: Arc<Leases>,
    cookie: String,
    kind: Kind,
    serial: u64,
    pub closed: oneshot::Receiver<()>,
}

impl Leases {
    /// Makes `tab` the page holding `cookie`'s seat and gives it the slot of
    /// `kind`, closing what held either before.
    pub fn claim(self: &Arc<Self>, cookie: &str, tab: &str, kind: Kind) -> Lease {
        let (close, closed) = oneshot::channel();
        let mut inner = self.inner.lock().unwrap();
        inner.next_serial += 1;
        let serial = inner.next_serial;
        let holder = inner
            .holders
            .entry(cookie.to_string())
            .or_insert_with(|| Holder {
                tab: tab.to_string(),
                slots: HashMap::new(),
            });
        if holder.tab != tab {
            holder.tab = tab.to_string();
            // A closed receiver only means that socket is already gone.
            for (_, slot) in holder.slots.drain() {
                let _ = slot.close.send(());
            }
        }
        if let Some(old) = holder.slots.insert(kind, Slot { serial, close }) {
            let _ = old.close.send(());
        }
        Lease {
            leases: Arc::clone(self),
            cookie: cookie.to_string(),
            kind,
            serial,
            closed,
        }
    }

    /// Whether `tab` is the page holding `cookie`'s seat. A cookie that no
    /// page has claimed yet passes, so an action sent before the sockets
    /// open still works; the next claim settles who holds it.
    pub fn holds(&self, cookie: &str, tab: Option<&str>) -> bool {
        let Some(tab) = valid_tab(tab) else {
            return false;
        };
        let inner = self.inner.lock().unwrap();
        inner
            .holders
            .get(cookie)
            .is_none_or(|holder| holder.tab == tab)
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut inner = self.leases.inner.lock().unwrap();
        let Some(holder) = inner.holders.get_mut(&self.cookie) else {
            return;
        };
        // A successor may have the slot already; only its own entry goes.
        if holder
            .slots
            .get(&self.kind)
            .is_some_and(|slot| slot.serial == self.serial)
        {
            holder.slots.remove(&self.kind);
        }
        // The holder's tab is kept when its sockets are gone, so an action
        // from a page that was displaced stays refused.
    }
}
