extern crate quinn;
extern crate tokio;
extern crate tokio_io;
#[macro_use]
extern crate failure;
extern crate futures;
#[macro_use]
extern crate structopt;
extern crate url;

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

use structopt::StructOpt;

use std::net::SocketAddr;
use std::path::PathBuf;

mod peer;

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
                colors.color(record.level()).to_string(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Trace)
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
    /// Initial node to connect to
    #[structopt(short = "b", long = "bootstrap")]
    bootstrap_peer: Option<url::Url>,

    /// Address to listen on for incoming connections.
    /// Currently we always listen but it would be nice to have
    /// a fetch-only peer sometime.
    #[structopt(short = "l", long = "listen", default_value = "[::]:4433")]
    listen: SocketAddr,
    #[structopt(parse(from_os_str), long = "ca")]
    ca: Option<PathBuf>,

    // TODO: External files here is not necessarily nicest for unit testing.
    // ALSO TODO: Can we make them optional?  Not yet.
    // weigh in on https://github.com/djc/quinn/issues/35 when sober.
    /// TLS private key in PEM format
    #[structopt(parse(from_os_str), short = "k", long = "key")]
    key: PathBuf,
    /// TLS certificate in PEM format
    #[structopt(parse(from_os_str), short = "c", long = "cert")]
    cert: PathBuf,
}

fn main() {
    setup_logging();
    let opt = PeerOpt::from_args();
    let mut peer = peer::Peer::new(opt);
    peer.run().expect("Peer did not exit successfully?");
}
