use std::io::Write;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::result;
use std::str;
use std::time::{Duration, Instant};

use failure::{self, Error};
use futures::{Future, Stream};
use quinn;
use rmp_serde;
use tokio;
use tokio::runtime::current_thread;
use tokio::runtime::current_thread::Runtime;

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
        println!("Bootstrap peer: {:?}", self.options.bootstrap_peer);
        if let Some(ref bootstrap_peer) = self.options.bootstrap_peer {
            let addr = bootstrap_peer
                .with_default_port(|_| Ok(4433))?
                .to_socket_addrs()?
                .next()
                .ok_or(format_err!("couldn't resolve bootstrap peer to an address"))?;
            info!("Attempting to talk to {}", addr);
            Self::start_client(&mut self.runtime, addr)?;
        }

        // Start each the client and server futures.
        info!("Starting server on {}", self.options.listen);
        Self::start_server(&mut self.runtime, self.options.listen)?;

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
            }).map(|()| info!("drained"))
    }

    pub fn start_client(runtime: &mut Runtime, bootstrap_peer: SocketAddr) -> Result<()> {
        let config = quinn::Config {
            ..quinn::Config::default()
        };

        let mut builder = quinn::Endpoint::new();
        builder.config(config);
        let (endpoint, driver, _incoming) = builder.bind("[::]:0")?;
        runtime.spawn(driver.map_err(|e| eprintln!("IO error: {}", e)));

        let start = Instant::now();

        let future = endpoint
            .connect(&bootstrap_peer, "localhost")?
            .map_err(|e| error!("failed to connect: {}", e))
            .and_then(move |conn: quinn::NewClientConnection| {
                let x = Self::handle_new_client_connection(conn, start).map_err(|e| {
                    error!("Error handling client connection: {}", e);
                });
                x
            });

        runtime.spawn(future);
        Ok(())
    }

    pub fn start_server(runtime: &mut Runtime, listen: SocketAddr) -> Result<()> {
        let mut builder = quinn::Endpoint::new();
        builder
            .config(quinn::Config {
                max_remote_bi_streams: 64,
                ..quinn::Config::default()
            }).listen();

        let (_endpoint, driver, incoming) = builder.bind(listen)?;

        info!("Bound to {}, listening for incoming connections.", listen);

        runtime.spawn(incoming.for_each(move |conn| {
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
        runtime.block_on(driver)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::net::ToSocketAddrs;
    use std::thread;

    use failure;
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
                });

                peer.run().expect("Could not run peer?");
                Ok(())
            })
        };
    }
    #[test]
    fn test_client_connection() {
        lazy_static::initialize(&SERVER_THREAD);

        // TODO: Make sure it actually fails when no server is running!
        let mut peer = peer::Peer::new(PeerOpt {
            bootstrap_peer: Some(url::Url::parse("quic://[::0]:4433/").unwrap()),
            listen: "[::]:4434".to_socket_addrs().unwrap().next().unwrap(),
        });

        // grumble grumble neither Url nor SocketAddr is quite Right.
        let peer_socketaddr = peer
            .options
            .bootstrap_peer
            .unwrap()
            .to_socket_addrs()
            .unwrap()
            .next()
            .unwrap();

        let res = peer::Peer::start_client(&mut peer.runtime, peer_socketaddr);
        assert!(res.is_ok())
    }
}
