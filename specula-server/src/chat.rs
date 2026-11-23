//! A match's chat (M31): one log from its setup's opening to its end. The
//! server relays it because a match has one engine client, while vanilla
//! chats between NetClients (D6). Channels and who may read them follow
//! vanilla's `ChatAddressees.js`, with the stances and states read from the
//! feed's latest players record at send time.
//!
//! A seated member's message while the game runs reaches an unseated viewer
//! only after the match's delay (D49): it can describe what the game shows
//! only after that delay, so watchers get it at the same moment.
//!
//! `Chat` is a leaf lock. Callers gather the seats, the play and the hub
//! facts first and only then call in, so nothing is held while another lock
//! is taken.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use rocket::tokio::sync::watch;
use serde_json::Value;
use serde_json::json;

use crate::limit::Limiter;
use crate::nickname::Nickname;
use crate::registry;
use crate::registry::MatchState;

/// Vanilla's chat input `max_length` (session and gamesetup).
pub const MAX_LENGTH: usize = 1024;
/// D52; vanilla has no rate limit.
pub const RATE_SCOPE: Duration = Duration::from_secs(5);
pub const RATE_PER_MEMBER: usize = 5;
/// Shared by every unseated sender of a match in game, since a visitor can
/// open more cookies (private windows) than a per-member limit sees.
pub const RATE_OBSERVERS: usize = 20;
/// The largest websocket message the chat socket reads: `MAX_LENGTH`
/// characters JSON-escaped as surrogate pairs (12 bytes each) and the
/// envelope, so a valid message always fits and a huge one is cut off
/// before it is buffered and parsed.
pub const MAX_FRAME: usize = 16 * 1024;
/// Entries kept for the backlog of a (re)connecting page.
pub const LOG_KEEP: usize = 500;
/// As `feed::Player::state` and `feed::Player::stances`.
const PLAYER_ACTIVE: u8 = 0;
pub(crate) const STANCE_ENEMY: u8 = 0;
const STANCE_ALLY: u8 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Everyone,
    Allies,
    Enemies,
    Observers,
    Private,
}

