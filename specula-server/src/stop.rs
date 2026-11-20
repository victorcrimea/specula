//! D22: the first SIGINT or SIGTERM lets the hosted matches play to their
//! end and then stops the web server; a second one kills everything.

use std::sync::Arc;

use rocket::Shutdown;
use rocket::tokio::signal::unix::Signal;
use rocket::tokio::signal::unix::SignalKind;
use rocket::tokio::signal::unix::signal;

use crate::observer;
use crate::registry::Registry;

/// `observe_socket`: the `--observe` engine's socket, killed with the rest
/// on a second signal.
pub async fn watch(registry: Arc<Registry>, shutdown: Shutdown, observe_socket: Option<String>) {
    let (mut interrupt, mut terminate) = match (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
    ) {
        (Ok(interrupt), Ok(terminate)) => (interrupt, terminate),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("cannot handle signals: {error}");
            return;
        }
    };
    next(&mut interrupt, &mut terminate).await;
    let running = registry.stop();
    println!(
        "stop requested: refusing new games, waiting for {running} running \
         match(es) to end; a second signal kills them"
    );
    let stopper = Arc::clone(&registry);
    // `wait_stopped` blocks on std locks and sleeps, so it gets a thread
    // rather than a slot in Rocket's async runtime.
    std::thread::spawn(move || {
        stopper.wait_stopped();
        println!("every match has ended, stopping");
        shutdown.notify();
    });
    next(&mut interrupt, &mut terminate).await;
    eprintln!("second signal: killing every engine");
    if let Some(socket) = &observe_socket {
        observer::kill_recorded(socket);
    }
    registry.kill_all();
    std::process::exit(1);
}

async fn next(interrupt: &mut Signal, terminate: &mut Signal) {
    rocket::tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
}
