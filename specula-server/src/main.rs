mod actors;
mod classify;
mod config;
mod delta;
mod detail;
mod feed;
mod history;
mod nickname;
mod observer;
mod play;
mod registry;
mod seats;
mod stop;
mod terrain;
mod usage;
mod view;
mod web;
mod wire;

use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::io::BufReader;
use std::io::ErrorKind;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use rocket::State;
use rocket::config::Shutdown as ShutdownConfig;
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
use crate::config::Config;
use crate::delta::DeltaMeter;
use crate::feed::Record;
use crate::history::History;
use crate::play::Play;
use crate::play::Setup;
use crate::registry::MatchId;
use crate::registry::Registry;
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
const USAGE: &str = "usage: specula-server [SOCKET] [--observe HOST:PORT | [--slots N] \
                     [--port-base PORT]] [--max-observers N] [--admin-key KEY] \
                     [--delay SECONDS] [--history DIR] [--history-limit SIZE] [--usage DIR] \
                     [--engine-dir DIR] [--stats-key KEY] [--wall-limit MINUTES] \
                     [--game-limit MINUTES]";
/// Relative to the working directory, so the server runs from the repo root.
pub const WEB_ROOT: &str = "specula-web";

#[rocket::main]
async fn main() {
    let Args {
        socket: path,
        observe,
        config,
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
    println!("viewers see the game {} s late", delay.as_secs());
    println!("{}", config.summary());
    let config = Arc::new(config);
    let history = if history_limit == 0 {
        println!("history off: only the current and the previous match are kept");
        None
    } else {
        match History::open(history_dir.clone(), history_limit) {
            Ok(history) => {
                println!("{}", history.summary());
                Some(Arc::new(Mutex::new(history)))
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

    if let Err(error) = std::fs::create_dir_all(&config.engine_dir) {
        eprintln!(
            "cannot create the engine dir {}: {error}",
            config.engine_dir.display()
        );
        std::process::exit(1);
    }
    println!("engine logs and caches in {}", config.engine_dir.display());

    let supervised = observe.is_some();
    let broadcast = if config.slots == 0 {
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
        let hub = registry::spawn_hub(delay, history.clone(), None);
        let feed_hub = Arc::clone(&hub);
        // The feed is blocking std I/O, so it gets its own thread rather
        // than a slot in Rocket's async runtime.
        std::thread::spawn(move || accept_observers(listener, &feed_hub));
        if let Some((host, port)) = observe {
            let supervisor_hub = Arc::clone(&hub);
            let socket = path.clone();
            let engine_dir = config.engine_dir.clone();
            std::thread::spawn(move || {
                observer::supervise(&host, &port, &socket, &engine_dir, &supervisor_hub)
            });
        }
        Some(hub)
    } else {
        // As `observer::supervise`: an engine an older server left behind,
        // and the matches' engines, which a dead server cannot list.
        observer::kill_recorded(&path);
        observer::kill_stale(&path);
        None
    };
    let registry = Arc::new(Registry::new(
        Arc::clone(&config),
        path.clone(),
        delay,
        history,
        broadcast,
    ));
    if config.slots > 0 {
        let watcher = Arc::clone(&registry);
        std::thread::spawn(move || watcher.watch_matches());
    }

    // Signals are `stop::watch`'s: Rocket's own handling would end the web
    // server, and with it every hosted match, on the first one.
    let figment = rocket::Config::figment().merge((
        "shutdown",
        ShutdownConfig {
            ctrlc: false,
            signals: HashSet::new(),
            ..ShutdownConfig::default()
        },
    ));
    let ignited = rocket::custom(figment)
        .manage(Arc::clone(&registry))
        .manage(usage)
        .manage(config)
        .manage(web::Pkg::default())
        .manage(web::AssetsSize::default())
        .manage(web::AtlasVersions::default())
        .mount(
            "/",
            rocket::routes![
                viewer,
                detail_viewer,
                match_page,
                match_map_page,
                play_status,
                new_status,
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
                match_uuid,
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
        .ignite()
        .await;
    // Ignited first, so the handler is in place before the server listens.
    let result = match ignited {
        Ok(rocket) => {
            rocket::tokio::spawn(stop::watch(
                Arc::clone(&registry),
                rocket.shutdown(),
                supervised.then(|| path.clone()),
            ));
            rocket.launch().await
        }
        Err(error) => Err(error),
    };
    if supervised {
        observer::kill_recorded(&path);
    }
    registry.kill_all();
    if let Err(error) = result {
        eprintln!("web server failed: {error}");
        std::process::exit(1);
    }
}

fn accept_observers(listener: UnixListener, hub: &Arc<Hub>) {
    // One observer at a time: each one is a whole match.
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                println!("observer connected");
                hub.start_feed();
                match serve(stream, hub, None) {
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

#[rocket::get("/g/<id>/ws")]
fn viewer(
    id: MatchId,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    usage: &State<Arc<Usage>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let hub = registry.resolve(id)?.hub?;
    let watch = usage.watch(cookies);
    Some(relay(socket, watch, move || hub.subscribe(View::Observer)))
}

/// The detail stream, which viewers hold open only while they have
/// something selected.
#[rocket::get("/g/<id>/detail")]
fn detail_viewer(
    id: MatchId,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
) -> Option<Channel<'static>> {
    let hub = registry.resolve(id)?.hub?;
    Some(relay(socket, (), move || {
        hub.subscribe_detail(View::Observer)
    }))
}

/// The match's page: the 3D page on the live stream for the visitor who
/// holds the seat, with the delay for anyone else; the client asks
/// `play/status` which it is. The same HTML for every visitor.
#[rocket::get("/g/<id>?<t>")]
#[allow(clippy::too_many_arguments)]
async fn match_page(
    t: Option<&str>,
    id: MatchId,
    registry: &State<Arc<Registry>>,
    pkg: &State<web::Pkg>,
    assets: &State<web::AssetsSize>,
    atlases: &State<web::AtlasVersions>,
    usage: &State<Arc<Usage>>,
    cookies: &CookieJar<'_>,
    referrer: usage::Referrer,
) -> Option<web::IndexPage> {
    let source = registry.resolve(id)?;
    // An ended match is its replay (D23), which the worker opens only from
    // a moment.
    if t.is_none() && source.replay_only() {
        return Some(web::IndexPage::Redirect(Box::new(
            rocket::response::Redirect::to(format!("/g/{id}?t=0")),
        )));
    }
    web::render("index.html", pkg, assets, atlases, usage, cookies, referrer)
        .await
        .map(web::IndexPage::Page)
}

#[rocket::get("/g/<id>/map?<t>")]
#[allow(clippy::too_many_arguments)]
async fn match_map_page(
    t: Option<&str>,
    id: MatchId,
    registry: &State<Arc<Registry>>,
    pkg: &State<web::Pkg>,
    assets: &State<web::AssetsSize>,
    atlases: &State<web::AtlasVersions>,
    usage: &State<Arc<Usage>>,
    cookies: &CookieJar<'_>,
    referrer: usage::Referrer,
) -> Option<web::IndexPage> {
    let source = registry.resolve(id)?;
    // As for `match_page`.
    if t.is_none() && source.replay_only() {
        return Some(web::IndexPage::Redirect(Box::new(
            rocket::response::Redirect::to(format!("/g/{id}/map?t=0")),
        )));
    }
    web::render("map.html", pkg, assets, atlases, usage, cookies, referrer)
        .await
        .map(web::IndexPage::Page)
}

/// What the page reads about the visitor's seat in the match.
#[rocket::get("/g/<id>/play/status")]
fn play_status(
    id: MatchId,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Json> {
    let visitor = usage::visitor(cookies);
    let status = match registry.hosted(id) {
        Some(found) => found.play.status(visitor.as_deref()),
        // The broadcast has no seats, but the observer page still asks. A
        // dropped match has no seat routes left, only its rematch.
        None => {
            let rematch = registry.ended_owned(id, visitor.as_deref());
            if rematch.is_none() {
                registry.resolve(id)?;
            }
            play::Status {
                running: false,
                mine: false,
                defeated: false,
                rematch: rematch.unwrap_or(false),
                seat: None,
                limits: None,
            }
        }
    };
    let status = play::Status {
        limits: registry.limits(id),
        ..status
    };
    Some(Json(
        play::setup_json(&status),
        Header::new("Cache-Control", "no-store"),
    ))
}

#[derive(rocket::Responder)]
#[response(content_type = "json")]
struct Json(String, Header<'static>);

/// The setup form's state: whether the server is full and which game, if
/// any, is the visitor's.
#[rocket::get("/new")]
fn new_status(registry: &State<Arc<Registry>>, cookies: &CookieJar<'_>) -> Option<Json> {
    if registry.broadcast().is_some() {
        return None;
    }
    let own = usage::visitor(cookies).and_then(|visitor| registry.own_running(&visitor));
    let mine = own.is_some();
    Some(Json(
        play::form_json(
            mine || !registry.slot_free(),
            mine,
            own,
            registry.latest_running(),
            registry.stopping(),
        ),
        Header::new("Cache-Control", "no-store"),
    ))
}

/// Starts a game from the setup page's form and answers its UUID; the text
/// says why not.
#[rocket::post("/new", data = "<form>")]
fn play_new(
    form: rocket::form::Form<HashMap<String, String>>,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Result<String, rocket::response::status::BadRequest<&'static str>> {
    let refuse = rocket::response::status::BadRequest;
    if registry.broadcast().is_some() {
        return Err(refuse("not in play mode"));
    }
    // The page set the cookie when it loaded.
    let visitor = usage::visitor(cookies).ok_or(refuse("no visitor cookie"))?;
    let setup = Setup::from_form(&form).map_err(refuse)?;
    registry
        .launch(&visitor, &setup)
        .map(|id| id.to_string())
        .map_err(refuse)
}

/// The player's page's Resign, Pause, Exit and Rematch, for the visitor's
/// game; the text says why not. A rematch answers the new game's UUID.
#[rocket::post("/g/<id>/play/<action>")]
fn play_action(
    id: MatchId,
    action: &str,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Result<String, rocket::response::status::BadRequest<&'static str>> {
    let refuse = rocket::response::status::BadRequest;
    let visitor = usage::visitor(cookies).ok_or(refuse("no visitor cookie"))?;
    // Also once the match is dropped, from the end screen left open.
    if action == "rematch" {
        return registry
            .rematch(&visitor, id)
            .map(|new| new.to_string())
            .map_err(refuse);
    }
    let found = registry.hosted(id).ok_or(refuse("not in play mode"))?;
    let play = &found.play;
    match action {
        "resign" => play.resign(&visitor).map(|()| String::new()),
        "pause" => play.set_paused(&visitor, true).map(|()| String::new()),
        "resume" => play.set_paused(&visitor, false).map(|()| String::new()),
        "end" => play.end(&visitor).map(|()| String::new()),
        _ => Err("unknown action"),
    }
    .map_err(refuse)
}

/// The stream of the visitor's seat: undelayed, and only what its player
/// sees. A visitor without a seat gets none.
fn seat_view(found: &registry::Match, cookies: &CookieJar<'_>) -> Option<View> {
    let seat = found.play.seat(usage::visitor(cookies).as_deref())?;
    Some(View::Seat(u32::from(seat)))
}

#[rocket::get("/g/<id>/play/ws")]
fn play_viewer(
    id: MatchId,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let found = registry.hosted(id)?;
    let view = seat_view(&found, cookies)?;
    let hub = Arc::clone(&found.hub);
    Some(relay(socket, (), move || hub.subscribe(view)))
}

#[rocket::get("/g/<id>/play/detail")]
fn play_detail_viewer(
    id: MatchId,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let found = registry.hosted(id)?;
    let view = seat_view(&found, cookies)?;
    let hub = Arc::clone(&found.hub);
    Some(relay(socket, (), move || hub.subscribe_detail(view)))
}

#[rocket::get("/g/<id>/play/match/<mid>/seg/<index>")]
fn play_segment_file(
    id: MatchId,
    mid: &str,
    index: usize,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<SegmentBytes> {
    let found = registry.hosted(id)?;
    let view = seat_view(&found, cookies)?;
    segment_bytes(
        found
            .hub
            .segment(u64::from_str_radix(mid, 16).ok()?, index, view)?,
        true,
    )
}

#[rocket::get("/g/<id>/play/match/<mid>/detail/<index>")]
fn play_detail_segment_file(
    id: MatchId,
    mid: &str,
    index: usize,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<SegmentBytes> {
    let found = registry.hosted(id)?;
    let view = seat_view(&found, cookies)?;
    segment_bytes(
        found
            .hub
            .detail_segment(u64::from_str_radix(mid, 16).ok()?, index, view)?,
        true,
    )
}

/// The seat's terrain as revealed so far (`terrain.rs`, kind 1); a later
/// revision than the keyframe names is as good, since reveals only add.
#[rocket::get("/g/<id>/play/terrain/<_tid>")]
fn play_terrain(
    id: MatchId,
    _tid: &str,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<SegmentBytes> {
    let found = registry.hosted(id)?;
    let View::Seat(player) = seat_view(&found, cookies)? else {
        return None;
    };
    Some(SegmentBytes(
        found.hub.seat_terrain(player)?,
        Header::new("Cache-Control", "no-store"),
    ))
}

/// A `/data` file the seat's stream named.
#[rocket::get("/g/<id>/play/data/<hash>")]
fn play_data(
    id: MatchId,
    hash: &str,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Json> {
    let found = registry.hosted(id)?;
    let view = seat_view(&found, cookies)?;
    let json = found.hub.data(u64::from_str_radix(hash, 16).ok()?, view)?;
    Some(Json(
        String::from_utf8(json.to_vec()).ok()?,
        Header::new("Cache-Control", "no-store"),
    ))
}

/// The player's orders (`specula_assets::command`). The server first sends
/// the format version and the player number the visitor commands, 0 for
/// none; orders from a visitor without the seat are dropped, as is the
/// seat looked up again for each, since a new game may have replaced it.
#[rocket::get("/g/<id>/play/command")]
fn play_command(
    id: MatchId,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let found = registry.hosted(id)?;
    let play = Arc::clone(&found.play);
    let hub = Arc::clone(&found.hub);
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
                // The page names entities by the seat's handles. Each keeps
                // its last id for the match, so the engine filters a stale
                // one; a handle never issued is no entity the seat was
                // shown, and drops the order.
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
/// there is nothing to follow (`play/ws` of a match without a seat).
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

#[rocket::get("/g/<id>/match/<mid>/seg/<index>")]
fn segment_file(
    id: MatchId,
    mid: &str,
    index: usize,
    registry: &State<Arc<Registry>>,
) -> Option<SegmentBytes> {
    let source = registry.resolve(id)?;
    segment_bytes(
        source.segment(u64::from_str_radix(mid, 16).ok()?, index, false)?,
        false,
    )
}

#[rocket::get("/g/<id>/match/<mid>/detail/<index>")]
fn detail_segment_file(
    id: MatchId,
    mid: &str,
    index: usize,
    registry: &State<Arc<Registry>>,
) -> Option<SegmentBytes> {
    let source = registry.resolve(id)?;
    segment_bytes(
        source.segment(u64::from_str_radix(mid, 16).ok()?, index, true)?,
        false,
    )
}

/// "<match hex> <segments> <live 0|1>" of the match the hub shows, as
/// `view.rs` describes.
#[rocket::get("/g/<id>/info")]
fn match_info(
    id: MatchId,
    registry: &State<Arc<Registry>>,
    usage: &State<Arc<Usage>>,
    cookies: &CookieJar<'_>,
) -> Option<MatchInfo> {
    let (shown, segments, live) = registry.resolve(id)?.info()?;
    usage.replay(cookies, &id.to_string());
    Some(MatchInfo(
        format!("{shown:016x} {segments} {}", u8::from(live)),
        // The open match grows a segment every 30 s.
        Header::new("Cache-Control", "no-store"),
    ))
}

/// The UUID of match `mid` as shown under `id`, for the page's Copy link:
/// the broadcast shows many matches under one UUID.
#[rocket::get("/g/<id>/match/<mid>/uuid")]
fn match_uuid(id: MatchId, mid: &str, registry: &State<Arc<Registry>>) -> Option<MatchInfo> {
    let source = registry.resolve(id)?;
    let uuid = source.uuid_of(u64::from_str_radix(mid, 16).ok()?, id)?;
    Some(MatchInfo(
        uuid.to_string(),
        // The pair never changes.
        Header::new("Cache-Control", "public, max-age=31536000, immutable"),
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
#[rocket::get("/g/<id>/match/<mid>/summary?<turn>")]
fn match_summary(
    id: MatchId,
    mid: &str,
    turn: Option<u32>,
    accepts: web::AcceptsBrotli,
    registry: &State<Arc<Registry>>,
) -> Option<web::Encoded<SharedBytes>> {
    let summary = registry
        .resolve(id)?
        .summary(u64::from_str_radix(mid, 16).ok()?, turn)?;
    summary_response(summary, accepts.accepts())
}

/// The statistics summary of the visitor's own seat, undelayed: only what
/// the seat itself may see in the summary.
#[rocket::get("/g/<id>/play/summary?<turn>")]
fn play_summary(
    id: MatchId,
    turn: Option<u32>,
    accepts: web::AcceptsBrotli,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<web::Encoded<SharedBytes>> {
    let metadata = registry
        .hosted(id)?
        .play
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

/// `private`: a seat's segment depends on the cookie, which a shared cache
/// must not hand on (D26).
fn segment_bytes((bytes, closed): (Message, bool), private: bool) -> Option<SegmentBytes> {
    // The match id is in the URL, so a closed segment never changes; the
    // open one grows every turn.
    let cache = if closed {
        let scope = if private { "private" } else { "public" };
        Header::new(
            "Cache-Control",
            format!("{scope}, max-age=31536000, immutable"),
        )
    } else {
        Header::new("Cache-Control", "no-store")
    };
    Some(SegmentBytes(bytes.to_vec(), cache))
}

struct Args {
    socket: String,
    observe: Option<(String, String)>,
    config: Config,
    delay: Duration,
    history_dir: PathBuf,
    /// 0: no history.
    history_limit: u64,
    usage_dir: PathBuf,
    stats_key: Option<String>,
}

/// `[SOCKET] [--observe HOST:PORT | [--slots N] [--port-base PORT]]
/// [--max-observers N] [--admin-key KEY] [--delay SECONDS] [--history DIR]
/// [--history-limit SIZE] [--usage DIR] [--engine-dir DIR] [--stats-key KEY]
/// [--wall-limit MINUTES] [--game-limit MINUTES]`, in any order.
fn parse_args() -> Result<Args, String> {
    let mut socket = None;
    let mut observe = None;
    let mut slots: Option<u16> = None;
    let mut port_base: Option<u16> = None;
    let mut max_observers = config::DEFAULT_MAX_OBSERVERS;
    let mut admin_key = None;
    let mut delay = Duration::from_secs(DEFAULT_DELAY_SECS);
    let mut history_dir = PathBuf::from(DEFAULT_HISTORY_DIR);
    let mut history_limit = DEFAULT_HISTORY_LIMIT;
    let mut usage_dir = PathBuf::from(DEFAULT_USAGE_DIR);
    let mut engine_dir = PathBuf::from(config::DEFAULT_ENGINE_DIR);
    let mut stats_key = None;
    let mut wall_limit = config::DEFAULT_LIMIT_MINUTES;
    let mut game_limit = config::DEFAULT_LIMIT_MINUTES;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--delay" {
            let seconds = args.next().ok_or("--delay needs SECONDS")?;
            let seconds = seconds
                .parse::<u32>()
                .map_err(|_| format!("--delay {seconds}: expected whole seconds"))?;
            delay = Duration::from_secs(u64::from(seconds));
        } else if arg == "--wall-limit" || arg == "--game-limit" {
            let minutes = args.next().ok_or_else(|| format!("{arg} needs MINUTES"))?;
            let minutes = minutes
                .parse::<u32>()
                .ok()
                .filter(|&minutes| minutes > 0)
                .ok_or_else(|| format!("{arg} {minutes}: expected whole minutes, at least 1"))?;
            if arg == "--wall-limit" {
                wall_limit = minutes;
            } else {
                game_limit = minutes;
            }
        } else if arg == "--history" {
            history_dir = args.next().ok_or("--history needs DIR")?.into();
        } else if arg == "--history-limit" {
            let size = args.next().ok_or("--history-limit needs SIZE")?;
            history_limit = history::parse_size(&size).ok_or_else(|| {
                format!("--history-limit {size}: expected bytes or a size such as 500M or 1G")
            })?;
        } else if arg == "--engine-dir" {
            engine_dir = args.next().ok_or("--engine-dir needs DIR")?.into();
        } else if arg == "--usage" {
            usage_dir = args.next().ok_or("--usage needs DIR")?.into();
        } else if arg == "--stats-key" {
            let key = args.next().filter(|key| !key.is_empty());
            stats_key = Some(key.ok_or("--stats-key needs KEY")?);
        } else if arg == "--admin-key" {
            let key = args.next().filter(|key| !key.is_empty());
            admin_key = Some(key.ok_or("--admin-key needs KEY")?);
        } else if arg == "--slots" {
            let n = args.next().ok_or("--slots needs N")?;
            slots = Some(
                n.parse()
                    .map_err(|_| format!("--slots {n}: expected a number of matches"))?,
            );
        } else if arg == "--port-base" {
            let port = args.next().ok_or("--port-base needs PORT")?;
            port_base = Some(
                port.parse()
                    .map_err(|_| format!("--port-base {port}: expected a port"))?,
            );
        } else if arg == "--max-observers" {
            let n = args.next().ok_or("--max-observers needs N")?;
            max_observers = n
                .parse()
                .map_err(|_| format!("--max-observers {n}: expected a number of observers"))?;
        } else if arg == "--observe" {
            let target = args.next().ok_or("--observe needs HOST:PORT")?;
            let (host, port) = target
                .rsplit_once(':')
                .filter(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
                .ok_or_else(|| format!("--observe {target}: expected HOST:PORT"))?;
            observe = Some((host.to_string(), port.to_string()));
        } else if arg == "--play" || arg == "--play-port" {
            return Err(format!(
                "{arg} is gone: hosting is on by default, --slots 0 turns it off"
            ));
        } else if socket.is_none() && !arg.starts_with("--") {
            socket = Some(arg);
        } else {
            return Err(format!("unexpected argument {arg}"));
        }
    }
    // Both would feed the one socket.
    if observe.is_some() && (slots.is_some() || port_base.is_some()) {
        return Err("--observe excludes --slots and --port-base".to_string());
    }
    let slots = if observe.is_some() {
        0
    } else {
        slots.unwrap_or(config::DEFAULT_SLOTS)
    };
    let port_base = port_base.unwrap_or(config::DEFAULT_PORT_BASE);
    if slots > 0 && u32::from(port_base) + u32::from(slots) - 1 > u32::from(u16::MAX) {
        return Err(format!(
            "--slots {slots} from --port-base {port_base} runs past port 65535"
        ));
    }
    let engine_dir = std::path::absolute(&engine_dir)
        .map_err(|error| format!("--engine-dir {}: {error}", engine_dir.display()))?;
    Ok(Args {
        socket: socket.unwrap_or_else(|| DEFAULT_SOCKET.to_string()),
        observe,
        config: Config {
            slots,
            port_base,
            max_observers,
            admin_key,
            engine_dir,
            wall_limit: Duration::from_secs(u64::from(wall_limit) * 60),
            game_limit: Duration::from_secs(u64::from(game_limit) * 60),
        },
        delay,
        history_dir,
        history_limit,
        usage_dir,
        stats_key,
    })
}

// Only a socket is removed, so a mistyped path cannot delete a real file.
pub(crate) fn remove_stale_socket(path: &str) -> io::Result<()> {
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

pub(crate) fn serve(
    stream: UnixStream,
    hub: &Arc<Hub>,
    play: Option<&Arc<Play>>,
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
                if let Some(play) = play {
                    play.set_summary(summary);
                }
            }
            Record::Pause(players) => hub.set_pause(&players),
            Record::KeepAlive => hub.feed_alive(),
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
