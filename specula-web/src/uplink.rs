//! `/play`'s orders to the server (`specula_assets::command`), over the
//! `/play/command` WebSocket, which the page holds itself: orders are page
//! events and few, so they need not pass through the stream worker. The
//! server's first message names the player the visitor commands; its text
//! messages answer the placement queries.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;

use specula_assets::command::COMMAND_VERSION;
use specula_assets::command::Command;
use specula_assets::command::Query;
use specula_assets::command::QueryKind;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::BinaryType;
use web_sys::MessageEvent;
use web_sys::WebSocket;

/// The pause before reconnecting, doubled after each failure, as the
/// server may be restarting.
const RECONNECT_MS: i32 = 1000;
const RECONNECT_MAX_MS: i32 = 30_000;

pub struct Uplink {
    inner: Rc<RefCell<Inner>>,
}

struct Inner {
    socket: Option<WebSocket>,
    /// The player commanded, 0 for none (yet).
    seat: Rc<Cell<u8>>,
    next_query: u32,
    /// Query id and result, as they came.
    replies: Rc<RefCell<Vec<(u32, JsValue)>>>,
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
            next_query: 1,
            replies: Rc::new(RefCell::new(Vec::new())),
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

    /// Dropped while the socket is not open: an order is worth giving only
    /// now.
    pub fn send(&self, command: &Command) {
        let inner = self.inner.borrow();
        let Some(socket) = &inner.socket else {
            return;
        };
        if socket.ready_state() == WebSocket::OPEN && inner.seat.get() != 0 {
            let _ = socket.send_with_u8_array(&command.encode());
        }
    }

    /// Asks the engine; the id the reply will carry, or none while the
    /// socket is not open.
    pub fn query(&self, kind: QueryKind) -> Option<u32> {
        let mut inner = self.inner.borrow_mut();
        let socket = inner.socket.as_ref()?;
        if socket.ready_state() != WebSocket::OPEN || inner.seat.get() == 0 {
            return None;
        }
        let id = inner.next_query;
        let bytes = Query { id, kind }.encode();
        socket.send_with_u8_array(&bytes).ok()?;
        inner.next_query = id.wrapping_add(1).max(1);
        Some(id)
    }

    /// The replies that came since the last call: id and result.
    pub fn take_replies(&self) -> Vec<(u32, JsValue)> {
        std::mem::take(&mut *self.inner.borrow().replies.borrow_mut())
    }
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
        "{scheme}://{}/play/command",
        location.host().unwrap_or_default()
    );
    let Ok(socket) = WebSocket::new(&url) else {
        web_sys::console::error_1(&format!("cannot open {url}").into());
        return;
    };
    socket.set_binary_type(BinaryType::Arraybuffer);
    let seat = Rc::clone(&strong.borrow().seat);
    let replies = Rc::clone(&strong.borrow().replies);
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        if let Some(text) = event.data().as_string() {
            let Ok(reply) = js_sys::JSON::parse(&text) else {
                return;
            };
            let field = |key: &str| js_sys::Reflect::get(&reply, &key.into()).ok();
            if let (Some(id), Some(result)) =
                (field("id").and_then(|id| id.as_f64()), field("result"))
            {
                replies.borrow_mut().push((id as u32, result));
            }
            return;
        }
        let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
            return;
        };
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        match bytes.as_slice() {
            [COMMAND_VERSION, player] => seat.set(*player),
            // A server on another format: this page's orders would be
            // misread, so it gives none until reloaded.
            _ => {
                seat.set(0);
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
            inner.socket = None;
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
