mod actors;
mod catalog;
mod chat;
mod classify;
mod config;
mod delta;
mod detail;
mod feed;
mod flare;
mod groups;
mod history;
mod lease;
mod limit;
mod nickname;
mod observer;
mod pause;
mod play;
mod registry;
mod replay_zip;
mod score;
mod seats;
mod settings;
mod setup;
mod stop;
mod terrain;
mod thumb;
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
use std::time::Instant;

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
use rocket::tokio::time::MissedTickBehavior;
use rocket_ws::Channel;
use rocket_ws::WebSocket;
use rocket_ws::frame::CloseCode;
use rocket_ws::frame::CloseFrame;
use specula_assets::command::COMMAND_VERSION;
use specula_assets::command::Command as Order;
use specula_assets::command::GROUP_TAG;
use specula_assets::command::GroupContents;
use specula_assets::command::GroupSave;
use specula_assets::command::OBSERVERS_FULL;
use specula_assets::command::OBSERVERS_HIDDEN;
use specula_assets::command::SELECTION_DEFINE_TAG;
use specula_assets::command::SELECTION_USE_TAG;
use specula_assets::command::TAKEN_OVER;
use specula_assets::command::Tokened;

use crate::classify::Classifier;
use crate::config::Config;
use crate::delta::DeltaMeter;
use crate::feed::Record;
use crate::history::History;
use crate::lease::Lease;
use crate::play::Play;
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
                     [--game-limit MINUTES] [--allow-ai-only]";
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

    // Only hosting sets games up; an observer needs none of it.
    let catalog = if config.slots == 0 {
        None
    } else {
        match catalog::data_dir().and_then(|root| Ok((catalog::Catalog::load(&root)?, root))) {
            Ok((catalog, root)) => {
                println!("{}", catalog.summary(&root));
                Some(catalog)
            }
            Err(error) => {
                eprintln!("cannot read the setup catalog: {error}");
                std::process::exit(1);
            }
        }
    };

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
        catalog,
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
                lobby,
                match_thumb,
                match_replay,
                play_new,
                setup_socket,
                chat_socket,
                play_action,
                play_kick,
                play_ban_member,
                observer_flare,
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
    if registry.hidden(id) {
        return Some(close_with(socket, OBSERVERS_HIDDEN, "hidden"));
    }
    let visitor = usage::visitor(cookies);
    let Ok(slot) = registry.observer_slot(id, visitor.as_deref()) else {
        return Some(close_with(socket, OBSERVERS_FULL, "full"));
    };
    let watch = usage.watch(cookies);
    let counted = hub.observer_guard();
    Some(relay(socket, (watch, counted, slot), move || {
        hub.subscribe(View::Observer)
    }))
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
    if registry.hidden(id) {
        return Some(close_with(socket, OBSERVERS_HIDDEN, "hidden"));
    }
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
    // A setup is its own page (D13); one that closed says why.
    if registry.setup_page(id) {
        return web::render("setup.html", pkg, assets, atlases, usage, cookies, referrer)
            .await
            .map(web::IndexPage::Page);
    }
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
    if registry.setup_page(id) {
        return Some(web::IndexPage::Redirect(Box::new(
            rocket::response::Redirect::to(format!("/g/{id}")),
        )));
    }
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
                watch: false,
                rematch: rematch.unwrap_or(false),
                ai_only: false,
                seat: None,
                limits: None,
                next: None,
            }
        }
    };
    let next = visitor
        .as_deref()
        .and_then(|visitor| registry.rematch_of(id, visitor));
    let status = play::Status {
        limits: registry.limits(id),
        next,
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

/// A running match's thumbnail (M41), what an observer would see now.
#[rocket::get("/g/<id>/thumb")]
async fn match_thumb(id: MatchId, registry: &State<Arc<Registry>>) -> Option<Thumbnail> {
    let found = registry.hosted(id)?;
    let bytes = rocket::tokio::task::spawn_blocking(move || found.thumb_webp())
        .await
        .ok()??;
    Some(Thumbnail(
        bytes.to_vec(),
        Header::new("Cache-Control", "public, max-age=10"),
    ))
}

#[derive(rocket::Responder)]
#[response(content_type = "image/webp")]
struct Thumbnail(Vec<u8>, Header<'static>);

/// A hosted match's replay for vanilla 0 A.D., once observers have reached
/// its end, so the download gives nothing of the delay away.
#[rocket::get("/g/<id>/replay.zip")]
async fn match_replay(id: MatchId, registry: &State<Arc<Registry>>) -> Option<ReplayZip> {
    let source = registry.resolve(id)?;
    if !source.replay_only() {
        return None;
    }
    let (files, started_ms) = source.replay_files()?;
    let folder = replay_zip::folder_name(started_ms, &id.to_string());
    let disposition = format!("attachment; filename=\"{folder}.zip\"");
    let bytes = rocket::tokio::task::spawn_blocking(move || {
        let mut read = Vec::with_capacity(files.len());
        for (name, path) in files {
            read.push((name, std::fs::read(path).ok()?));
        }
        let zipped = replay_zip::build(&folder, &read, started_ms / 1000);
        if let Err(error) = &zipped {
            eprintln!("cannot zip the replay of {folder}: {error}");
        }
        zipped.ok()
    })
    .await
    .ok()??;
    Some(ReplayZip(
        bytes,
        Header::new("Content-Disposition", disposition),
        Header::new("Cache-Control", "no-store"),
    ))
}

#[derive(rocket::Responder)]
#[response(content_type = "application/zip")]
struct ReplayZip(Vec<u8>, Header<'static>, Header<'static>);

/// The lobby's lists (M40); a visitor's own game is in `/new`.
#[rocket::get("/lobby.json?<replays>")]
fn lobby(registry: &State<Arc<Registry>>, replays: Option<usize>) -> Option<Json> {
    if registry.broadcast().is_some() {
        return None;
    }
    Some(Json(
        registry.lobby_json(replays.unwrap_or(20).min(200)),
        Header::new("Cache-Control", "no-store"),
    ))
}

/// The setup form's state: whether the server is full and which game, if
/// any, is the visitor's.
#[rocket::get("/new")]
fn new_status(registry: &State<Arc<Registry>>, cookies: &CookieJar<'_>) -> Option<Json> {
    if registry.broadcast().is_some() {
        return None;
    }
    let own = usage::visitor(cookies).and_then(|visitor| registry.own_hosted(&visitor));
    Some(Json(
        play::form_json(
            own.is_some() || !registry.slot_free(),
            own,
            registry.latest_running(),
            registry.stopping(),
        ),
        Header::new("Cache-Control", "no-store"),
    ))
}

/// Opens a setup for the visitor (D13) and answers its UUID; the text says
/// why not. The form is the nickname only.
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
    let nickname = form
        .get("nickname")
        .and_then(|name| nickname::Nickname::parse(name))
        .ok_or(refuse("bad nickname"))?;
    // A stale batch is the browser's, so it only means defaults, never a
    // refusal.
    let batch = form
        .get("settings")
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .filter(serde_json::Value::is_object);
    registry
        .open_setup(&visitor, nickname, None, batch.as_ref())
        .map(|id| id.to_string())
        .map_err(refuse)
}

/// The player's page's Resign, Pause, Claim, End, Exit, Rematch and Flare,
/// for the visitor's game; the text says why not. A rematch answers the new
/// game's UUID.
#[rocket::post("/g/<id>/play/<action>?<x>&<z>&<tab>")]
fn play_action(
    id: MatchId,
    action: &str,
    x: Option<f32>,
    z: Option<f32>,
    tab: Option<&str>,
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
    if !play.holds(&visitor, tab) {
        return Err(refuse(TAKEN_OVER_REASON));
    }
    match action {
        "resign" => play.resign(&visitor).map(|()| String::new()),
        "pause" => play.set_paused(&visitor, true).map(|()| String::new()),
        "resume" => play.set_paused(&visitor, false).map(|()| String::new()),
        "claim" => play.claim(&visitor).map(|()| String::new()),
        "end" => play.end(&visitor).map(|()| String::new()),
        "flare" => match (x, z) {
            (Some(x), Some(z)) => play.flare(&visitor, x, z).map(|()| String::new()),
            _ => Err("no position"),
        },
        _ => Err("unknown action"),
    }
    .map_err(refuse)
}

/// The refusal of an action from a page that no longer holds the seat
/// (D64).
const TAKEN_OVER_REASON: &str = "This seat is controlled from another window";

/// An observer's flare, for the other observers only (D51).
#[rocket::post("/g/<id>/flare?<x>&<z>")]
fn observer_flare(
    id: MatchId,
    x: f32,
    z: f32,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Result<(), rocket::response::status::BadRequest<&'static str>> {
    let refuse = rocket::response::status::BadRequest;
    let visitor = usage::visitor(cookies).ok_or(refuse("no visitor cookie"))?;
    let hub = registry
        .observable(id)
        .and_then(|found| found.hub)
        .ok_or(refuse("no match"))?;
    // The chat's nickname, if the visitor ever joined it; a broadcast has
    // no chat, so its observers are nameless.
    let name = registry
        .hosted(id)
        .and_then(|found| found.seats.nickname_of(&visitor));
    hub.flare_from_observer(
        &visitor,
        name.as_ref().map(nickname::Nickname::as_str),
        x,
        z,
    )
    .map_err(refuse)
}

/// The host's kick of a seat from the diplomacy window (M29), with a ban
/// when `ban=true` (D54).
#[rocket::post("/g/<id>/play/kick/<player>?<ban>&<tab>")]
fn play_kick(
    id: MatchId,
    player: u8,
    ban: Option<bool>,
    tab: Option<&str>,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Result<String, rocket::response::status::BadRequest<&'static str>> {
    let refuse = rocket::response::status::BadRequest;
    let visitor = usage::visitor(cookies).ok_or(refuse("no visitor cookie"))?;
    let found = registry.hosted(id).ok_or(refuse("not in play mode"))?;
    if !found.play.holds(&visitor, tab) {
        return Err(refuse(TAKEN_OVER_REASON));
    }
    found
        .play
        .kick(&visitor, player, ban.unwrap_or(false))
        .map(|()| String::new())
        .map_err(refuse)
}

/// The host's ban of an observer member from the chat window (D54).
#[rocket::post("/g/<id>/play/ban_member/<member>?<tab>")]
fn play_ban_member(
    id: MatchId,
    member: usize,
    tab: Option<&str>,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Result<String, rocket::response::status::BadRequest<&'static str>> {
    let refuse = rocket::response::status::BadRequest;
    let visitor = usage::visitor(cookies).ok_or(refuse("no visitor cookie"))?;
    let found = registry.hosted(id).ok_or(refuse("not in play mode"))?;
    if !found.play.holds(&visitor, tab) {
        return Err(refuse(TAKEN_OVER_REASON));
    }
    found
        .play
        .ban_observer(&visitor, member)
        .map(|()| String::new())
        .map_err(refuse)
}

/// Sent on a setup socket that closes as it opens, or for a match that is
/// no setup any more.
fn done_channel(socket: WebSocket, text: String) -> Channel<'static> {
    socket.channel(move |mut stream| {
        Box::pin(async move {
            stream.send(rocket_ws::Message::text(text)).await?;
            Ok(())
        })
    })
}