impl Channel {
    pub fn parse(name: &str) -> Option<Channel> {
        Some(match name {
            "everyone" => Channel::Everyone,
            "allies" => Channel::Allies,
            "enemies" => Channel::Enemies,
            "observers" => Channel::Observers,
            "private" => Channel::Private,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::Everyone => "everyone",
            Channel::Allies => "allies",
            Channel::Enemies => "enemies",
            Channel::Observers => "observers",
            Channel::Private => "private",
        }
    }
}

pub enum Request {
    Join(Nickname),
    /// A `join` whose nickname breaks the rule, answered so the page can say
    /// why it cannot send.
    BadNickname,
    Send {
        channel: Channel,
        /// A `Seats` member index; only `Private` reads it.
        to: Option<usize>,
        text: String,
    },
}

/// Who reads a socket. Re-read on every delivery, since a seat can be lost
/// (kick, M34) or a visitor can join.
pub struct Viewer {
    pub member: Option<usize>,
    pub player: Option<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Member(usize),
    /// Every unseated sender of a match in game.
    Observers,
}

struct Entry {
    seq: u64,
    sent: Instant,
    /// Sent by a seated member while the game ran: unseated viewers get it
    /// after the delay.
    delayed: bool,
    channel: Channel,
    from_member: usize,
    /// Player numbers that receive it (allies, enemies, observers' seats).
    players: Vec<u8>,
    /// Unseated viewers receive it (everyone, observers).
    observers: bool,
    to_member: Option<usize>,
    /// The message as sent to pages, built once.
    json: String,
}

struct Log {
    entries: VecDeque<Entry>,
    next_seq: u64,
    limiter: Limiter<Key>,
    closed: bool,
    /// Open chat sockets per cookie, for the members list's `online`.
    online: HashMap<String, u32>,
}

pub struct Chat {
    inner: Mutex<Log>,
    /// Bumped on every entry and on close; sockets then read what is new.
    revision: watch::Sender<u64>,
    delay: Duration,
}

impl Chat {
    pub fn new(delay: Duration) -> Chat {
        Chat {
            inner: Mutex::new(Log {
                entries: VecDeque::new(),
                next_seq: 0,
                limiter: Limiter::new(RATE_SCOPE),
                closed: false,
                online: HashMap::new(),
            }),
            revision: watch::channel(0).0,
            delay,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    /// `send_modify` works from any thread and needs no receiver.
    fn bump(&self) {
        self.revision.send_modify(|revision| *revision += 1);
    }

    /// The match is over for chat: its sockets end once what is held is due.
    pub fn close(&self) {
        self.inner.lock().unwrap().closed = true;
        self.bump();
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().unwrap().closed
    }

    /// Counts an open chat socket of `cookie` while it lives, so the others
    /// can list who is online (vanilla lists connected clients only).
    pub fn present(self: &Arc<Self>, cookie: Option<&str>) -> Presence {
        if let Some(cookie) = cookie {
            *self
                .inner
                .lock()
                .unwrap()
                .online
                .entry(cookie.to_string())
                .or_insert(0) += 1;
            self.bump();
        }
        Presence {
            chat: Arc::clone(self),
            cookie: cookie.map(str::to_string),
        }
    }

    fn is_online(&self, cookie: &str) -> bool {
        self.inner.lock().unwrap().online.contains_key(cookie)
    }

    /// Appends `entry` with `body` (its message, without `seq`) if the match
    /// is open and the limits allow it. A refused message spends no budget.
    fn post(
        &self,
        mut entry: Entry,
        mut body: Value,
        keys: &[(Key, usize)],
        now: Instant,
    ) -> Result<(), &'static str> {
        {
            let log = &mut *self.inner.lock().unwrap();
            if log.closed {
                return Err("the chat is closed");
            }
            if !log.limiter.allow(keys, now) {
                return Err("too many messages");
            }
            entry.seq = log.next_seq;
            log.next_seq += 1;
            body["seq"] = json!(entry.seq);
            entry.json = body.to_string();
            log.entries.push_back(entry);
            while log.entries.len() > LOG_KEEP {
                log.entries.pop_front();
            }
        }
        self.bump();
        Ok(())
    }

    /// The entries at or after `cursor` that `viewer` may read, each with the
    /// instant it is due, in order; advances `cursor` past the log.
    pub fn take(&self, cursor: &mut u64, viewer: &Viewer) -> Vec<(Instant, String)> {
        let log = self.inner.lock().unwrap();
        let mut out = Vec::new();
        for entry in log.entries.iter().filter(|entry| entry.seq >= *cursor) {
            if !visible(entry, viewer) {
                continue;
            }
            // The sender's own message is never held back, even after they
            // lost their seat.
            let delayed = entry.delayed
                && viewer.player.is_none()
                && viewer.member != Some(entry.from_member);
            let due = if delayed {
                entry.sent + self.delay
            } else {
                entry.sent
            };
            out.push((due, entry.json.clone()));
        }
        *cursor = log.next_seq;
        out
    }
}

/// Held by a chat socket for as long as it is open.
pub struct Presence {
    chat: Arc<Chat>,
    cookie: Option<String>,
}

impl Drop for Presence {
    fn drop(&mut self) {
        let Some(cookie) = self.cookie.take() else {
            return;
        };
        {
            let mut log = self.chat.inner.lock().unwrap();
            if let Some(count) = log.online.get_mut(&cookie) {
                *count -= 1;
                if *count == 0 {
                    log.online.remove(&cookie);
                }
            }
        }
        self.chat.bump();
    }
}

fn visible(entry: &Entry, viewer: &Viewer) -> bool {
    if viewer.member == Some(entry.from_member) {
        return true;
    }
    let player_receives = viewer
        .player
        .is_some_and(|player| entry.players.contains(&player));
    match entry.channel {
        Channel::Everyone => true,
        Channel::Allies | Channel::Enemies => player_receives,
        Channel::Observers => player_receives || (viewer.player.is_none() && entry.observers),
        Channel::Private => viewer.member == entry.to_member,
    }
}

/// The `you` message: who the reader is, if they have joined, and the
/// longest message the server takes.
pub fn you_json(found: &registry::Match, cookie: Option<&str>) -> String {
    let member = cookie.and_then(|cookie| found.seats.index_of(cookie));
    let name = cookie.and_then(|cookie| found.seats.nickname_of(cookie));
    json!({
        "type": "you",
        "member": member,
        "name": name.as_ref().map(Nickname::as_str),
        "max_length": MAX_LENGTH,
    })
    .to_string()
}

/// The `members` message for `cookie`'s socket: who they are and whom they
/// could address, with vanilla's observer test and presence.
pub fn members_json(found: &registry::Match, cookie: Option<&str>) -> String {
    let in_game = found.state() != MatchState::Setup;
    // Once per message: the hub's lock is not worth taking per member.
    let diplomacy = if in_game {
        found.hub.diplomacy()
    } else {
        Vec::new()
    };
    let members: Vec<Value> = found
        .seats
        .chat_members()
        .into_iter()
        .enumerate()
        .map(|(member, entry)| {
            let observer = match entry.player {
                None => true,
                Some(player) => in_game && !is_active(&diplomacy, player),
            };
            json!({
                "member": member,
                "name": entry.nickname.as_str(),
                "player": entry.player,
                "observer": observer,
                "online": found.chat.is_online(&entry.cookie),
                "banned": entry.banned,
            })
        })
        .collect();
    json!({
        "type": "members",
        "you": cookie.and_then(|cookie| found.seats.index_of(cookie)),
        "host": found.seats.host_index(),
        "members": members,
    })
    .to_string()
}

/// A socket's text, if it is a request.
pub fn parse(text: &str) -> Option<Request> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value["type"].as_str()? {
        "join" => Some(
            Nickname::parse(value["nickname"].as_str()?)
                .map_or(Request::BadNickname, Request::Join),
        ),
        "send" => {
            let channel = Channel::parse(value["channel"].as_str()?)?;
            let to = value["to"].as_u64().and_then(|to| usize::try_from(to).ok());
            let text = value["text"].as_str()?.to_string();
            Some(Request::Send { channel, to, text })
        }
        _ => None,
    }
}

/// Vanilla's input is one line, so a control character becomes a space
/// rather than a refusal. Escaping is the page's, as vanilla's `escapeText`.
fn clean(text: &str) -> Result<String, &'static str> {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if text.trim().is_empty() {
        return Err("empty message");
    }
    if text.chars().count() > MAX_LENGTH {
        return Err("message too long");
    }
    Ok(text)
}

