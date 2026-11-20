//! Who is in a match: every visitor who joined it, by cookie (D11), in join
//! order, with the name they go by and the player number they hold, if any.
//! A spectator in setup (D35) is a member without a player. The table is
//! the match's from the moment its setup opens (D13), so it is its own lock
//! rather than `Play`'s: lock order registry, `Play`, `Seats`, `Hub`.
//!
//! A member who leaves keeps their entry, so the same cookie coming back is
//! the same member, keeps their place in the join order and their name, and
//! no other visitor can take the name meanwhile.

use std::sync::Mutex;

use crate::nickname::Nickname;
use crate::nickname::suffix_duplicates;

pub struct Seats {
    inner: Mutex<Table>,
}

#[derive(Default)]
struct Table {
    /// Join order, earliest first; never shrinks.
    members: Vec<Member>,
    /// Index into `members`. None once nobody is left to take the powers.
    host: Option<usize>,
}

struct Member {
    cookie: String,
    /// Suffixed against every earlier member when they joined (D10).
    nickname: Nickname,
    player: Option<u8>,
    present: bool,
}

/// Where a host transfer looks for the next host.
// Built only by `leave`'s callers (M15, M25).
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub enum Phase {
    /// The earliest joiner still here, seated or not (D38).
    Setup,
    /// The earliest joiner still here who holds a seat still in the game
    /// (D46): a resigned or defeated player is out of it.
    Running,
}

impl Seats {
    pub fn new() -> Self {
        Seats {
            inner: Mutex::new(Table::default()),
        }
    }

    /// Adds `cookie` with `nickname`, or marks a returning member present
    /// again. Answers the name they go by in the match. The first member
    /// is the host; a member who joins a table with no host takes it, so an
    /// empty setup someone reopens has one.
    pub fn join(&self, cookie: &str, nickname: Nickname) -> Nickname {
        let table = &mut *self.inner.lock().unwrap();
        let index = match table.index_of(cookie) {
            Some(index) => {
                table.members[index].present = true;
                index
            }
            None => {
                let mut names: Vec<Nickname> = table
                    .members
                    .iter()
                    .map(|member| member.nickname.clone())
                    .collect();
                names.push(nickname);
                suffix_duplicates(&mut names);
                let nickname = names.pop().expect("just pushed");
                table.members.push(Member {
                    cookie: cookie.to_string(),
                    nickname,
                    player: None,
                    present: true,
                });
                table.members.len() - 1
            }
        };
        if table.host.is_none() {
            table.host = Some(index);
        }
        table.members[index].nickname.clone()
    }

    /// Gives `cookie` the seat of `player`, or makes them a spectator with
    /// None. One seat per member and one member per seat (D12).
    pub fn set_player(&self, cookie: &str, player: Option<u8>) -> Result<(), &'static str> {
        let table = &mut *self.inner.lock().unwrap();
        let index = table.index_of(cookie).ok_or("not in this match")?;
        if let Some(player) = player
            && table
                .members
                .iter()
                .enumerate()
                .any(|(other, member)| other != index && member.player == Some(player))
        {
            return Err("that slot is taken");
        }
        table.members[index].player = player;
        Ok(())
    }

    /// Marks `cookie` gone, and passes the host powers on if they held
    /// them. `active` says whether a player is still in the game.
    // Its callers come with the setup socket (M15) and presence (M25);
    // until then nobody leaves a match but by its end.
    #[allow(dead_code)]
    pub fn leave(&self, cookie: &str, phase: Phase, active: impl Fn(u8) -> bool) {
        let table = &mut *self.inner.lock().unwrap();
        let Some(index) = table.index_of(cookie) else {
            return;
        };
        table.members[index].present = false;
        if table.host == Some(index) {
            table.host = table.members.iter().position(|member| {
                member.present
                    && match phase {
                        Phase::Setup => true,
                        Phase::Running => member.player.is_some_and(&active),
                    }
            });
        }
    }

    /// The player number `cookie` holds.
    pub fn player_of(&self, cookie: &str) -> Option<u8> {
        let table = self.inner.lock().unwrap();
        table.members[table.index_of(cookie)?].player
    }

    pub fn is_host(&self, cookie: &str) -> bool {
        let table = self.inner.lock().unwrap();
        table.host.is_some() && table.host == table.index_of(cookie)
    }

    pub fn host(&self) -> Option<String> {
        let table = self.inner.lock().unwrap();
        Some(table.members[table.host?].cookie.clone())
    }

    /// Every held player number, in join order.
    pub fn players(&self) -> Vec<u8> {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .filter_map(|member| member.player)
            .collect()
    }
}

impl Table {
    fn index_of(&self, cookie: &str) -> Option<usize> {
        self.members
            .iter()
            .position(|member| member.cookie == cookie)
    }
}
