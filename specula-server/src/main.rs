mod actors;
mod classify;
mod delta;
mod detail;
mod feed;
mod history;
mod observer;
mod play;
mod terrain;
mod usage;
mod view;
mod web;
mod wire;

use std::collections::HashMap;
use std::io;
use std::io::BufReader;
use std::io::ErrorKind;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rocket::State;
use rocket::fairing::AdHoc;
use rocket::fs::FileServer;
use rocket::futures::SinkExt;
use rocket::futures::StreamExt;
use rocket::http::ContentType;
use rocket::http::CookieJar;
use rocket::http::Header;
use rocket::tokio::sync::broadcast::Receiver;
use rocket::tokio::sync::broadcast::error::RecvError;
use rocket_ws::Channel;
use rocket_ws::WebSocket;
use specula_assets::command::COMMAND_VERSION;
use specula_assets::command::Command as Order;
use specula_assets::command::QUERY_TAG;
use specula_assets::command::Query;

use crate::classify::Classifier;
use crate::delta::DeltaMeter;
use crate::feed::Record;
use crate::history::History;
use crate::play::Play;
use crate::play::Setup;
use crate::usage::Usage;
use crate::view::Hub;
use crate::view::Message;
use crate::view::SummaryBody;
use crate::view::View;

const DEFAULT_SOCKET: &str = "/tmp/specula-feed.sock";
/// As AoE2 DE's ranked spectating: long enough that a player cannot scout
/// the opponent through the broadcast.
const DEFAULT_DELAY_SECS: u64 = 120;
/// Relative to the working directory, as `WEB_ROOT`.
const DEFAULT_HISTORY_DIR: &str = "history";
/// About 70 one-hour 1v1s or 20 large 4v4s.
const DEFAULT_HISTORY_LIMIT: u64 = 1 << 30;
/// Relative to the working directory, as `WEB_ROOT`.
const DEFAULT_USAGE_DIR: &str = "usage";
const USAGE: &str = "usage: specula-server [SOCKET] [--observe HOST:PORT | --play [--play-port PORT]] \
                     [--delay SECONDS] [--history DIR] [--history-limit SIZE] [--usage DIR] \
                     [--stats-key KEY]";
/// Relative to the working directory, so the server runs from the repo root.
pub const WEB_ROOT: &str = "specula-web";

#[rocket::main]
async fn main() {
    let Args {
        socket: path,
        observe,
        play_port,
        delay,
        history_dir,
        history_limit,
        usage_dir,
        stats_key,
    } = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(error) = remove_stale_socket(&path) {
        eprintln!("cannot remove stale socket {path}: {error}");
        std::process::exit(1);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("cannot listen on {path}: {error}");
            std::process::exit(1);
        }
    };
    println!("waiting for an observer on {path}");
    println!("viewers see the game {} s late", delay.as_secs());
    let history = if history_limit == 0 {
        println!("history off: only the current and the previous match are kept");
        None
    } else {
        match History::open(history_dir.clone(), history_limit) {
            Ok(history) => {
                println!("{}", history.summary());
                Some(history)
            }
            Err(error) => {
                eprintln!(
                    "cannot open the history in {}: {error}",
                    history_dir.display()
                );
                std::process::exit(1);
            }
        }
    };

    let usage = match Usage::open(usage_dir.clone(), stats_key) {
        Ok(usage) => {
            println!("{}", usage.summary());
            Arc::new(usage)
        }
        Err(error) => {
            eprintln!(
                "cannot open the usage log in {}: {error}",
                usage_dir.display()
            );
            std::process::exit(1);
        }
    };

    let hub = Arc::new(Hub::new(delay, history));
    let publisher_hub = Arc::clone(&hub);
    std::thread::spawn(move || publisher_hub.run_publisher());
    let play = play_port.map(|port| {
        // As `observer::supervise`: an engine a dead server left behind.
        observer::kill_recorded(&path);
        println!("hosting games for /play on port {port}");
        Arc::new(Play::new(path.clone(), port, Arc::clone(&hub)))
    });
    let feed_hub = Arc::clone(&hub);
    let feed_play = play.clone();
    // The feed is blocking std I/O, so it gets its own thread rather than
    // a slot in Rocket's async runtime.
    std::thread::spawn(move || accept_observers(listener, &feed_hub, feed_play.as_ref()));
    let supervised = observe.is_some() || play.is_some();
    if let Some((host, port)) = observe {
        let supervisor_hub = Arc::clone(&hub);
        let socket = path.clone();
        std::thread::spawn(move || observer::supervise(&host, &port, &socket, &supervisor_hub));
    }

    let result = rocket::build()
        .manage(hub)
        .manage(play)
        .manage(usage)
        .manage(web::Pkg::default())
        .manage(web::AssetsSize::default())
        .manage(web::AtlasVersions::default())
        .mount(
            "/",
            rocket::routes![
                viewer,
                detail_viewer,
                play_page,
                play_setup,
                play_new,
                play_action,
                play_viewer,
                play_detail_viewer,
                play_segment_file,
                play_detail_segment_file,
                play_terrain,
                play_data,
                play_command,
                play_summary,
                web::terrain,
                web::data,
                segment_file,
                detail_segment_file,
                match_info,
                match_summary,
                beat,
                stats,
                web::index,
                web::page,
                web::pkg_file,
                web::asset,
                web::selection_atlas,
                web::icon_atlas,
                web::gui_file,
                web::lang_file
            ],
        )
        .mount("/", FileServer::from(WEB_ROOT))
        .attach(AdHoc::on_response("Cache-Control", |_, response| {
            Box::pin(async move {
                // What is not named by its content (pages, the worker, the
                // pointers) keeps its name across builds, and a browser
                // left to guess may pair a new page with an old worker.
                if !response.headers().contains("Cache-Control") {
                    response.set_header(Header::new("Cache-Control", "no-cache"));
                }
            })
        }))
        .launch()
        .await;
    if supervised {
        observer::kill_recorded(&path);
    }
    if let Err(error) = result {
        eprintln!("web server failed: {error}");
        std::process::exit(1);
    }
}

