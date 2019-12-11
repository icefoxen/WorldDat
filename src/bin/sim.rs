/// Run with
/// env RUST_LOG=trace cargo run --bin sim
use log::*;
use structopt;
use worlddat::*;

use structopt::StructOpt;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Command line options for a peer node.
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

/// Simulate many workers together?
struct WorkerSim {
    workers: HashMap<SocketAddr, worker::WorkerHandle>,
    bootstrap: Option<SocketAddr>,

    /// A queue of workers to add with associated delays, so we can simulate
    /// having new workers drop into the network instead of having them all
    /// appear instantly.
    ///
    /// The `run()` method sorts the vec by duration, in reverse, so we just
    /// pop the workers we need off the end.
    worker_add_queue: Vec<(SocketAddr, worker::WorkerHandle, Duration)>,

    rng: oorandom::Rand64,
}

impl WorkerSim {
    fn new() -> Self {
        Self {
            workers: HashMap::new(),
            bootstrap: None,
            worker_add_queue: Vec::new(),
            rng: oorandom::Rand64::new(12345),
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
    fn add_new_worker(&mut self, addr: &str, delay: Duration) {
        let w_id = types::PeerId::new_insecure_random(&mut self.rng);
        let w_addr = addr.parse().unwrap();
        let w = worker::WorkerState::start(w_id, w_addr);
        if let Some(bootstrap) = self.bootstrap {
            w.controller()
                .message(bootstrap, types::Message::Ping { id: w_id })
                .unwrap();
            w.controller()
                .message(bootstrap, types::Message::FindPeer { id: w_id })
                .unwrap();
        }
        self.worker_add_queue.push((w_addr, w, delay));
    }

    fn run(&mut self) {
        // Source, destination, message
        let mut messages: Vec<(SocketAddr, SocketAddr, types::Message)> = vec![];
        let start = Instant::now();
        self.worker_add_queue
            .sort_by(|(_, _, d1), (_, _, d2)| d2.cmp(d1));
        loop {
            // Add new workers, if any need adding.
            if let Some((addr, worker, dur)) = self.worker_add_queue.pop() {
                if dur < start.elapsed() {
                    info!("Adding worker: {:?} {}", worker.peer_id(), addr);
                    self.add_worker(addr, worker);
                } else {
                    // debug!(
                    //     "Worker should be added at {:?} but it's only {:?}",
                    //     dur,
                    //     start.elapsed()
                    // );
                    // Icky but it makes the borrow checker happy
                    self.worker_add_queue.push((addr, worker, dur));
                }
            }
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
            // Don't hog CPU... too much.
            std::thread::sleep(std::time::Duration::from_millis(10));
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
    sim.add_new_worker("10.0.0.1:4433", Duration::from_secs(0));
    sim.add_new_worker("10.0.0.2:4433", Duration::from_secs(2));
    sim.add_new_worker("10.0.0.3:4433", Duration::from_secs(10));
    // sim.add_new_worker("10.0.0.3:4433");
    // sim.add_new_worker("10.0.0.4:4433");
    sim.run();
    sim.quit();
}

fn main() {
    pretty_env_logger::init();
    // let opt = PeerOpt::from_args();
    heckin_simulator();
    // let peer = peer2::Peer::new(opt).expect("Could not create peer struct?");
    // peer.run().expect("Peer did not exit successfully?");
}
