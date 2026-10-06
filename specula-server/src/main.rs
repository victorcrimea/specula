mod classify;
mod delta;
mod feed;
mod terrain;
mod view;
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
use crate::view::Hub;

const DEFAULT_SOCKET: &str = "/tmp/specula-feed.sock";
/// Relative to the working directory, so the server runs from the repo root.
const WEB_ROOT: &str = "specula-web";

#[rocket::main]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_SOCKET.to_string());
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

    let result = rocket::build()
        .manage(hub)
        .mount("/", rocket::routes![viewer, terrain_file, segment_file])
        .mount("/", FileServer::from(WEB_ROOT))
        .launch()
        .await;
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
            let (mut keyframe, mut receiver) = hub.subscribe();
            loop {
                if let Some(message) = keyframe.take() {
                    stream.send(rocket_ws::Message::binary(&*message)).await?;
                }
                rocket::tokio::select! {
                    broadcast = receiver.recv() => match broadcast {
                        Ok(message) => stream.send(rocket_ws::Message::binary(&*message)).await?,
                        // Missed deltas cannot be patched over; start again
                        // from a fresh keyframe.
                        Err(RecvError::Lagged(_)) => (keyframe, receiver) = hub.subscribe(),
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
struct TerrainBytes(Vec<u8>, Header<'static>);

#[rocket::get("/terrain/<id>")]
fn terrain_file(id: &str, hub: &State<Arc<Hub>>) -> Option<TerrainBytes> {
    let bytes = hub.terrain(u64::from_str_radix(id, 16).ok()?)?;
    // The id is a hash of the content, so a cached copy is never stale.
    let cache = Header::new("Cache-Control", "public, max-age=31536000, immutable");
    Some(TerrainBytes(bytes.to_vec(), cache))
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
    let mut meter = DeltaMeter::new()?;
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
                hub.set_terrain(resource.id, resource.bytes);
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