/// `play`: in play mode, where the commands go back over the feed socket.
fn accept_observers(listener: UnixListener, hub: &Arc<Hub>, play: Option<&Arc<Play>>) {
    // One observer at a time: each one is a whole match.
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                println!("observer connected");
                hub.start_feed();
                if let Some(play) = play {
                    play.set_uplink(stream.try_clone().ok());
                }
                // A feed that outlives its game must not write into the
                // rematch's.
                let generation = play.and_then(|play| play.current_generation());
                let served = serve(stream, hub, play, generation);
                if let Some(play) = play {
                    play.set_uplink(None);
                }
                match served {
                    Ok(()) => println!("observer disconnected"),
                    Err(error) => eprintln!("observer feed failed: {error}"),
                }
                // Whether the host quit, the game was won or the observer
                // died, this match gets no more turns; a new observer starts
                // a new one.
                hub.end_match();
            }
            Err(error) => eprintln!("accept failed: {error}"),
        }
    }
}

#[rocket::get("/ws")]
fn viewer(
    socket: WebSocket,
    hub: &State<Arc<Hub>>,
    usage: &State<Arc<Usage>>,
    cookies: &CookieJar<'_>,
) -> Channel<'static> {
    let hub = Arc::clone(hub);
    let watch = usage.watch(cookies);
    relay(socket, watch, move || hub.subscribe(View::Observer))
}

/// The detail stream, which viewers hold open only while they have
/// something selected.
#[rocket::get("/detail")]
fn detail_viewer(socket: WebSocket, hub: &State<Arc<Hub>>) -> Channel<'static> {
    let hub = Arc::clone(hub);
    relay(socket, (), move || hub.subscribe_detail(View::Observer))
}

/// In play mode: the player's page, the 3D page on the live stream, for
/// the visitor who holds the seat; the setup page for anyone else, or with
/// `?new`.
#[rocket::get("/play?<new>")]
#[allow(clippy::too_many_arguments)]
async fn play_page(
    new: Option<&str>,
    play: &State<Option<Arc<Play>>>,
    pkg: &State<web::Pkg>,
    assets: &State<web::AssetsSize>,
    atlases: &State<web::AtlasVersions>,
    usage: &State<Arc<Usage>>,
    cookies: &CookieJar<'_>,
    referrer: usage::Referrer,
) -> Option<rocket::response::content::RawHtml<String>> {
    let play = play.as_ref()?;
    let seated = play.seat(usage::visitor(cookies).as_deref()).is_some();
    let page = if seated && new.is_none() {
        "index.html"
    } else {
        "play.html"
    };
    web::page(
        web::Page(page),
        pkg,
        assets,
        atlases,
        usage,
        cookies,
        referrer,
    )
    .await
}

