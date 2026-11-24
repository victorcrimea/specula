//! Who is in a match: every visitor who joined it, by cookie (D11), in join
//! order, with the name they go by and the player number they hold, if any.
//! A spectator in setup (D35) is a member without a player. The table is
//! the match's from the moment its setup opens (D13), so it is its own lock
//! rather than `Play`'s: lock order registry, `Play`'s pause order,
//! `Play`, `Seats`, `Hub`, with the pause ledger a leaf taken after it
//! (`play.rs`).
//!
//! A member who leaves keeps their entry, so the same cookie coming back is
//! the same member, keeps their place in the join order and their name, and
//! no other visitor can take the name meanwhile.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crate::nickname::Nickname;
use crate::nickname::suffix_duplicates;

/// A seat with no game connection for this long counts as disconnected
/// (D43); the setup gives a refresh longer (`setup::LEAVE_GRACE`).
pub const DISCONNECT_GRACE: Duration = Duration::from_secs(5);

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
    ready: Ready,
    /// Open setup sockets of this cookie.
    sockets: u32,
    /// When the last setup socket closed; a refresh reconnects within
    /// `LEAVE_GRACE` and keeps the seat.
    away_since: Option<Instant>,
    /// The seat held when they left the setup, for D37.
    last_player: Option<u8>,
    /// Open `play/command` sockets of this cookie; presence counts only
    /// these (M25).
    play_sockets: u32,
    /// When the last one closed, or the launch if none was open then.
    play_away_since: Option<Instant>,
    /// D54: the visitor may not join this match again.
    banned: bool,
    /// Bumped on every kick or ban, so an open setup socket can tell that
    /// its member was removed after it opened.
    kicks: u32,
}

/// Vanilla's ready states (`ReadyController.js`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    No = 0,
    Yes = 1,
    /// The host's, and a client's who pressed Ready twice.
    Stay = 2,
}

impl Ready {
    pub fn from_u8(state: u8) -> Option<Ready> {
        match state {
            0 => Some(Ready::No),
            1 => Some(Ready::Yes),
            2 => Some(Ready::Stay),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// One member as the chat's list shows them.
pub struct ChatMember {
    pub cookie: String,
    pub nickname: Nickname,
    pub player: Option<u8>,
    pub banned: bool,
}

/// One member as the setup page shows them.
pub struct MemberView {
    pub name: Nickname,
    pub player: Option<u8>,
    pub present: bool,
    pub connected: bool,
    pub ready: Ready,
}

/// Where a host transfer looks for the next host.
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
            None => table.push(cookie, nickname, true),
        };
        if table.host.is_none() {
            table.host = Some(index);
        }
        table.members[index].nickname.clone()
    }

    /// Adds `cookie` as a member without a player, for chat in a running
    /// match (M31), or finds them if they are one. Unlike `join` it never
    /// makes them host, never marks a returning member present (host
    /// transfer reads that), and keeps a returning member's name (D10 locks
    /// it at launch).
    pub fn join_observer(&self, cookie: &str, nickname: Nickname) -> (usize, Nickname) {
        let table = &mut *self.inner.lock().unwrap();
        let index = match table.index_of(cookie) {
            Some(index) => index,
            None => table.push(cookie, nickname, false),
        };
        (index, table.members[index].nickname.clone())
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
        let member = &mut table.members[index];
        if member.player != player {
            member.ready = Ready::No;
        }
        member.player = player;
        Ok(())
    }

