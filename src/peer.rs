use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::result;
use std::str;
use std::time::{Duration, Instant};

use failure::{self, err_msg, Error, ResultExt};
use futures::{Future, Stream};
use quinn;
use rmp_serde;
use tokio;
use tokio::runtime::current_thread;
use tokio::runtime::current_thread::Runtime;

use rustls::internal::pemfile;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Message {
    Ping { id: u32 },
    Pong { id: u32 },
}

use PeerOpt;
type Result<T> = result::Result<T, Error>;

fn duration_secs(x: &Duration) -> f32 {
    x.as_secs() as f32 + x.subsec_nanos() as f32 * 1e-9
}

fn run_ping(stream: quinn::Stream) -> impl Future<Item = (), Error = ()> {
    info!("Trying to send ping");
    let message = Message::Ping { id: 999 };
    let serialized_message = rmp_serde::to_vec(&message).expect("Could not serialize message?!");
    tokio::io::write_all(stream, serialized_message)
        .map_err(|e| warn!("Failed to send request: {}", e))
        .and_then(move |(mut stream, _v)| {
            stream.flush().expect("Could not flush stream?");
            info!("Sent, reading?");
            quinn::read_to_end(stream, 1024 * 64)
                .map_err(|e| warn!("failed to read response: {}", e))
        }).and_then(move |(stream, req)| {
            let msg: ::std::result::Result<Message, rmp_serde::decode::Error> =
                rmp_serde::from_slice(&req);
            debug!("Got response: {:?}", msg);
            let to_send = match msg {
                Ok(Message::Ping { id }) => {
                    info!("Trying to send pong");
                    let message = Message::Pong { id };
                    rmp_serde::to_vec(&message)
                        .expect("Could not serialize message; should never happen!")
                }
                Ok(val) => {
                    info!("Got message: {:?}, not doing anything with it", val);
                    vec![]
                }
                Err(e) => {
                    info!("Got unknown message: {:?}, error {:?}", &req, e);
                    vec![]
                }
            };

            tokio::io::write_all(stream, to_send)
                .map_err(|e| warn!("Failed to send request: {}", e))
        }).and_then(|(stream, _)| {
            tokio::io::shutdown(stream).map_err(|e| warn!("Failed to shut down stream: {}", e))
        }).map(move |_| info!("request complete"))
}

/// All peer state stuff.
pub struct Peer {
    options: PeerOpt,
    runtime: Runtime,
}

impl Peer {
    pub fn new(options: PeerOpt) -> Self {
        let runtime = Runtime::new().expect("Could not create runtime");
        Peer { options, runtime }
    }

    /// Actually starts the node, blocking the current thread.
    pub fn run(&mut self) -> Result<()> {
        // For now we always listen...
        info!("Bootstrap peer: {:?}", self.options.bootstrap_peer);
        self.start_client()?;

        // Start each the client and server futures.
        info!("Starting server on {}", self.options.listen);
        self.start_server()?;

        // Block on futures and run them to completion.
        self.runtime.run().map_err(Error::from)
    }

    pub fn handle_new_client_connection(
        conn: quinn::NewClientConnection,
        start: Instant,
    ) -> impl Future<Item = (), Error = failure::Error> {
        let message = Message::Ping { id: 999 };
        let serialized_message =
            rmp_serde::to_vec(&message).expect("Could not serialize message?!");

        info!("connected at {}", duration_secs(&start.elapsed()));
        let conn = conn.connection;
        let stream = conn.open_bi();
        stream
            .map_err(|e| format_err!("failed to open stream: {}", e))
            .and_then(move |stream| {
                info!("stream opened at {}", duration_secs(&start.elapsed()));
                tokio::io::write_all(stream, serialized_message)
                    .map_err(|e| format_err!("failed to send request: {}", e))
            }).and_then(|(stream, _)| {
                tokio::io::shutdown(stream)
                    .map_err(|e| format_err!("failed to shutdown stream: {}", e))
            }).and_then(move |stream| {
                let response_start = Instant::now();
                info!(
                    "request sent at {}",
                    duration_secs(&(response_start - start))
                );
                quinn::read_to_end(stream, usize::max_value())
                    .map_err(|e| format_err!("failed to read response: {}", e))
                    .map(move |x| (x, response_start))
            }).and_then(move |((_, data), response_start)| {
                let seconds = duration_secs(&response_start.elapsed());
                info!(
                    "response received in {}ms - {} KiB/s",
                    response_start.elapsed().subsec_millis(),
                    data.len() as f32 / (seconds * 1024.0)
                );
                let msg: ::std::result::Result<Message, rmp_serde::decode::Error> =
                    rmp_serde::from_slice(&data);
                debug!("Got response: {:?}", msg);
                conn.close(0, b"done").map_err(|_| unreachable!())
            }).map(|()| info!("connection closed"))
    }

