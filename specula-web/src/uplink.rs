//! `/play`'s orders to the server (`specula_assets::command`), over the
//! `/play/command` WebSocket, which the page holds itself: orders are page
//! events and few, so they need not pass through the stream worker. The
//! server's first message names the player the visitor commands. An order naming a selection the
//! socket sent before names it by a per-socket token instead.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;

use specula_assets::command::COMMAND_VERSION;
use specula_assets::command::Command;
use specula_assets::command::GROUP_TAG;
use specula_assets::command::GroupContents;
use specula_assets::command::GroupSave;
use specula_assets::command::Tokened;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::BinaryType;
use web_sys::MessageEvent;
use web_sys::WebSocket;

/// The pause before reconnecting, doubled after each failure, as the
/// server may be restarting.
const RECONNECT_MS: i32 = 1000;
const RECONNECT_MAX_MS: i32 = 30_000;

/// Below this the tag and token cost about as much as the list.
const MIN_TOKEN_LIST: usize = 4;

pub struct Uplink {
    inner: Rc<RefCell<Inner>>,
}

/// The list the server keeps for this socket's token, as handles,
/// sorted and deduped.
struct Sent {
    token: u32,
    handles: Vec<u32>,
}

struct Inner {
    socket: Option<WebSocket>,
    /// The player commanded, 0 for none (yet).
    seat: Rc<Cell<u8>>,
    /// The map's starting camera of the seat, x and z, when it has one.
    starting_camera: Rc<Cell<Option<[f32; 2]>>>,
    /// What the server keeps under this socket's token.
    sent: Option<Sent>,
    next_token: u32,
    /// The control groups the server pushed, as they came.
    groups: Rc<RefCell<Vec<GroupContents>>>,
    reconnect_ms: i32,
    /// Kept as long as their socket.
    _handlers: Option<Handlers>,
}

struct Handlers {
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut()>,
}

impl Uplink {
    pub fn open() -> Uplink {
        let inner = Rc::new(RefCell::new(Inner {
            socket: None,
            seat: Rc::new(Cell::new(0)),
            starting_camera: Rc::new(Cell::new(None)),
            sent: None,
            next_token: 1,
            groups: Rc::new(RefCell::new(Vec::new())),
            reconnect_ms: RECONNECT_MS,
            _handlers: None,
        }));
        connect(&Rc::downgrade(&inner));
        Uplink { inner }
    }

    /// The player the visitor commands, once the server has said.
    pub fn seat(&self) -> Option<i32> {
        match self.inner.borrow().seat.get() {
            0 => None,
            seat => Some(i32::from(seat)),
        }
    }

    pub fn starting_camera(&self) -> Option<[f32; 2]> {
        self.inner.borrow().starting_camera.get()
    }

    /// Dropped while the socket is not open: an order is worth giving only
    /// now.
    pub fn send(&self, command: &Command) {
        let mut inner = self.inner.borrow_mut();
        let Some(socket) = inner.socket.clone() else {
            return;
        };
        if socket.ready_state() == WebSocket::OPEN && inner.seat.get() != 0 {
            let inner = &mut *inner;
            let bytes = wire(command, &mut inner.sent, &mut inner.next_token);
            if socket.send_with_u8_array(&bytes).is_err() {
                // The server may never have got the list.
                inner.sent = None;
            }
        }
    }

    /// Dropped while the socket is not open, as `send`.
    pub fn save_group(&self, save: &GroupSave) {
        let inner = self.inner.borrow();
        let Some(socket) = &inner.socket else {
            return;
        };
        if socket.ready_state() == WebSocket::OPEN && inner.seat.get() != 0 {
            let _ = socket.send_with_u8_array(&save.encode());
        }
    }

    /// The control group contents that came since the last call.
    pub fn take_groups(&self) -> Vec<GroupContents> {
        std::mem::take(&mut *self.inner.borrow().groups.borrow_mut())
    }
}

