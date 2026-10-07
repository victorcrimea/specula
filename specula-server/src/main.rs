mod classify;
mod delta;
mod feed;
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
use std::sync::Arc;

use rocket::State;
use rocket::fairing::AdHoc;
use rocket::fs::FileServer;
use rocket::futures::SinkExt;
use rocket::futures::StreamExt;
use rocket::http::Header;
use rocket::tokio::sync::broadcast::error::RecvError;
use rocket_ws::Channel;
use rocket_ws::WebSocket;

use crate::classify::Classifier;
use crate::delta::DeltaMeter;
use crate::feed::Record;
use crate::terrain::Ground;
use crate::view::Hub;

const DEFAULT_SOCKET: &str = "/tmp/specula-feed.sock";
/// Relative to the working directory, so the server runs from the repo root.
pub const WEB_ROOT: &str = "specula-web";

#[rocket::main]
async fn main() {
    let (path, observe) = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            eprintln!("usage: specula-server [SOCKET] [--observe HOST:PORT]");
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

    let hub = Arc::new(Hub::new());
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
        .mount(
            "/",
            rocket::routes![
                viewer,
                web::terrain,
                segment_file,
                web::index,
                web::page,
                web::pkg_file,
                web::asset
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
    socket.channel(move |mut stream| {
        Box::pin(async move {
            let (mut first, mut receiver) = hub.subscribe();
            loop {
                for message in first.drain(..) {
                    stream.send(rocket_ws::Message::binary(&*message)).await?;
                }
                rocket::tokio::select! {
                    broadcast = receiver.recv() => match broadcast {
                        Ok(message) => stream.send(rocket_ws::Message::binary(&*message)).await?,
                        // Missed deltas cannot be patched over; start again
                        // from a fresh keyframe.
                        Err(RecvError::Lagged(_)) => (first, receiver) = hub.subscribe(),
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
    let (bytes, closed) = hub.segment(u64::from_str_radix(id, 16).ok()?, index)?;
    // The match id is in the URL, so a closed segment never changes; the
    // open one grows every turn.
    let cache = if closed {
        Header::new("Cache-Control", "public, max-age=31536000, immutable")
    } else {
        Header::new("Cache-Control", "no-store")
    };
    Some(SegmentBytes(bytes.to_vec(), cache))
}

/// `[SOCKET] [--observe HOST:PORT]`, in any order.
fn parse_args() -> Result<(String, Option<(String, String)>), String> {
    let mut socket = None;
    let mut observe = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--observe" {
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
    Ok((
        socket.unwrap_or_else(|| DEFAULT_SOCKET.to_string()),
        observe,
    ))
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
                hub.publish(turn, &entities)?;
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