    pub fn start_client(&mut self) -> Result<()> {
        let config = quinn::Config {
            ..quinn::Config::default()
        };
        if let Some(ref bootstrap_peer) = self.options.bootstrap_peer {
            let mut builder = quinn::Endpoint::new();
            builder.config(config);

            if let Some(ref ca_path) = self.options.ca {
                builder.set_certificate_authority(&fs::read(&ca_path)?)?;
            }
            let (endpoint, driver, mut _incoming) = builder.bind("[::]:0")?;
            // self.runtime
            //     .spawn(driver.map_err(|e| eprintln!("IO error: {}", e)));

            let start = Instant::now();

            let addr = bootstrap_peer
                .with_default_port(|_| Ok(4433))?
                .to_socket_addrs()?
                .next()
                .ok_or(format_err!("couldn't resolve bootstrap peer to an address"))?;
            let future1: Box<dyn Future<Item = (), Error = ()>> = Box::new(
                endpoint
                    .connect(&addr, "localhost")?
                    .map_err(|e| error!("failed to connect: {}", e))
                    .and_then(move |conn: quinn::NewClientConnection| {
                        debug!("Connection established");
                        Self::handle_new_client_connection(conn, start).map_err(|e| {
                            error!("Error handling client connection: {}", e);
                        })
                    }),
            );

            let future2: Box<dyn Future<Item = (), Error = ()>> =
                Box::new(driver.map_err(|e| eprintln!("IO error: {}", e)));

            use futures::*;

            // This too half a fucking hour to figure out,
            // after I'd already been told the answer.
            // select() returns a future that yields a tuple
            // for both the Item type AND THE Error TYPE.
            // NOWHERE did it tell me that the type mismatch
            // was in the Error type!
            let v: Box<dyn Future<Item = (), Error = ()>> = Box::new(
                future1
                    .select(future2)
                    .map(|((), _select_next)| ())
                    .map_err(|_| ()),
            );

            self.runtime.spawn(v);
            // self.runtime.spawn(future);
        }

        Ok(())
    }

    pub fn start_server(&mut self) -> Result<()> {
        let mut builder = quinn::Endpoint::new();
        builder
            .config(quinn::Config {
                max_remote_bi_streams: 64,
                ..quinn::Config::default()
            }).listen();

        // Mongle TLS keys
        let keys = {
            let mut reader = io::BufReader::new(
                fs::File::open(&self.options.key).context("failed to read private key")?,
            );
            pemfile::rsa_private_keys(&mut reader)
                .map_err(|_| err_msg("failed to read private key"))?
        };
        let cert_chain = {
            let mut reader = io::BufReader::new(
                fs::File::open(&self.options.cert).context("failed to read private key")?,
            );
            pemfile::certs(&mut reader).map_err(|_| err_msg("failed to read certificates"))?
        };
        builder.set_certificate(cert_chain, keys[0].clone())?;

        let (_endpoint, driver, incoming) = builder.bind(self.options.listen)?;

        info!(
            "Bound to {}, listening for incoming connections.",
            self.options.listen
        );

        self.runtime.spawn(incoming.for_each(move |conn| {
            let quinn::NewConnection {
                incoming,
                connection,
            } = conn;
            info!(
                "got connection: {}, {}, {:?}",
                connection.remote_id(),
                connection.remote_address(),
                connection.protocol()
            );
            let connection_handle_future = incoming
                .map_err(move |e| info!("connection terminated: {}", e))
                .for_each(move |stream| {
                    info!("Processing stream");
                    let stream = match stream {
                        quinn::NewStream::Bi(stream) => stream,
                        quinn::NewStream::Uni(_) => {
                            error!("client opened unidirectional stream");
                            return Ok(());
                        }
                    };
                    current_thread::spawn(run_ping(stream));
                    Ok(())
                });

            current_thread::spawn(connection_handle_future);
            Ok(())
        }));

        // TODO: Is this block_on() what we actually want?
        self.runtime.block_on(driver)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::net::ToSocketAddrs;
    use std::thread;
    use std::time::Duration;

    use failure::{self, Error};
    use lazy_static;
    use url;

    use peer;
    use PeerOpt;

    // This isn't necessarily the best way to do this, but...
    lazy_static! {
        static ref SERVER_THREAD: thread::JoinHandle<Result<(), failure::Error>> = {
            thread::spawn(move || {
                let mut peer = peer::Peer::new(PeerOpt {
                    bootstrap_peer: None,
                    listen: "[::]:4433".to_socket_addrs().unwrap().next().unwrap(),
                    ca: None,
                    key: "certs/server.rsa".into(),
                    cert: "certs/server.chain".into(),
                });

                peer.run().expect("Could not run peer?");
                Ok(())
            })
        };
    }
    #[test]
    fn test_client_connection() {
        // ::setup_logging();

        lazy_static::initialize(&SERVER_THREAD);

        // TODO: Make sure it actually fails when no server is running!
        // Currently it doesn't, it just times out... eventually.
        let mut peer = peer::Peer::new(PeerOpt {
            bootstrap_peer: Some(url::Url::parse("quic://[::1]:4433/").unwrap()),
            listen: "[::]:4434".to_socket_addrs().unwrap().next().unwrap(),
            ca: Some("certs/ca.der".into()),
            key: "certs/server.rsa".into(),
            cert: "certs/server.chain".into(),
        });

        let res = peer.start_client();

        // Block on futures and run them to completion.
        peer.runtime.run().map_err(Error::from).unwrap();
        assert!(res.is_ok());
    }
}
