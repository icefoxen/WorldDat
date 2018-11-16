extern crate quinn;
extern crate tokio;
extern crate tokio_io;
#[macro_use]
extern crate failure;
extern crate futures;
extern crate structopt;
extern crate url;

// #[macro_use(slog_o)]
extern crate slog;
extern crate slog_async;
extern crate slog_scope;
extern crate slog_stdlog;
extern crate slog_term;

extern crate blake2;
extern crate bytes;
extern crate chrono;
#[macro_use]
extern crate log;
extern crate fern;
extern crate rmp;
extern crate rmp_serde;
extern crate rustls;
extern crate serde;
#[macro_use]
extern crate serde_derive;
// lazy_static used in unit tests
#[allow(unused_imports)]
#[macro_use]
extern crate lazy_static;
extern crate base64;
extern crate rand;

use structopt::StructOpt;

use std::net::SocketAddr;
use std::path::PathBuf;

// mod connection_tests;
mod hash;
// mod peer;
// mod peer2;
mod types;
mod worker;

// #[cfg(test)]
// mod tests;

fn setup_logging() {
    use fern::colors::{Color, ColoredLevelConfig};
    let colors = ColoredLevelConfig::default()
        .info(Color::Green)
        .debug(Color::BrightMagenta)
        .trace(Color::BrightBlue);
    // This sets up a `fern` logger and initializes `log`.
    fern::Dispatch::new()
        // Formats logs
        .format(move |out, message, record| {
            out.finish(format_args!(
                "[{}][{:<5}][{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                // BUGGO TODO: The coloring breaks the justification...
                // Probably 'cause the control characters mess with the
                // character count.  :|
                colors.color(record.level()),
                record.target(),
                message
            ))
        }).level(log::LevelFilter::Trace)
        .level_for("tokio_reactor", log::LevelFilter::Warn)
        .level_for("mio", log::LevelFilter::Warn)
        .level_for("rustls", log::LevelFilter::Warn)
        // Hooks up console output.
        .chain(std::io::stdout())
        .apply()
        .expect("Could not init logging!");
}

#[derive(StructOpt, Debug, Clone)]
pub struct PeerOpt {
    /// Initial node to connect to, if any.
    #[structopt(short = "b", long = "bootstrap")]
    bootstrap_peer: Option<SocketAddr>,

    /// Address to listen on for incoming connections.
    /// Currently we always listen but it would be nice to have
    /// a fetch-only peer sometime.
    #[structopt(short = "l", long = "listen", default_value = "[::]:4433")]
    listen: SocketAddr,

    /// Certificate authority key.
    #[structopt(parse(from_os_str), long = "ca")]
    ca: Option<PathBuf>,

    // TODO: External files here is not necessarily nicest for unit testing.
    // ALSO TODO: Can we make them optional?  Not yet.
    // See https://github.com/djc/quinn/issues/35
    /// TLS private key in PEM format.
    #[structopt(parse(from_os_str), short = "k", long = "key")]
    key: PathBuf,
    /// TLS certificate in PEM format.
    #[structopt(parse(from_os_str), short = "c", long = "cert")]
    cert: PathBuf,

    /// Whether or not to output low-level QUIC protocol logging,
    /// useful for debugging.
    #[structopt(long = "logproto")]
    logproto: bool,
}

use std::collections::HashMap;
struct WorkerSim {
    workers: HashMap<SocketAddr, worker::WorkerHandle>,
    bootstrap: Option<SocketAddr>,
}

impl WorkerSim {
    fn new() -> Self {
        Self {
            workers: HashMap::new(),
            bootstrap: None,
        }
    }

    /// Sets which bootstrap address new workers will attempt to connect to.
    fn set_bootstrap(&mut self, bootstrap: &str) {
        self.bootstrap = Some(bootstrap.parse().unwrap());
    }

    /// Adds an already-created worker with the given address.
    fn add_worker(&mut self, addr: SocketAddr, worker: worker::WorkerHandle) {
        self.workers.insert(addr, worker);
    }

    /// Creates a new worker with a random ID and adds it.  The string must parse to a valid `SocketAddr`.
    ///
    /// If we have a bootstrap address, the worker will send a message to it trying to look
    /// up its own address to get some nearby peers.
    fn add_new_worker(&mut self, addr: &str) {
        let w_id = types::PeerId::new_insecure_random();
        let w_addr = addr.parse().unwrap();
        let w = worker::WorkerState::start(w_id);
        if let Some(bootstrap) = self.bootstrap {
            w.controller()
                .message(bootstrap, types::Message::Ping { id: w_id })
                .unwrap();
            w.controller()
                .message(bootstrap, types::Message::FindPeer { id: w_id })
                .unwrap();
        }
        self.add_worker(w_addr, w);
    }

    fn run(&mut self) {
        // Source, destination, message
        let mut messages: Vec<(SocketAddr, SocketAddr, types::Message)> = vec![];
        loop {
            // Collect messages.
            // Crudely.
            for (src, worker) in &self.workers {
                while let Ok((dst, msg)) = worker.recv_message() {
                    messages.push((*src, dst, msg));
                }
            }
            for (src, dest, msg) in messages.drain(..) {
                // Messages to unknown addresses get ignored.
                if let Some(worker) = self.workers.get(&dest) {
                    // TODO: This is inefficient since it creates a clone
                    worker
                        .controller()
                        .message(src, msg)
                        .expect("Sent message to nonexistent worker?")
                } else {
                    warn!("Message sent to unknown peer at address: {}", dest);
                }
            }
            // Don't hog CPU.
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
    }
    fn quit(mut self) {
        for (_addr, worker) in self.workers.drain() {
            worker.quit().unwrap();
        }
    }
}

/// I'm really sick of fucking around with networking, and so
/// am just going to simulate things.
fn heckin_simulator() {
    let mut sim = WorkerSim::new();
    sim.set_bootstrap("10.0.0.1:4433");
    sim.add_new_worker("10.0.0.1:4433");
    sim.add_new_worker("10.0.0.2:4433");
    // sim.add_new_worker("10.0.0.3:4433");
    // sim.add_new_worker("10.0.0.4:4433");
    sim.run();
    sim.quit();
}

fn main() {
    setup_logging();
    // let opt = PeerOpt::from_args();
    heckin_simulator();
    // let peer = peer2::Peer::new(opt).expect("Could not create peer struct?");
    // peer.run().expect("Peer did not exit successfully?");
}
