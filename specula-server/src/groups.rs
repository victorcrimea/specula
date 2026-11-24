//! The control groups of /play (vanilla's `EntityGroupsContainer`), kept
//! here instead of in the page so that they survive a reload and follow
//! the engine's renames and deaths without the page being told each one.
//! The page sends a group only when it saves one and reads the contents
//! this pushes.

use std::collections::HashMap;

use specula_assets::command::GROUP_COUNT;
use specula_assets::command::MAX_GROUP;

/// What a member is this turn, as the feed shows it.
pub enum Alive {
    /// In the world, of this player.
    Owned(i32),
    /// Inside a holder, so the feed lists it only there.
    Garrisoned,
    Gone,
}

/// A group's contents to send to the page that holds the seat.
#[derive(Clone)]
pub struct Push {
    pub cookie: String,
    pub seat: u8,
    pub group: u8,
    pub handles: Vec<u32>,
}

/// One group of a set, as `Store::snapshot` and `Store::all` report it.
pub struct Entry {
    pub cookie: String,
    pub seat: u8,
    pub group: u8,
    /// Feed ids.
    pub ids: Vec<u32>,
    pub version: u64,
}

#[derive(Default)]
struct Set {
    /// Feed ids, sorted and deduped.
    members: [Vec<u32>; GROUP_COUNT],
    /// The handles last sent, so only a change is sent again.
    pushed: [Vec<u32>; GROUP_COUNT],
    /// Bumped whenever `members` changes, so a push computed from an older
    /// snapshot does not overwrite a newer save.
    version: [u64; GROUP_COUNT],
}

impl Set {
    fn unused(&self) -> bool {
        self.members.iter().all(Vec::is_empty) && self.pushed.iter().all(Vec::is_empty)
    }
}

/// Keyed by the visitor's cookie and the seat's player, so the groups are
/// the person's and not the connection's.
#[derive(Default)]
pub struct Store {
    sets: HashMap<(String, u8), Set>,
}

impl Store {
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// Replaces a group; an empty `ids` clears it. Returns the groups that
    /// changed.
    pub fn save(
        &mut self,
        cookie: &str,
        seat: u8,
        group: u8,
        disjoint: bool,
        mut ids: Vec<u32>,
    ) -> Vec<u8> {
        ids.sort_unstable();
        ids.dedup();
        ids.truncate(MAX_GROUP);
        let key = (cookie.to_string(), seat);
        if !self.sets.contains_key(&key) && ids.is_empty() {
            return Vec::new();
        }
        let set = self.sets.entry(key.clone()).or_default();
        let mut changed = Vec::new();
        if disjoint {
            for (other, members) in set.members.iter_mut().enumerate() {
                if other == usize::from(group) {
                    continue;
                }
                let before = members.len();
                members.retain(|id| ids.binary_search(id).is_err());
                if members.len() != before {
                    set.version[other] += 1;
                    changed.push(other as u8);
                }
            }
        }
        let slot = usize::from(group);
        if set.members[slot] != ids {
            set.members[slot] = ids;
            set.version[slot] += 1;
            changed.push(group);
        }
        if set.unused() {
            self.sets.remove(&key);
        }
        changed
    }

    /// Follows the engine's renames and drops what left the seat.
    pub fn turn(&mut self, renames: &HashMap<u32, u32>, alive: impl Fn(u32) -> Alive) {
        for ((_, seat), set) in &mut self.sets {
            let seat = i32::from(*seat);
            for (slot, members) in set.members.iter_mut().enumerate() {
                if members.is_empty() {
                    continue;
                }
                let mut next: Vec<u32> = members
                    .iter()
                    .map(|id| renames.get(id).copied().unwrap_or(*id))
                    .filter(|id| match alive(*id) {
                        Alive::Owned(owner) => owner == seat,
                        Alive::Garrisoned => true,
                        Alive::Gone => false,
                    })
                    .collect();
                // Two members may have been renamed into one entity.
                next.sort_unstable();
                next.dedup();
                if next != *members {
                    *members = next;
                    set.version[slot] += 1;
                }
            }
        }
    }

    /// All groups of a set, empty ones too.
    pub fn snapshot(&self, cookie: &str, seat: u8) -> Vec<Entry> {
        let set = self.sets.get(&(cookie.to_string(), seat));
        (0..GROUP_COUNT)
            .map(|slot| Entry {
                cookie: cookie.to_string(),
                seat,
                group: slot as u8,
                ids: set.map_or_else(Vec::new, |set| set.members[slot].clone()),
                version: set.map_or(0, |set| set.version[slot]),
            })
            .collect()
    }

    /// The groups the next turn's push check has to look at.
    pub fn all(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        for ((cookie, seat), set) in &self.sets {
            for slot in 0..GROUP_COUNT {
                if set.members[slot].is_empty() && set.pushed[slot].is_empty() {
                    continue;
                }
                entries.push(Entry {
                    cookie: cookie.clone(),
                    seat: *seat,
                    group: slot as u8,
                    ids: set.members[slot].clone(),
                    version: set.version[slot],
                });
            }
        }
        entries
    }

    /// Records what was computed for `version` as sent, and says whether
    /// it differs from the last send. False when a newer change landed
    /// meanwhile; its own push follows.
    pub fn mark_pushed(
        &mut self,
        cookie: &str,
        seat: u8,
        group: u8,
        version: u64,
        handles: Vec<u32>,
    ) -> bool {
        let key = (cookie.to_string(), seat);
        let Some(set) = self.sets.get_mut(&key) else {
            return false;
        };
        let slot = usize::from(group);
        if set.version[slot] != version || set.pushed[slot] == handles {
            return false;
        }
        set.pushed[slot] = handles;
        if set.unused() {
            self.sets.remove(&key);
        }
        true
    }
}