#[rocket::get("/play/setup")]
fn play_setup(play: &State<Option<Arc<Play>>>, cookies: &CookieJar<'_>) -> Option<Json> {
    let play = play.as_ref()?;
    let status = play.status(usage::visitor(cookies).as_deref());
    Some(Json(
        play::setup_json(&status),
        Header::new("Cache-Control", "no-store"),
    ))
}

#[derive(rocket::Responder)]
#[response(content_type = "json")]
struct Json(String, Header<'static>);

/// Starts a game from the setup page's form; the text says why not.
#[rocket::post("/play/new", data = "<form>")]
fn play_new(
    form: rocket::form::Form<HashMap<String, String>>,
    play: &State<Option<Arc<Play>>>,
    cookies: &CookieJar<'_>,
) -> Result<(), rocket::response::status::BadRequest<&'static str>> {
    let refuse = rocket::response::status::BadRequest;
    let play = play.as_ref().ok_or(refuse("not in play mode"))?;
    // The page set the cookie when it loaded.
    let visitor = usage::visitor(cookies).ok_or(refuse("no visitor cookie"))?;
    let setup = Setup::from_form(&form).map_err(refuse)?;
    play.start(&visitor, &setup).map_err(refuse)
}

/// The player's page's Resign, Exit and Rematch, for the visitor's game;
/// the text says why not.
#[rocket::post("/play/<action>")]
fn play_action(
    action: &str,
    play: &State<Option<Arc<Play>>>,
    cookies: &CookieJar<'_>,
) -> Result<(), rocket::response::status::BadRequest<&'static str>> {
    let refuse = rocket::response::status::BadRequest;
    let play = play.as_ref().ok_or(refuse("not in play mode"))?;
    let visitor = usage::visitor(cookies).ok_or(refuse("no visitor cookie"))?;
    match action {
        "resign" => play.resign(&visitor),
        "end" => play.end(&visitor),
        "rematch" => play.rematch(&visitor),
        _ => Err("unknown action"),
    }
    .map_err(refuse)
}

/// The stream of the visitor's seat: undelayed, and only what its player
/// sees. A visitor without a seat gets none.
fn seat_view(play: &State<Option<Arc<Play>>>, cookies: &CookieJar<'_>) -> Option<View> {
    let seat = play.as_ref()?.seat(usage::visitor(cookies).as_deref())?;
    Some(View::Seat(u32::from(seat)))
}

#[rocket::get("/play/ws")]
fn play_viewer(
    socket: WebSocket,
    hub: &State<Arc<Hub>>,
    play: &State<Option<Arc<Play>>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let hub = Arc::clone(hub);
    let view = seat_view(play, cookies)?;
    Some(relay(socket, (), move || hub.subscribe(view)))
}

#[rocket::get("/play/detail")]
fn play_detail_viewer(
    socket: WebSocket,
    hub: &State<Arc<Hub>>,
    play: &State<Option<Arc<Play>>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let hub = Arc::clone(hub);
    let view = seat_view(play, cookies)?;
    Some(relay(socket, (), move || hub.subscribe_detail(view)))
}

#[rocket::get("/play/match/<id>/seg/<index>")]
fn play_segment_file(
    id: &str,
    index: usize,
    hub: &State<Arc<Hub>>,
    play: &State<Option<Arc<Play>>>,
    cookies: &CookieJar<'_>,
) -> Option<SegmentBytes> {
    let view = seat_view(play, cookies)?;
    segment_bytes(hub.segment(u64::from_str_radix(id, 16).ok()?, index, view)?)
}

#[rocket::get("/play/match/<id>/detail/<index>")]
fn play_detail_segment_file(
    id: &str,
    index: usize,
    hub: &State<Arc<Hub>>,
    play: &State<Option<Arc<Play>>>,
    cookies: &CookieJar<'_>,
) -> Option<SegmentBytes> {
    let view = seat_view(play, cookies)?;
    segment_bytes(hub.detail_segment(u64::from_str_radix(id, 16).ok()?, index, view)?)
}