/// A seat the feed has not reported yet (setup, loading) is active, or its
/// player would be refused Allies and marked an observer until the first
/// players record.
fn is_active(diplomacy: &[(u8, Vec<u8>)], player: u8) -> bool {
    diplomacy
        .get(usize::from(player))
        .is_none_or(|(state, _)| *state == PLAYER_ACTIVE)
}

fn stance(diplomacy: &[(u8, Vec<u8>)], from: u8, to: u8) -> Option<u8> {
    diplomacy
        .get(usize::from(from))?
        .1
        .get(usize::from(to))
        .copied()
}

/// `Diplomacy.IsMutualAlly`: gaia is nobody's ally.
fn mutual_ally(diplomacy: &[(u8, Vec<u8>)], a: u8, b: u8) -> bool {
    a != 0
        && b != 0
        && a != b
        && stance(diplomacy, a, b) == Some(STANCE_ALLY)
        && stance(diplomacy, b, a) == Some(STANCE_ALLY)
}

/// Every player number but gaia's, which the diplomacy holds by index.
fn players_of(diplomacy: &[(u8, Vec<u8>)]) -> impl Iterator<Item = u8> + '_ {
    (1..diplomacy.len()).filter_map(|index| u8::try_from(index).ok())
}

/// Who reads a socket of `found`: its member and seat, if any.
pub fn viewer(found: &registry::Match, cookie: Option<&str>) -> Viewer {
    Viewer {
        member: cookie.and_then(|cookie| found.seats.index_of(cookie)),
        player: found.play.seat(cookie),
    }
}

/// Makes `cookie` a member of a running match (M31); the setup page's own
/// join takes the setup's seats, so this refuses there.
pub fn join(
    found: &registry::Match,
    cookie: &str,
    nickname: Nickname,
) -> Result<(usize, Nickname), &'static str> {
    if found.state() == MatchState::Setup {
        return Err("join through the setup");
    }
    Ok(found.seats.join_observer(cookie, nickname))
}