/// The bytes of `command`, naming its selection by the token when that
/// is cheaper; updates what the server will keep.
fn wire(command: &Command, sent: &mut Option<Sent>, next_token: &mut u32) -> Vec<u8> {
    let Some(selection) = command.selection() else {
        return command.encode();
    };
    let mut list = selection.clone();
    list.sort_unstable();
    list.dedup();
    if list.len() < MIN_TOKEN_LIST {
        return command.encode();
    }
    if let Some(kept) = sent {
        let subset = list.iter().all(|h| kept.handles.binary_search(h).is_ok());
        if subset {
            let removed: Vec<u32> = kept
                .handles
                .iter()
                .copied()
                .filter(|h| list.binary_search(h).is_err())
                .collect();
            if removed.len() <= list.len() {
                let mut bare = command.clone();
                if let Some(selection) = bare.selection_mut() {
                    selection.clear();
                }
                kept.handles = list;
                return Tokened::Use {
                    token: kept.token,
                    removed,
                    command: bare,
                }
                .encode();
            }
        }
    }
    let token = *next_token;
    *next_token = next_token.wrapping_add(1).max(1);
    *sent = Some(Sent {
        token,
        handles: list,
    });
    Tokened::Define {
        token,
        command: command.clone(),
    }
    .encode()
}

fn connect(inner: &Weak<RefCell<Inner>>) {
    let Some(strong) = inner.upgrade() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let location = window.location();
    let scheme = match location.protocol().as_deref() {
        Ok("https:") => "wss",
        _ => "ws",
    };
    let url = format!(
        "{scheme}://{}{}/play/command",
        location.host().unwrap_or_default(),
        crate::match_base()
    );
    let Ok(socket) = WebSocket::new(&url) else {
        web_sys::console::error_1(&format!("cannot open {url}").into());
        return;
    };
    socket.set_binary_type(BinaryType::Arraybuffer);
    let seat = Rc::clone(&strong.borrow().seat);
    let starting_camera = Rc::clone(&strong.borrow().starting_camera);
    let groups = Rc::clone(&strong.borrow().groups);
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
            return;
        };
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        match bytes.as_slice() {
            [GROUP_TAG, ..] => match GroupContents::decode(&bytes) {
                Ok(contents) => groups.borrow_mut().push(contents),
                Err(error) => {
                    web_sys::console::error_1(&format!("bad control group: {error}").into());
                }
            },
            [COMMAND_VERSION, player, rest @ ..] if rest.is_empty() || rest.len() == 8 => {
                let float = |at: usize| {
                    f32::from_le_bytes([rest[at], rest[at + 1], rest[at + 2], rest[at + 3]])
                };
                starting_camera.set((!rest.is_empty()).then(|| [float(0), float(4)]));
                seat.set(*player);
            }
            // A server on another format: this page's orders would be
            // misread, so it gives none until reloaded.
            _ => {
                seat.set(0);
                starting_camera.set(None);
                web_sys::console::error_1(&"the server speaks another command format".into());
            }
        }
    });
    let weak = inner.clone();
    let on_close = Closure::<dyn FnMut()>::new(move || {
        let Some(strong) = weak.upgrade() else {
            return;
        };
        let wait = {
            let mut inner = strong.borrow_mut();
            inner.seat.set(0);
            inner.starting_camera.set(None);
            inner.socket = None;
            // The server's slot died with the socket.
            inner.sent = None;
            inner.next_token = 1;
            let wait = inner.reconnect_ms;
            inner.reconnect_ms = (wait * 2).min(RECONNECT_MAX_MS);
            wait
        };
        let again = weak.clone();
        let retry = Closure::once_into_js(move || connect(&again));
        if let Some(window) = web_sys::window() {
            let _ = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(retry.unchecked_ref(), wait);
        }
    });
    let weak = inner.clone();
    let on_open = Closure::once_into_js(move || {
        if let Some(strong) = weak.upgrade() {
            strong.borrow_mut().reconnect_ms = RECONNECT_MS;
        }
    });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    socket.set_onopen(Some(on_open.unchecked_ref()));
    let mut inner = strong.borrow_mut();
    inner.socket = Some(socket);
    inner._handlers = Some(Handlers {
        _on_message: on_message,
        _on_close: on_close,
    });
}