/// The seat's terrain as revealed so far (`terrain.rs`, kind 1); a later
/// revision than the keyframe names is as good, since reveals only add.
#[rocket::get("/play/terrain/<_id>")]
fn play_terrain(
    _id: &str,
    hub: &State<Arc<Hub>>,
    play: &State<Option<Arc<Play>>>,
    cookies: &CookieJar<'_>,
) -> Option<SegmentBytes> {
    let View::Seat(player) = seat_view(play, cookies)? else {
        return None;
    };
    Some(SegmentBytes(
        hub.seat_terrain(player)?,
        Header::new("Cache-Control", "no-store"),
    ))
}

/// A `/data` file the seat's stream named.
#[rocket::get("/play/data/<hash>")]
fn play_data(
    hash: &str,
    hub: &State<Arc<Hub>>,
    play: &State<Option<Arc<Play>>>,
    cookies: &CookieJar<'_>,
) -> Option<Json> {
    let view = seat_view(play, cookies)?;
    let json = hub.data(u64::from_str_radix(hash, 16).ok()?, view)?;
    Some(Json(
        String::from_utf8(json.to_vec()).ok()?,
        Header::new("Cache-Control", "no-store"),
    ))
}

/// The player's orders (`specula_assets::command`). The server first sends
/// the format version and the player number the visitor commands, 0 for
/// none; orders from a visitor without the seat are dropped, as is the
/// seat looked up again for each, since a new game may have replaced it.
#[rocket::get("/play/command")]
fn play_command(
    socket: WebSocket,
    play: &State<Option<Arc<Play>>>,
    hub: &State<Arc<Hub>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let play = Arc::clone(play.as_ref()?);
    let hub = Arc::clone(hub);
    let visitor = usage::visitor(cookies);
    Some(socket.channel(move |mut stream| {
        Box::pin(async move {
            let seat = play.seat(visitor.as_deref()).unwrap_or(0);
            let mut replies = play.replies();
            stream
                .send(rocket_ws::Message::binary(vec![COMMAND_VERSION, seat]))
                .await?;
            loop {
                let message = rocket::tokio::select! {
                    reply = replies.recv() => match reply {
                        Ok(reply) => {
                            // Every page of the seat asks with its own ids;
                            // a reply to another's is ignored there.
                            if let Some(seat) = play.seat(visitor.as_deref()) {
                                let reply = play::map_reply_ids(&reply, |id| {
                                    hub.seat_handle(u32::from(seat), id)
                                });
                                stream.send(rocket_ws::Message::text(reply)).await?;
                            }
                            continue;
                        }
                        // A dropped reply only leaves one preview untinted.
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => return Ok(()),
                    },
                    incoming = stream.next() => match incoming {
                        None => return Ok(()),
                        Some(message) => message?,
                    },
                };
                let rocket_ws::Message::Binary(bytes) = message else {
                    continue;
                };
                let Some(seat) = play.seat(visitor.as_deref()) else {
                    continue;
                };
                // The page names entities by the seat's handles; one the
                // seat does not hold now drops the order, as the seat
                // cannot have been shown it.
                let to_feed = |ids: Vec<&mut u32>| {
                    ids.into_iter()
                        .all(|id| match hub.seat_entity(u32::from(seat), *id) {
                            Some(feed) => {
                                *id = feed;
                                true
                            }
                            None => false,
                        })
                };
                if bytes.first() == Some(&QUERY_TAG) {
                    match Query::decode(&bytes) {
                        Ok(mut query) => {
                            if to_feed(query.entity_ids_mut()) {
                                play.query(seat, &query);
                            }
                        }
                        Err(error) => eprintln!("bad query from the player: {error}"),
                    }
                    continue;
                }
                match Order::decode(&bytes) {
                    Ok(mut order) => {
                        if to_feed(order.entity_ids_mut()) {
                            play.send(seat, &order);
                        }
                    }
                    Err(error) => eprintln!("bad command from the player: {error}"),
                }
            }
        })
    }))
}