/// What a visitor's request is answered with, if it is: `you` once a join
/// works, an error text for a refused join or message.
pub fn answer(found: &registry::Match, cookie: Option<&str>, request: Request) -> Option<String> {
    let refused = |reason: &str| json!({ "type": "error", "text": reason }).to_string();
    match request {
        Request::BadNickname => Some(refused("invalid nickname")),
        Request::Join(nickname) => Some(match cookie {
            None => refused("no visitor cookie"),
            Some(cookie) => match join(found, cookie, nickname) {
                Ok(_) => you_json(found, Some(cookie)),
                Err(reason) => refused(reason),
            },
        }),
        Request::Send { channel, to, text } => {
            let sent = match cookie {
                None => Err("no visitor cookie"),
                Some(cookie) => send(found, cookie, channel, to, &text),
            };
            sent.err().map(refused)
        }
    }
}

/// Posts a message from `cookie` on `channel`, to `to` for a private one,
/// with the recipients fixed now (vanilla's `getReceiverGUIDs`).
pub fn send(
    found: &registry::Match,
    cookie: &str,
    channel: Channel,
    to: Option<usize>,
    text: &str,
) -> Result<(), &'static str> {
    let text = clean(text)?;
    if found.seats.is_banned(cookie) {
        return Err("you are banned from this match");
    }
    let member = found.seats.index_of(cookie).ok_or("join first")?;
    let name = found.seats.nickname_of(cookie).ok_or("join first")?;
    let in_game = found.state() != MatchState::Setup;
    if !in_game && channel != Channel::Everyone {
        return Err("only Everyone in setup");
    }
    let player = found.play.seat(Some(cookie));
    let diplomacy = if in_game {
        found.hub.diplomacy()
    } else {
        Vec::new()
    };
    let active = player.filter(|player| is_active(&diplomacy, *player));

    let (players, observers, to) = match channel {
        Channel::Everyone => (Vec::new(), true, None),
        Channel::Allies => {
            let Some(sender) = active else {
                return Err("observers cannot use that channel");
            };
            let players: Vec<u8> = players_of(&diplomacy)
                .filter(|other| *other != sender && mutual_ally(&diplomacy, sender, *other))
                .collect();
            (players, false, None)
        }
        Channel::Enemies => {
            let Some(sender) = active else {
                return Err("observers cannot use that channel");
            };
            let players: Vec<u8> = players_of(&diplomacy)
                .filter(|other| {
                    *other != sender && stance(&diplomacy, sender, *other) == Some(STANCE_ENEMY)
                })
                .collect();
            (players, false, None)
        }
        Channel::Observers => {
            let players: Vec<u8> = players_of(&diplomacy)
                .filter(|other| !is_active(&diplomacy, *other))
                .collect();
            (players, true, None)
        }
        Channel::Private => {
            let to = to.ok_or("no addressee")?;
            if to == member {
                return Err("no addressee");
            }
            let target = found.seats.cookie_at(to).ok_or("no such member")?;
            let target_name = found.seats.nickname_of(&target).ok_or("no such member")?;
            // An observer's private message reaches only observers (D48).
            if in_game && active.is_none() {
                let target_active = found
                    .seats
                    .player_of(&target)
                    .is_some_and(|other| is_active(&diplomacy, other));
                if target_active {
                    return Err("observers cannot message players");
                }
            }
            (Vec::new(), false, Some((to, target_name)))
        }
    };

    let delayed = in_game && player.is_some();
    let mut keys = vec![(Key::Member(member), RATE_PER_MEMBER)];
    if in_game && player.is_none() {
        keys.push((Key::Observers, RATE_OBSERVERS));
    }

    let mut body = json!({
        "type": "chat",
        "channel": channel.name(),
        "from": {
            "member": member,
            "name": name.as_str(),
            "player": player,
            "observer": active.is_none(),
        },
        "text": text,
        // Pages stamp the time from it, so a backlog shows when the message
        // was written rather than when the page received it.
        "sent_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64),
        "turn": in_game.then(|| found.hub.turn()),
        "setup": !in_game,
    });
    if let Some((to, target_name)) = &to {
        body["to"] = json!({ "member": to, "name": target_name.as_str() });
    }

    let entry = Entry {
        seq: 0,
        sent: Instant::now(),
        delayed,
        channel,
        from_member: member,
        players,
        observers,
        to_member: to.map(|(to, _)| to),
        json: String::new(),
    };
    let now = entry.sent;
    found.chat.post(entry, body, &keys, now)
}
