mod classify;
mod delta;
mod detail;
mod feed;
mod history;
mod observer;
mod terrain;
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
use rocket::http::Header;
use rocket::tokio::sync::broadcast::Receiver;
use rocket::tokio::sync::broadcast::error::RecvError;
use rocket_ws::Channel;
use rocket_ws::WebSocket;

use crate::classify::Classifier;
use crate::delta::DeltaMeter;
use crate::feed::Record;
use crate::history::History;
use crate::terrain::Ground;
use crate::view::Hub;
use crate::view::Message;

const DEFAULT_SOCKET: &str = "/tmp/specula-feed.sock";
/// As AoE2 DE's ranked spectating: long enough that a player cannot scout
/// the opponent through the broadcast.
const DEFAULT_DELAY_SECS: u64 = 120;
/// Relative to the working directory, as `WEB_ROOT`.
const DEFAULT_HISTORY_DIR: &str = "history";
/// About 70 one-hour 1v1s or 20 large 4v4s.
const DEFAULT_HISTORY_LIMIT: u64 = 1 << 30;
const USAGE: &str = "usage: specula-server [SOCKET] [--observe HOST:PORT] [--delay SECONDS] \
                     [--history DIR] [--history-limit SIZE]";
/// Relative to the working directory, so the server runs from the repo root.
pub const WEB_ROOT: &str = "specula-web";

#[rocket::main]
async fn main() {
    let Args {
        socket: path,
        observe,
        delay,
        history_dir,
        history_limit,
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

    let hub = Arc::new(Hub::new(delay, history));
    let publisher_hub = Arc::clone(&hub);
    std::thread::spawn(move || publisher_hub.run_publisher());
    let feed_hub = Arc::clone(&hub);
    // The feed is blocking std I/O, so it gets its own thread rather than
    // a slot in Rocket's async runtime.
    std::thread::spawn(move || accept_observers(listener, &feed_hub));
    let supervised = observe.is_some();
    if let Some((host, port)) = observe {
        let supervisor_hub = Arc::clone(&hub);
        let socket = path.clone();
        std::thread::spawn(move || observer::supervise(&host, &port, &socket, &supervisor_hub));
    }

    let result = rocket::build()
        .manage(hub)
        .manage(web::Pkg::default())
        .manage(web::AssetsSize::default())
        .manage(web::AtlasVersions::default())
        .mount(
            "/",
            rocket::routes![
                viewer,
                detail_viewer,
                web::terrain,
                web::data,
                segment_file,
                detail_segment_file,
                match_info,
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

fn accept_observers(listener: UnixListener, hub: &Hub) {
    // One observer at a time: each one is a whole match.
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                println!("observer connected");
                hub.start_feed();
                match serve(stream, hub) {
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
fn viewer(socket: WebSocket, hub: &State<Arc<Hub>>) -> Channel<'static> {
    let hub = Arc::clone(hub);
    relay(socket, move || hub.subscribe())
}

/// The detail stream, which viewers hold open only while they have
/// something selected.
#[rocket::get("/detail")]
fn detail_viewer(socket: WebSocket, hub: &State<Arc<Hub>>) -> Channel<'static> {
    let hub = Arc::clone(hub);
    relay(socket, move || hub.subscribe_detail())
}

fn relay(
    socket: WebSocket,
    subscribe: impl Fn() -> (Vec<Message>, Receiver<Message>) + Send + 'static,
) -> Channel<'static> {
    socket.channel(move |mut stream| {
        Box::pin(async move {
            let (mut first, mut receiver) = subscribe();
            loop {
                for message in first.drain(..) {
                    stream.send(rocket_ws::Message::binary(&*message)).await?;
                }
                rocket::tokio::select! {
                    broadcast = receiver.recv() => match broadcast {
                        Ok(message) => stream.send(rocket_ws::Message::binary(&*message)).await?,
                        // Missed deltas cannot be patched over; start again
                        // from a fresh keyframe.
                        Err(RecvError::Lagged(_)) => (first, receiver) = subscribe(),
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
    segment_bytes(hub.segment(u64::from_str_radix(id, 16).ok()?, index)?)
}

#[rocket::get("/match/<id>/detail/<index>")]
fn detail_segment_file(id: &str, index: usize, hub: &State<Arc<Hub>>) -> Option<SegmentBytes> {
    segment_bytes(hub.detail_segment(u64::from_str_radix(id, 16).ok()?, index)?)
}

/// "<segments> <live 0|1>", as `view.rs` describes.
#[rocket::get("/match/<id>/info")]
fn match_info(id: &str, hub: &State<Arc<Hub>>) -> Option<MatchInfo> {
    let (segments, live) = hub.match_info(u64::from_str_radix(id, 16).ok()?)?;
    Some(MatchInfo(
        format!("{segments} {}", u8::from(live)),
        // The open match grows a segment every 30 s.
        Header::new("Cache-Control", "no-store"),
    ))
}

#[derive(rocket::Responder)]
#[response(content_type = "text")]
struct MatchInfo(String, Header<'static>);

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
    delay: Duration,
    history_dir: PathBuf,
    /// 0: no history.
    history_limit: u64,
}

/// `[SOCKET] [--observe HOST:PORT] [--delay SECONDS] [--history DIR]
/// [--history-limit SIZE]`, in any order.
fn parse_args() -> Result<Args, String> {
    let mut socket = None;
    let mut observe = None;
    let mut delay = Duration::from_secs(DEFAULT_DELAY_SECS);
    let mut history_dir = PathBuf::from(DEFAULT_HISTORY_DIR);
    let mut history_limit = DEFAULT_HISTORY_LIMIT;
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
        } else if arg == "--observe" {
            let target = args.next().ok_or("--observe needs HOST:PORT")?;
            let (host, port) = target
                .rsplit_once(':')
                .filter(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
                .ok_or_else(|| format!("--observe {target}: expected HOST:PORT"))?;
            observe = Some((host.to_string(), port.to_string()));
        } else if socket.is_none() && !arg.starts_with("--") {
            socket = Some(arg);
        } else {
            return Err(format!("unexpected argument {arg}"));
        }
    }
    Ok(Args {
        socket: socket.unwrap_or_else(|| DEFAULT_SOCKET.to_string()),
        observe,
        delay,
        history_dir,
        history_limit,
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

fn serve(stream: UnixStream, hub: &Hub) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    feed::read_header(&mut reader)?;
    let mut templates = HashMap::new();
    let mut details = feed::Details::default();
    let mut classifier = Classifier::default();
    let mut meter = DeltaMeter::new();
    while let Some(record) = feed::read_record(&mut reader)? {
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
                hub.set_terrain(resource.id, resource.bytes, Ground::new(&terrain));
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