/// `held` is dropped when the stream closes. `subscribe` gives None when
/// there is nothing to follow (`/play/ws` outside play mode).
fn relay(
    socket: WebSocket,
    held: impl Send + 'static,
    subscribe: impl Fn() -> Option<(Vec<Message>, Receiver<Message>)> + Send + 'static,
) -> Channel<'static> {
    socket.channel(move |mut stream| {
        Box::pin(async move {
            let _held = held;
            let Some((mut first, mut receiver)) = subscribe() else {
                return Ok(());
            };
            loop {
                for message in first.drain(..) {
                    stream.send(rocket_ws::Message::binary(&*message)).await?;
                }
                rocket::tokio::select! {
                    broadcast = receiver.recv() => match broadcast {
                        Ok(message) => stream.send(rocket_ws::Message::binary(&*message)).await?,
                        // Missed deltas cannot be patched over; start again
                        // from a fresh keyframe.
                        Err(RecvError::Lagged(_)) => match subscribe() {
                            Some(again) => (first, receiver) = again,
                            None => return Ok(()),
                        },
                        Err(RecvError::Closed) => return Ok(()),
                    },
                    incoming = stream.next() => match incoming {
                        None => return Ok(()),
                        Some(Err(error)) => return Err(error),
                        // Nothing is read from viewers yet; the stream is
                        // polled only to notice the close.
                        Some(Ok(_)) => {}
                    },
                }
            }
        })
    })
}

#[derive(rocket::Responder)]
#[response(content_type = "binary")]
struct SegmentBytes(Vec<u8>, Header<'static>);

#[rocket::get("/match/<id>/seg/<index>")]
fn segment_file(id: &str, index: usize, hub: &State<Arc<Hub>>) -> Option<SegmentBytes> {
    segment_bytes(hub.segment(u64::from_str_radix(id, 16).ok()?, index, View::Observer)?)
}

#[rocket::get("/match/<id>/detail/<index>")]
fn detail_segment_file(id: &str, index: usize, hub: &State<Arc<Hub>>) -> Option<SegmentBytes> {
    segment_bytes(hub.detail_segment(u64::from_str_radix(id, 16).ok()?, index, View::Observer)?)
}

/// "<segments> <live 0|1>", as `view.rs` describes.
#[rocket::get("/match/<id>/info")]
fn match_info(
    id: &str,
    hub: &State<Arc<Hub>>,
    usage: &State<Arc<Usage>>,
    cookies: &CookieJar<'_>,
) -> Option<MatchInfo> {
    let (segments, live) = hub.match_info(u64::from_str_radix(id, 16).ok()?)?;
    usage.replay(cookies, id);
    Some(MatchInfo(
        format!("{segments} {}", u8::from(live)),
        // The open match grows a segment every 30 s.
        Header::new("Cache-Control", "no-store"),
    ))
}

#[derive(rocket::Responder)]
#[response(content_type = "text")]
struct MatchInfo(String, Header<'static>);

/// The envelope the summary routes answer: the capture's turn, whether it
/// is the final one, and the engine's JSON as it is.
pub(crate) fn summary_envelope(turn: u32, is_final: bool, metadata: &[u8]) -> Option<Vec<u8>> {
    std::str::from_utf8(metadata).ok()?;
    let mut body = format!("{{\"turn\":{turn},\"final\":{is_final},\"metadata\":").into_bytes();
    body.extend_from_slice(metadata);
    body.push(b'}');
    Some(body)
}

/// An envelope as the response body, shared with the entry that holds it
/// rather than copied for each request.
struct SharedBytes(Arc<[u8]>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<'r> rocket::response::Responder<'r, 'static> for SharedBytes {
    fn respond_to(self, _: &'r rocket::Request<'_>) -> rocket::response::Result<'static> {
        rocket::Response::build()
            .sized_body(self.0.len(), std::io::Cursor::new(self))
            .ok()
    }
}

/// Serves an envelope, compressed when the client takes brotli and the
/// stored copy is.
fn summary_response(body: SummaryBody, accepts: bool) -> Option<web::Encoded<SharedBytes>> {
    let brotli = body.brotli && accepts;
    let bytes: Arc<[u8]> = if body.brotli && !accepts {
        // A client without brotli is rare; the decode must not hold up the
        // runtime's other requests meanwhile.
        rocket::tokio::task::block_in_place(|| {
            let mut reader = brotli::Decompressor::new(&body.bytes[..], 4096);
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut reader, &mut bytes).ok()?;
            Some(Arc::from(bytes))
        })?
    } else {
        body.bytes
    };
    Some(web::Encoded::new(
        SharedBytes(bytes),
        ContentType::JSON,
        brotli,
        "no-store",
    ))
}

