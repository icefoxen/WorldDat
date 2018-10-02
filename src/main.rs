extern crate bytes;
extern crate quicr;
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
extern crate serde;
#[macro_use]
extern crate serde_derive;
// lazy_static used in unit tests
#[allow(unused_imports)]
#[macro_use]
extern crate lazy_static;

use std::path::PathBuf;
use structopt::StructOpt;

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
    // Hooks up console output.
        .chain(std::io::stdout())
        .apply()
        .expect("Could not init logging!");
}

#[derive(StructOpt, Debug, Clone)]
pub struct PeerOpt {
    /// Initial node to connect to
    #[structopt(short = "b", long = "bootstrap")]
    bootstrap_peer: Option<String>,

    /// Port to listen on for incoming connections.
    #[structopt(default_value = "4433", short = "p", long = "port")]
    listen_port: u16,
    /*
    /// TLS private key in PEM format
    #[structopt(parse(from_os_str), short = "k", long = "key", requires = "cert")]
    key: Option<PathBuf>,
    /// TLS certificate in PEM format
    #[structopt(parse(from_os_str), short = "c", long = "cert", requires = "key")]
    cert: Option<PathBuf>,
*/
}

#[derive(StructOpt, Debug)]
pub struct ServerOpt {
    // /// directory to serve files from
    //#[structopt(parse(from_os_str))]
    //root: PathBuf,
    /// TLS private key in PEM format
    #[structopt(
        parse(from_os_str),
        short = "k",
        long = "key",
        requires = "cert"
    )]
    key: Option<PathBuf>,
    /// TLS certificate in PEM format
    #[structopt(
        parse(from_os_str),
        short = "c",
        long = "cert",
        requires = "key"
    )]
    cert: Option<PathBuf>,
}

#[derive(StructOpt, Debug)]
enum Opt {
    #[structopt(name = "server")]
    Server(ServerOpt),
    #[structopt(name = "client")]
    Client,
    #[structopt(name = "peer")]
    Peer(PeerOpt),
}

fn main() {
    setup_logging();
    let opt = Opt::from_args();
    let code = {
        match opt {
            Opt::Server(s) => {
                if let Err(e) = peer::run_server(s) {
                    eprintln!("ERROR: {:?}", e);
                    1
                } else {
                    0
                }
            }
            Opt::Client => {
                if let Err(e) = peer::run_client() {
                    eprintln!("ERROR: {:?}", e);
                    1
                } else {
                    0
                }
            }
            Opt::Peer(_p) => {
                0
                /*
                if let Err(e) = peer::run_peer(p) {
                    eprintln!("ERROR: {}", e.cause());
                    1
                } else { 0 }
                 */
            }
        }
    };
    ::std::process::exit(code);
}