    /// Marks `cookie` gone, and passes the host powers on if they held
    /// them. `active` says whether a player is still in the game.
    pub fn leave(&self, cookie: &str, phase: Phase, active: impl Fn(u8) -> bool) {
        let table = &mut *self.inner.lock().unwrap();
        let Some(index) = table.index_of(cookie) else {
            return;
        };
        table.members[index].present = false;
        if table.host == Some(index) {
            // `present` is a setup page flag that a return to a running match
            // never sets again, so only the setup phase may read it.
            table.host = table
                .members
                .iter()
                .enumerate()
                .position(|(other, member)| {
                    other != index
                        && match phase {
                            Phase::Setup => member.present,
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

    pub fn index_of(&self, cookie: &str) -> Option<usize> {
        self.inner.lock().unwrap().index_of(cookie)
    }

    /// Every member in join order with their cookie, for the chat's members
    /// list (M32).
    pub fn chat_members(&self) -> Vec<ChatMember> {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .map(|member| ChatMember {
                cookie: member.cookie.clone(),
                nickname: member.nickname.clone(),
                player: member.player,
                banned: member.banned,
            })
            .collect()
    }

    /// The host's member index, which the pages compare to their own.
    pub fn host_index(&self) -> Option<usize> {
        self.inner.lock().unwrap().host
    }

    /// Every seated member's cookie, name and player number (M35).
    pub fn seated_members(&self) -> Vec<(String, Nickname, u8)> {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .filter_map(|member| {
                Some((
                    member.cookie.clone(),
                    member.nickname.clone(),
                    member.player?,
                ))
            })
            .collect()
    }

    /// The host kicks `cookie` out of the setup, or bans them with it (D54):
    /// a kicked visitor does not get the seat back on a return. The host
    /// cannot be removed.
    pub fn kick(&self, cookie: &str, ban: bool) -> Result<(), &'static str> {
        let table = &mut *self.inner.lock().unwrap();
        let index = table.index_of(cookie).ok_or("not in this match")?;
        if table.host == Some(index) {
            return Err("the host cannot be kicked");
        }
        let member = &mut table.members[index];
        member.player = None;
        member.last_player = None;
        member.ready = Ready::No;
        member.present = false;
        member.kicks += 1;
        member.banned |= ban;
        Ok(())
    }

    /// Bans `cookie` from a running match's chat and seats (D54), where a
    /// seat is already handled by `Play::kick`.
    pub fn ban(&self, cookie: &str) {
        let table = &mut *self.inner.lock().unwrap();
        let Some(index) = table.index_of(cookie) else {
            return;
        };
        table.members[index].banned = true;
        table.members[index].kicks += 1;
    }

    pub fn is_banned(&self, cookie: &str) -> bool {
        let table = self.inner.lock().unwrap();
        table
            .index_of(cookie)
            .is_some_and(|index| table.members[index].banned)
    }

    /// How often `cookie` was kicked or banned, 0 for a non-member.
    pub fn kicks_of(&self, cookie: &str) -> u32 {
        let table = self.inner.lock().unwrap();
        table
            .index_of(cookie)
            .map_or(0, |index| table.members[index].kicks)
    }

    /// Whether `cookie` is a member who has not left the setup.
    pub fn is_present(&self, cookie: &str) -> bool {
        let table = self.inner.lock().unwrap();
        table
            .index_of(cookie)
            .is_some_and(|index| table.members[index].present)
    }

    /// Host operations name members by index; cookies never leave the
    /// server.
    pub fn cookie_at(&self, index: usize) -> Option<String> {
        let table = self.inner.lock().unwrap();
        Some(table.members.get(index)?.cookie.clone())
    }

    pub fn nickname_of(&self, cookie: &str) -> Option<Nickname> {
        let table = self.inner.lock().unwrap();
        Some(table.members[table.index_of(cookie)?].nickname.clone())
    }

    /// The member holding player `player`.
    pub fn cookie_of_player(&self, player: u8) -> Option<String> {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .find(|member| member.player == Some(player))
            .map(|member| member.cookie.clone())
    }

    /// Unseats every member holding a player number above `count`
    /// (`unassignInvalidPlayers`); answers whether any.
    pub fn unseat_beyond(&self, count: usize) -> bool {
        let table = &mut *self.inner.lock().unwrap();
        let mut any = false;
        for member in &mut table.members {
            if member
                .player
                .is_some_and(|player| usize::from(player) > count)
            {
                member.player = None;
                member.ready = Ready::No;
                any = true;
            }
        }
        any
    }

    /// A setup socket of `cookie` opened; answers whether they are a member
    /// (a visitor who has not joined has nothing to count).
    pub fn connect(&self, cookie: &str) -> bool {
        let table = &mut *self.inner.lock().unwrap();
        let Some(index) = table.index_of(cookie) else {
            return false;
        };
        let member = &mut table.members[index];
        member.sockets += 1;
        member.away_since = None;
        true
    }

    pub fn disconnect(&self, cookie: &str) {
        let table = &mut *self.inner.lock().unwrap();
        if let Some(index) = table.index_of(cookie) {
            let member = &mut table.members[index];
            member.sockets = member.sockets.saturating_sub(1);
            if member.sockets == 0 {
                member.away_since = Some(Instant::now());
            }
        }
    }

    /// Marks every member with no game socket away at the launch, so a seat
    /// whose page never opens counts as disconnected after the grace.
    pub fn start_presence(&self) {
        let now = Instant::now();
        let table = &mut *self.inner.lock().unwrap();
        for member in &mut table.members {
            if member.play_sockets == 0 {
                member.play_away_since = Some(now);
            }
        }
    }

    /// A `play/command` socket of `cookie` opened.
    pub fn connect_play(&self, cookie: &str) {
        let table = &mut *self.inner.lock().unwrap();
        if let Some(index) = table.index_of(cookie) {
            let member = &mut table.members[index];
            member.play_sockets += 1;
            member.play_away_since = None;
        }
    }

    pub fn disconnect_play(&self, cookie: &str) {
        let table = &mut *self.inner.lock().unwrap();
        if let Some(index) = table.index_of(cookie) {
            let member = &mut table.members[index];
            member.play_sockets = member.play_sockets.saturating_sub(1);
            if member.play_sockets == 0 {
                member.play_away_since = Some(Instant::now());
            }
        }
    }

    /// Every seated member and how long their `play/command` sockets have
    /// been closed: None while one is open (M25).
    pub fn away(&self) -> Vec<(u8, Option<Duration>)> {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .filter_map(|member| {
                let player = member.player?;
                let away = if member.play_sockets > 0 {
                    None
                } else {
                    Some(
                        member
                            .play_away_since
                            .map_or(Duration::ZERO, |since| since.elapsed()),
                    )
                };
                Some((player, away))
            })
            .collect()
    }

    /// The player number the host holds, if the host is seated.
    pub fn host_player(&self) -> Option<u8> {
        let table = self.inner.lock().unwrap();
        table.members[table.host?].player
    }

    /// Every seated member, and whether they are connected (D43): a socket
    /// is open, or the last one closed less than `grace` ago.
    pub fn presence(&self, grace: Duration) -> Vec<(u8, bool)> {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .filter_map(|member| {
                let player = member.player?;
                let connected = member.play_sockets > 0
                    || member
                        .play_away_since
                        .is_none_or(|since| since.elapsed() < grace);
                Some((player, connected))
            })
            .collect()
    }

    /// Members present with no setup socket for longer than `grace` (U2).
    pub fn due_to_leave(&self, grace: Duration) -> Vec<String> {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .filter(|member| {
                member.present
                    && member.sockets == 0
                    && member
                        .away_since
                        .is_some_and(|since| since.elapsed() >= grace)
            })
            .map(|member| member.cookie.clone())
            .collect()
    }

    /// A member leaves the setup: their seat is remembered for their return
    /// and freed. Answers whether they held one.
    pub fn leave_setup(&self, cookie: &str) -> bool {
        let seated = {
            let table = &mut *self.inner.lock().unwrap();
            let Some(index) = table.index_of(cookie) else {
                return false;
            };
            let member = &mut table.members[index];
            let seated = member.player.is_some();
            member.last_player = member.player;
            member.player = None;
            member.ready = Ready::No;
            seated
        };
        self.leave(cookie, Phase::Setup, |_| true);
        seated
    }

    /// A member coming back takes the seat they held if `free` still
    /// offers it and nobody holds it (D37).
    pub fn take_back(&self, cookie: &str, free: impl Fn(u8) -> bool) -> Option<u8> {
        let table = &mut *self.inner.lock().unwrap();
        let index = table.index_of(cookie)?;
        let held = table.members[index].last_player?;
        table.members[index].last_player = None;
        let taken = table
            .members
            .iter()
            .any(|member| member.player == Some(held));
        if taken || !free(held) {
            return None;
        }
        let member = &mut table.members[index];
        member.player = Some(held);
        member.ready = Ready::No;
        Some(held)
    }

    /// Two seated members trade seats.
    pub fn swap_players(&self, a: &str, b: &str) -> Result<(), &'static str> {
        let table = &mut *self.inner.lock().unwrap();
        let a = table.index_of(a).ok_or("not in this match")?;
        let b = table.index_of(b).ok_or("not in this match")?;
        let (first, second) = (table.members[a].player, table.members[b].player);
        if first.is_none() || second.is_none() {
            return Err("not seated");
        }
        table.members[a].player = second;
        table.members[b].player = first;
        Ok(())
    }

    /// Only a seated member readies, and not the host, whose ready stays
    /// `Stay` (`ReadyButton`'s constructor).
    pub fn set_ready(&self, cookie: &str, ready: Ready) -> Result<(), &'static str> {
        let table = &mut *self.inner.lock().unwrap();
        let index = table.index_of(cookie).ok_or("not in this match")?;
        if table.host == Some(index) {
            return Err("the host never readies");
        }
        let member = &mut table.members[index];
        if member.player.is_none() {
            return Err("not seated");
        }
        member.ready = ready;
        Ok(())
    }

    /// `CNetServerWorker::ClearAllPlayerReady`: Ready goes back to Not
    /// ready, Stay Ready stays.
    pub fn reset_ready(&self) {
        let table = &mut *self.inner.lock().unwrap();
        for member in &mut table.members {
            if member.ready == Ready::Yes {
                member.ready = Ready::No;
            }
        }
    }

    pub fn force_host_stay(&self) {
        let table = &mut *self.inner.lock().unwrap();
        if let Some(host) = table.host {
            table.members[host].ready = Ready::Stay;
        }
    }

    /// Every member in join order, and the host's index.
    pub fn snapshot(&self) -> (Vec<MemberView>, Option<usize>) {
        let table = self.inner.lock().unwrap();
        let views = table
            .members
            .iter()
            .map(|member| MemberView {
                name: member.nickname.clone(),
                player: member.player,
                present: member.present,
                connected: member.sockets > 0,
                ready: member.ready,
            })
            .collect();
        (views, table.host)
    }

    /// Seated members' player numbers and names, for the launch.
    pub fn seated_names(&self) -> Vec<(u8, Nickname)> {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .filter_map(|member| Some((member.player?, member.nickname.clone())))
            .collect()
    }

    /// Whether no seated member is Not ready. The host counts too, but is
    /// always Stay ready (`force_host_stay`).
    pub fn everyone_ready(&self) -> bool {
        let table = self.inner.lock().unwrap();
        table
            .members
            .iter()
            .all(|member| member.player.is_none() || member.ready != Ready::No)
    }
}

impl Table {
    fn index_of(&self, cookie: &str) -> Option<usize> {
        self.members
            .iter()
            .position(|member| member.cookie == cookie)
    }

    /// Adds a member with `nickname` suffixed against every earlier one
    /// (D10); answers its index.
    fn push(&mut self, cookie: &str, nickname: Nickname, present: bool) -> usize {
        let mut names: Vec<Nickname> = self
            .members
            .iter()
            .map(|member| member.nickname.clone())
            .collect();
        names.push(nickname);
        suffix_duplicates(&mut names);
        let nickname = names.pop().expect("just pushed");
        self.members.push(Member {
            cookie: cookie.to_string(),
            nickname,
            player: None,
            present,
            ready: Ready::No,
            sockets: 0,
            away_since: None,
            last_player: None,
            play_sockets: 0,
            play_away_since: None,
            banned: false,
            kicks: 0,
        });
        self.members.len() - 1
    }
}