/// The statistics summary of a match as `/` may see it, which trails the
/// game by `--delay` like the stream.
#[rocket::get("/match/<id>/summary?<turn>")]
fn match_summary(
    id: &str,
    turn: Option<u32>,
    accepts: web::AcceptsBrotli,
    hub: &State<Arc<Hub>>,
) -> Option<web::Encoded<SharedBytes>> {
    let summary = hub.summary(u64::from_str_radix(id, 16).ok()?, turn)?;
    summary_response(summary, accepts.accepts())
}

/// The statistics summary of the visitor's own seat, undelayed: only what
/// the seat itself may see in the summary.
#[rocket::get("/play/summary?<turn>")]
fn play_summary(
    turn: Option<u32>,
    play: &State<Option<Arc<Play>>>,
    accepts: web::AcceptsBrotli,
    cookies: &CookieJar<'_>,
) -> Option<web::Encoded<SharedBytes>> {
    let metadata = play
        .as_ref()?
        .summary(usage::visitor(cookies).as_deref(), turn)?;
    summary_response(metadata, accepts.accepts())
}

/// The client's heartbeat, a `sendBeacon` from `usage.rs` there.
#[rocket::post("/beat", data = "<body>")]
fn beat(body: String, usage: &State<Arc<Usage>>, cookies: &CookieJar<'_>) {
    usage.beat(cookies, &body);
}

/// For the operator only: 404 unless `key` is `--stats-key`.
#[rocket::get("/stats?<key>")]
fn stats(key: Option<&str>, usage: &State<Arc<Usage>>) -> Option<MatchInfo> {
    usage.authorized(key).then(|| {
        MatchInfo(
            usage.report(),
            // Nor kept by Cloudflare, which would show it without the key.
            Header::new("Cache-Control", "no-store"),
        )
    })
}

fn segment_bytes((bytes, closed): (Message, bool)) -> Option<SegmentBytes> {
    // The match id is in the URL, so a closed segment never changes; the
    // open one grows every turn.
    let cache = if closed {
        Header::new("Cache-Control", "public, max-age=31536000, immutable")
    } else {
        Header::new("Cache-Control", "no-store")
    };
    Some(SegmentBytes(bytes.to_vec(), cache))
}

struct Args {
    socket: String,
    observe: Option<(String, String)>,
    /// With `--play`: the port the hosted games listen on.
    play_port: Option<u16>,
    delay: Duration,
    history_dir: PathBuf,
    /// 0: no history.
    history_limit: u64,
    usage_dir: PathBuf,
    stats_key: Option<String>,
}

/// `[SOCKET] [--observe HOST:PORT | --play [--play-port PORT]] [--delay
/// SECONDS] [--history DIR] [--history-limit SIZE] [--usage DIR]
/// [--stats-key KEY]`, in any order.
fn parse_args() -> Result<Args, String> {
    let mut socket = None;
    let mut observe = None;
    let mut play = false;
    let mut play_port = play::DEFAULT_PORT;
    let mut delay = Duration::from_secs(DEFAULT_DELAY_SECS);
    let mut history_dir = PathBuf::from(DEFAULT_HISTORY_DIR);
    let mut history_limit = DEFAULT_HISTORY_LIMIT;
    let mut usage_dir = PathBuf::from(DEFAULT_USAGE_DIR);
    let mut stats_key = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--delay" {
            let seconds = args.next().ok_or("--delay needs SECONDS")?;
            let seconds = seconds
                .parse::<u32>()
                .map_err(|_| format!("--delay {seconds}: expected whole seconds"))?;
            delay = Duration::from_secs(u64::from(seconds));
        } else if arg == "--history" {
            history_dir = args.next().ok_or("--history needs DIR")?.into();
        } else if arg == "--history-limit" {
            let size = args.next().ok_or("--history-limit needs SIZE")?;
            history_limit = history::parse_size(&size).ok_or_else(|| {
                format!("--history-limit {size}: expected bytes or a size such as 500M or 1G")
            })?;
        } else if arg == "--usage" {
            usage_dir = args.next().ok_or("--usage needs DIR")?.into();
        } else if arg == "--stats-key" {
            let key = args.next().filter(|key| !key.is_empty());
            stats_key = Some(key.ok_or("--stats-key needs KEY")?);
        } else if arg == "--observe" {
            let target = args.next().ok_or("--observe needs HOST:PORT")?;
            let (host, port) = target
                .rsplit_once(':')
                .filter(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
                .ok_or_else(|| format!("--observe {target}: expected HOST:PORT"))?;
            observe = Some((host.to_string(), port.to_string()));
        } else if arg == "--play" {
            play = true;
        } else if arg == "--play-port" {
            let port = args.next().ok_or("--play-port needs PORT")?;
            play_port = port
                .parse()
                .map_err(|_| format!("--play-port {port}: expected a port"))?;
        } else if socket.is_none() && !arg.starts_with("--") {
            socket = Some(arg);
        } else {
            return Err(format!("unexpected argument {arg}"));
        }
    }
    // Both would feed the one socket.
    if play && observe.is_some() {
        return Err("--play and --observe exclude each other".to_string());
    }
    Ok(Args {
        socket: socket.unwrap_or_else(|| DEFAULT_SOCKET.to_string()),
        observe,
        play_port: play.then_some(play_port),
        delay,
        history_dir,
        history_limit,
        usage_dir,
        stats_key,
    })
}

