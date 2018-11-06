extern crate quinn;
extern crate tokio;
extern crate tokio_io;
#[macro_use]
extern crate failure;
extern crate futures;
extern crate structopt;
extern crate url;

#[macro_use(slog_o, slog_trace)]
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

use structopt::StructOpt;

use std::net::SocketAddr;
use std::path::PathBuf;

// mod connection_tests;
// mod hash;
// mod peer;
mod peer2;

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
    // See https://github.com/djc/quinn/issues/35
    /// TLS private key in PEM format
    #[structopt(parse(from_os_str), short = "k", long = "key")]
    key: PathBuf,
    /// TLS certificate in PEM format
    #[structopt(parse(from_os_str), short = "c", long = "cert")]
    cert: PathBuf,

    /// Whether or not to output low-level QUIC protocol logging,
    /// useful for debugging
    #[structopt(long = "logproto")]
    logproto: bool,
}

fn main() {
    setup_logging();
    let opt = PeerOpt::from_args();
    let mut peer = peer2::Peer::new(opt);
    peer.run().expect("Peer did not exit successfully?");
}