/// Counts a visitor's socket in the room and, once they are a member, in
/// the seat table (U2); undone when the stream closes.
struct SetupGuard {
    found: Arc<registry::Match>,
    cookie: String,
    counted: bool,
}

impl SetupGuard {
    /// A socket counts for its member once, however often they join.
    fn member(&mut self) {
        if !self.counted {
            self.counted = self.found.seats.connect(&self.cookie);
        }
    }
}

impl Drop for SetupGuard {
    fn drop(&mut self) {
        self.found.room.close_socket();
        if self.counted {
            self.found.seats.disconnect(&self.cookie);
        }
    }
}

/// The setup page's socket (M15): the catalog, then the whole state at
/// every change, and the visitor's requests back. Anything that is not the
/// visitor's to ask is dropped.
#[rocket::get("/g/<id>/setup")]
fn setup_socket(
    id: MatchId,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    if let Some(reason) = registry.closed_reason(id) {
        return Some(done_channel(socket, setup::closed_json(reason)));
    }
    let Some(found) = registry
        .hosted(id)
        .filter(|found| found.state() == registry::MatchState::Setup)
    else {
        // Launched, or on its way: the page reloads into the game.
        registry.resolve(id)?;
        return Some(done_channel(socket, r#"{"type":"launched"}"#.to_string()));
    };
    let cookie = usage::visitor(cookies)?;
    if found.seats.is_banned(&cookie) {
        return Some(done_channel(socket, setup::kicked_json(true)));
    }
    let kicks = found.seats.kicks_of(&cookie);
    let registry = Arc::clone(registry);
    found.room.open_socket();
    let mut guard = SetupGuard {
        found: Arc::clone(&found),
        cookie: cookie.clone(),
        counted: false,
    };
    guard.member();
    Some(socket.channel(move |mut stream| {
        Box::pin(async move {
            let mut changes = guard.found.room.subscribe();
            let Some((catalog, catalog_json)) = registry.catalog() else {
                return Ok(());
            };
            stream.send(rocket_ws::Message::text(catalog_json)).await?;
            loop {
                if guard.found.seats.kicks_of(&cookie) != kicks {
                    let ban = guard.found.seats.is_banned(&cookie);
                    stream
                        .send(rocket_ws::Message::text(setup::kicked_json(ban)))
                        .await?;
                    return Ok(());
                }
                let (state, phase) =
                    guard
                        .found
                        .room
                        .state_json(&guard.found.seats, catalog, Some(&cookie));
                match phase {
                    // Nothing while launching: the next state is `launched`,
                    // or the setup open again.
                    setup::Phase::Launching => {}
                    setup::Phase::Open => stream.send(rocket_ws::Message::text(state)).await?,
                    setup::Phase::Launched | setup::Phase::Closed(_) => {
                        stream.send(rocket_ws::Message::text(state)).await?;
                        return Ok(());
                    }
                }
                // Until the next change; requests are served as they come.
                loop {
                    rocket::tokio::select! {
                        biased;
                        incoming = stream.next() => {
                            let Some(message) = incoming else {
                                return Ok(());
                            };
                            let rocket_ws::Message::Text(text) = message? else {
                                continue;
                            };
                            let Some(request) = setup::parse(&text) else {
                                continue;
                            };
                            let joined = matches!(request, setup::Request::Join(_));
                            let launch = matches!(request, setup::Request::Launch);
                            let result = registry.setup_request(id, &cookie, request);
                            if joined {
                                guard.member();
                            }
                            if let (true, Err(reason)) = (launch, result) {
                                let error = serde_json::json!({ "type": "error", "text": reason });
                                stream
                                    .send(rocket_ws::Message::text(error.to_string()))
                                    .await?;
                            }
                        }
                        changed = changes.changed() => {
                            if changed.is_err() {
                                return Ok(());
                            }
                            break;
                        }
                    }
                }
            }
        })
    }))
}

/// The match's chat (M31) from its setup to its end: the backlog this
/// visitor may read, then each new entry, which an unseated viewer gets
/// from a seat only after the delay (D49). Sending needs a member, and
/// `join` makes an observer one. The socket counts for nothing else: a
/// seat's presence stays with the setup and command sockets.
#[rocket::get("/g/<id>/chat")]
fn chat_socket(
    id: MatchId,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let Some(found) = registry.hosted(id) else {
        // Dropped once ended, or a broadcast that has no chat: nothing more
        // comes either way.
        if registry.closed_reason(id).is_some() || registry.resolve(id).is_some() {
            return Some(done_channel(socket, r#"{"type":"closed"}"#.to_string()));
        }
        return None;
    };
    let cookie = usage::visitor(cookies);
    // Observers of a hidden match have no chat either; a defeated player
    // still holds a seat, so theirs goes on.
    if found.hidden()
        && !cookie
            .as_deref()
            .is_some_and(|cookie| found.seats.player_of(cookie).is_some())
    {
        return Some(done_channel(socket, r#"{"type":"closed"}"#.to_string()));
    }
    let config = rocket_ws::Config {
        max_message_size: Some(chat::MAX_FRAME),
        max_frame_size: Some(chat::MAX_FRAME),
        ..Default::default()
    };
    Some(socket.config(config).channel(move |mut stream| {
        Box::pin(async move {
            let _presence = found.chat.present(cookie.as_deref());
            let mut changes = found.chat.subscribe();
            stream
                .send(rocket_ws::Message::text(chat::you_json(
                    &found,
                    cookie.as_deref(),
                )))
                .await?;
            let mut cursor = 0u64;
            let mut pending: Vec<(Instant, String)> = Vec::new();
            let mut last_members = String::new();
            let mut live_sent = false;
            // A seat taken in the setup, a kick or a defeat changes the
            // members without a chat event, so they are polled.
            let mut tick = rocket::tokio::time::interval(Duration::from_secs(2));
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                // Read before taking, so an entry posted before the close is
                // among those taken.
                let closed = found.chat.is_closed();
                let viewer = chat::viewer(&found, cookie.as_deref());
                pending.extend(found.chat.take(&mut cursor, &viewer));
                let now = Instant::now();
                let mut held = Vec::new();
                for (due, json) in std::mem::take(&mut pending) {
                    if due <= now {
                        stream.send(rocket_ws::Message::text(json)).await?;
                    } else {
                        held.push((due, json));
                    }
                }
                pending = held;
                let members = chat::members_json(&found, cookie.as_deref());
                if members != last_members {
                    stream
                        .send(rocket_ws::Message::text(members.clone()))
                        .await?;
                    last_members = members;
                }
                // The page skips the backlog for its overlay and sound.
                if !live_sent {
                    live_sent = true;
                    stream
                        .send(rocket_ws::Message::text(r#"{"type":"live"}"#.to_string()))
                        .await?;
                }
                // A closed match still delivers what is held for the delay,
                // since its last messages are not yet due.
                if closed && pending.is_empty() {
                    stream
                        .send(rocket_ws::Message::text(r#"{"type":"closed"}"#.to_string()))
                        .await?;
                    return Ok(());
                }
                let wake = pending.iter().map(|(due, _)| *due).min();
                let wake_at = wake.unwrap_or(now);
                rocket::tokio::select! {
                    biased;
                    incoming = stream.next() => {
                        let Some(message) = incoming else {
                            return Ok(());
                        };
                        let rocket_ws::Message::Text(text) = message? else {
                            continue;
                        };
                        let Some(request) = chat::parse(&text) else {
                            continue;
                        };
                        if let Some(reply) = chat::answer(&found, cookie.as_deref(), request) {
                            stream.send(rocket_ws::Message::text(reply)).await?;
                        }
                    }
                    changed = changes.changed() => {
                        if changed.is_err() {
                            return Ok(());
                        }
                    }
                    _ = rocket::tokio::time::sleep_until(
                        rocket::tokio::time::Instant::from_std(wake_at)
                    ), if wake.is_some() => {}
                    _ = tick.tick() => {}
                }
            }
        })
    }))
}

/// The stream of the visitor's seat: undelayed, and only what its player
/// sees. A visitor without a seat gets none.
fn seat_view(found: &registry::Match, cookies: &CookieJar<'_>) -> Option<View> {
    let seat = found.play.seat(usage::visitor(cookies).as_deref())?;
    Some(View::Seat(u32::from(seat)))
}

/// `seat_view` for a socket of the page `tab`, which now holds the seat
/// (D64); a page without a tab id gets none.
fn seat_stream(
    found: &registry::Match,
    cookies: &CookieJar<'_>,
    tab: Option<&str>,
    kind: lease::Kind,
) -> Option<(View, Lease)> {
    let view = seat_view(found, cookies)?;
    let visitor = usage::visitor(cookies)?;
    let lease = found.play.claim_lease(&visitor, tab, kind)?;
    Some((view, lease))
}

#[rocket::get("/g/<id>/play/ws?<tab>")]
fn play_viewer(
    id: MatchId,
    tab: Option<&str>,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let found = registry.hosted(id)?;
    let (view, lease) = seat_stream(&found, cookies, tab, lease::Kind::View)?;
    let hub = Arc::clone(&found.hub);
    Some(relay_leased(socket, lease, move || hub.subscribe(view)))
}

#[rocket::get("/g/<id>/play/detail?<tab>")]
fn play_detail_viewer(
    id: MatchId,
    tab: Option<&str>,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let found = registry.hosted(id)?;
    let (view, lease) = seat_stream(&found, cookies, tab, lease::Kind::Detail)?;
    let hub = Arc::clone(&found.hub);
    Some(relay_leased(socket, lease, move || {
        hub.subscribe_detail(view)
    }))
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
/// none, then the seat's starting camera (x and z, f32 LE) when the map
/// gives one. The seat is the one held when the socket opened: orders are
/// dropped once the visitor no longer holds it. Orders may name the socket's last selection
/// by token (`Tokened`), kept only for this connection.
#[rocket::get("/g/<id>/play/command?<tab>")]
fn play_command(
    id: MatchId,
    tab: Option<&str>,
    socket: WebSocket,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<Channel<'static>> {
    let found = registry.hosted(id)?;
    let play = Arc::clone(&found.play);
    let hub = Arc::clone(&found.hub);
    let visitor = usage::visitor(cookies);
    let tab = tab.map(str::to_string);
    Some(socket.channel(move |mut stream| {
        Box::pin(async move {
            // A seated page without a tab id commands nothing, or a copied
            // cookie could skip the one-page rule (D64).
            let mut lease = play.seat(visitor.as_deref()).and_then(|_| {
                play.claim_lease(visitor.as_deref()?, tab.as_deref(), lease::Kind::Command)
            });
            let bound = lease.as_ref().and(play.seat(visitor.as_deref()));
            // Only a socket bound to a seat counts against presence, and it
            // counts for as long as the loop below runs (M25).
            let _presence = bound
                .and(visitor.as_deref())
                .map(|cookie| play.present(cookie));
            // The token's list as the feed's ids, for this socket only, since
            // the page numbers its tokens per connection.
            let mut kept: Option<(u32, Vec<u32>)> = None;
            // Before the snapshot, so a push in between is not missed.
            let mut pushes = play.group_pushes();
            let mut hello = vec![COMMAND_VERSION, bound.unwrap_or(0)];
            if let Some([x, z]) = bound.and_then(|seat| play.starting_camera(seat)) {
                hello.extend_from_slice(&x.to_le_bytes());
                hello.extend_from_slice(&z.to_le_bytes());
            }
            stream.send(rocket_ws::Message::binary(hello)).await?;
            if let (Some(seat), Some(cookie)) = (bound, visitor.as_deref()) {
                for contents in play.group_snapshot(cookie, seat) {
                    stream
                        .send(rocket_ws::Message::binary(contents.encode()))
                        .await?;
                }
            }
            loop {
                let message = rocket::tokio::select! {
                    () = taken_over(&mut lease) => return close_taken_over(&mut stream).await,
                    push = pushes.recv() => match push {
                        Ok(push) => {
                            if Some(push.seat) == bound
                                && Some(push.cookie.as_str()) == visitor.as_deref()
                            {
                                let contents = GroupContents {
                                    group: push.group,
                                    entities: push.handles,
                                };
                                stream
                                    .send(rocket_ws::Message::binary(contents.encode()))
                                    .await?;
                            }
                            continue;
                        }
                        Err(RecvError::Lagged(_)) => {
                            if let (Some(seat), Some(cookie)) = (bound, visitor.as_deref()) {
                                for contents in play.group_snapshot(cookie, seat) {
                                    stream
                                        .send(rocket_ws::Message::binary(contents.encode()))
                                        .await?;
                                }
                            }
                            continue;
                        }
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
                let Some(seat) = bound.filter(|seat| play.seat(visitor.as_deref()) == Some(*seat))
                else {
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
                if bytes.first() == Some(&GROUP_TAG) {
                    match GroupSave::decode(&bytes) {
                        Ok(mut save) => {
                            // Unlike an order, a group with a few stale
                            // handles still means something, so those are
                            // left out instead of dropping the save.
                            save.entities.retain_mut(|id| {
                                match hub.seat_entity(u32::from(seat), *id) {
                                    Some(feed) => {
                                        *id = feed;
                                        true
                                    }
                                    None => false,
                                }
                            });
                            if let Some(cookie) = visitor.as_deref() {
                                play.save_group(cookie, seat, save);
                            }
                        }
                        Err(error) => eprintln!("bad control group from the player: {error}"),
                    }
                    continue;
                }
                if matches!(
                    bytes.first(),
                    Some(&SELECTION_DEFINE_TAG | &SELECTION_USE_TAG)
                ) {
                    match Tokened::decode(&bytes) {
                        Ok(Tokened::Define { token, mut command }) => {
                            // The page counts this as kept whatever happens
                            // here, so a refused list must not leave an
                            // older one in its place.
                            kept = None;
                            if to_feed(command.entity_ids_mut()) {
                                let ids = command.selection().cloned().unwrap_or_default();
                                kept = Some((token, ids));
                                play.send(seat, &command);
                            }
                        }
                        Ok(Tokened::Use {
                            token,
                            removed,
                            mut command,
                        }) => match &mut kept {
                            Some((kept_token, ids)) if *kept_token == token => {
                                // Before the order may be refused: the page
                                // has already shrunk its list.
                                let gone: HashSet<u32> = removed
                                    .iter()
                                    .filter_map(|handle| hub.seat_entity(u32::from(seat), *handle))
                                    .collect();
                                ids.retain(|id| !gone.contains(id));
                                // The selection is empty here, so only the
                                // other ids (a target, say) are handles.
                                if to_feed(command.entity_ids_mut()) {
                                    if let Some(selection) = command.selection_mut() {
                                        selection.clone_from(ids);
                                    }
                                    play.send(seat, &command);
                                }
                            }
                            _ => eprintln!("order with an unknown selection token {token}"),
                        },
                        Err(error) => eprintln!("bad tokened order from the player: {error}"),
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
    socket.channel(move |stream| Box::pin(relay_loop(stream, held, None, subscribe)))
}

/// `relay` for a seat's stream, which closes once another page or socket
/// takes the seat over (D64).
fn relay_leased(
    socket: WebSocket,
    lease: Lease,
    subscribe: impl Fn() -> Option<(Vec<Message>, Receiver<Message>)> + Send + 'static,
) -> Channel<'static> {
    socket.channel(move |stream| Box::pin(relay_loop(stream, (), Some(lease), subscribe)))
}

async fn relay_loop(
    mut stream: rocket_ws::stream::DuplexStream,
    held: impl Send + 'static,
    mut lease: Option<Lease>,
    subscribe: impl Fn() -> Option<(Vec<Message>, Receiver<Message>)> + Send + 'static,
) -> rocket_ws::result::Result<()> {
    let _held = held;
    let Some((mut first, mut receiver)) = subscribe() else {
        return Ok(());
    };
    loop {
        for message in first.drain(..) {
            stream.send(rocket_ws::Message::binary(&*message)).await?;
        }
        rocket::tokio::select! {
            () = taken_over(&mut lease) => return close_taken_over(&mut stream).await,
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
}

/// Fires once another page or socket took the seat's slot over (D64);
/// never without a lease.
async fn taken_over(lease: &mut Option<Lease>) {
    if let Some(lease) = lease
        && (&mut lease.closed).await.is_ok()
    {
        return;
    }
    std::future::pending().await
}

/// Accepts the socket only to close it with `code`, so the page learns why;
/// a refused upgrade would look like a network error and be retried.
fn close_with(socket: WebSocket, code: u16, reason: &'static str) -> Channel<'static> {
    socket.channel(move |mut stream| {
        Box::pin(async move {
            let frame = CloseFrame {
                code: CloseCode::from(code),
                reason: reason.into(),
            };
            stream.send(rocket_ws::Message::Close(Some(frame))).await
        })
    })
}

/// Tells a displaced page why its socket closes, so it does not reconnect
/// and take the seat straight back.
async fn close_taken_over(
    stream: &mut rocket_ws::stream::DuplexStream,
) -> rocket_ws::result::Result<()> {
    let frame = CloseFrame {
        code: CloseCode::from(TAKEN_OVER),
        reason: "taken over".into(),
    };
    stream.send(rocket_ws::Message::Close(Some(frame))).await
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
    let source = registry.observable(id)?;
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
    let source = registry.observable(id)?;
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
    let (shown, segments, live) = registry.observable(id)?.info()?;
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
    let source = registry.observable(id)?;
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
/// is the final one, whether the alliances have followed the teams so far
/// (`Hub::teams_held`), and the engine's JSON as it is.
pub(crate) fn summary_envelope(
    turn: u32,
    is_final: bool,
    teams_held: bool,
    metadata: &[u8],
) -> Option<Vec<u8>> {
    std::str::from_utf8(metadata).ok()?;
    let mut body =
        format!("{{\"turn\":{turn},\"final\":{is_final},\"teamsHeld\":{teams_held},\"metadata\":")
            .into_bytes();
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
        .observable(id)?
        .summary(u64::from_str_radix(mid, 16).ok()?, turn)?;
    summary_response(summary, accepts.accepts())
}

/// The statistics summary for the visitor's seat, undelayed: the seat's own
/// view while the game goes on, every player once it is over.
#[rocket::get("/g/<id>/play/summary?<turn>")]
fn play_summary(
    id: MatchId,
    turn: Option<u32>,
    accepts: web::AcceptsBrotli,
    registry: &State<Arc<Registry>>,
    cookies: &CookieJar<'_>,
) -> Option<web::Encoded<SharedBytes>> {
    let summary = match registry.hosted(id) {
        Some(found) => found
            .play
            .summary(usage::visitor(cookies).as_deref(), turn)?,
        // A dropped match has no seats left; what `/` serves of it is no
        // longer ahead of anyone.
        None => registry.resolve(id)?.own_summary(turn)?,
    };
    summary_response(summary, accepts.accepts())
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
    let mut ai_only = false;
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
        } else if arg == "--allow-ai-only" {
            ai_only = true;
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
            ai_only,
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
            Record::Notifications(notes) => hub.notify(&notes),
            Record::Events(json) => hub.notify_events(&json),
            Record::TimeNotifications(notes) => hub.set_time_notifications(notes),
            Record::ControlGroups(groups) => hub.set_control_groups(&groups),
            Record::KeepAlive => hub.feed_alive(),
            Record::SimTime(microseconds) => hub.set_sim_time(microseconds),
            Record::MetadataSaved => {
                if let Some(play) = play {
                    play.metadata_saved();
                }
            }
            Record::InitAttributes(json) => {
                hub.set_lock_teams(&json);
                // The broadcast has no game to compare with.
                if let Some(play) = play {
                    play.check_attributes(&json);
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
                // Before `end_turn` clears the renames.
                if let Some(play) = play {
                    play.update_groups(&entities, &details);
                }
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