// Only a socket is removed, so a mistyped path cannot delete a real file.
fn remove_stale_socket(path: &str) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path),
        Ok(_) => Err(io::Error::new(
            ErrorKind::AlreadyExists,
            "exists and is not a socket",
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn serve(
    stream: UnixStream,
    hub: &Arc<Hub>,
    play: Option<&Arc<Play>>,
    generation: Option<u64>,
) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let version = feed::read_header(&mut reader)?;
    let mut templates = HashMap::new();
    let mut details = feed::Details::default();
    let mut classifier = Classifier::default();
    let mut meter = DeltaMeter::new();
    while let Some(record) = feed::read_record(&mut reader, version)? {
        match record {
            Record::Template {
                index,
                name,
                icon,
                player_color,
            } => {
                hub.add_template(index, name.clone())?;
                templates.insert(index, name);
                if let Some(icon) = icon {
                    hub.set_template_icon(index, icon);
                }
                if player_color {
                    hub.set_template_player_color(index);
                }
            }
            Record::Map { size, speed } => hub.set_map(size, speed),
            Record::Players(players) => hub.set_players(players),
            Record::Territory(territory) => hub.set_territory(territory),
            Record::Sounds(sounds) => hub.set_sounds(sounds),
            Record::Projectiles { launches, removals } => hub.set_projectiles(launches, removals),
            Record::String { index, text } => hub.add_string(index, text)?,
            Record::VariantSet { index, pairs } => hub.add_variant_set(index, pairs)?,
            Record::VariableSet { index, pairs } => hub.add_variable_set(index, pairs)?,
            Record::TemplateDetails {
                index,
                details: template,
            } => hub.set_template_details(index, *template)?,
            Record::Details { changed, gone } => details.apply(changed, gone),
            Record::ProductionList { index, items } => details.add_list(index, items),
            Record::Renames(renames) => details.set_renames(renames),
            Record::Data { key, json } => details.set_data(key, hub.add_data(json)),
            Record::Stats { index, json } => details.set_stats(index, hub.add_data(json)),
            Record::PlayerGui(player_gui) => details.set_player_gui(player_gui),
            Record::Los { side, players } => hub.set_los(side, players),
            Record::Summary(summary) => {
                hub.set_summary(summary.turn, summary.is_final, Arc::clone(&summary.full));
                if let (Some(play), Some(generation)) = (play, generation) {
                    play.set_summary(generation, summary);
                }
            }
            Record::QueryReply { id, json } => {
                if let Some(play) = play {
                    play.reply(id, &json);
                }
            }
            Record::Economy(economy) => {
                hub.set_economy(&economy);
                details.set_economy(economy);
            }
            Record::Terrain(terrain) => {
                let resource = terrain::encode(&terrain);
                println!(
                    "terrain {:016x}: {} tiles per side, {} textures ({} unknown), {} bytes",
                    resource.id,
                    terrain.tiles_per_side,
                    terrain.textures.len(),
                    resource.unknown,
                    resource.bytes.len()
                );
                hub.set_terrain(resource.id, resource.bytes, Arc::from(terrain));
            }
            Record::Turn { turn, entities } => {
                hub.publish(turn, &entities, &details)?;
                details.end_turn();
                if let Some(summary) = classifier.observe(turn, &entities) {
                    println!("{summary}, {} templates", templates.len());
                }
                if let Some(summary) = meter.observe(turn, &entities)? {
                    println!("{summary}");
                }
            }
        }
    }
    Ok(())
}
