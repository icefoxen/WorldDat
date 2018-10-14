use std::cmp::Ordering;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::ops;
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

/// The number of bytes in a `Blake2Hash`.
pub const BLAKE2_HASH_SIZE: usize = 64;

/// A hash uniquely identifying a peer or data type, barring hash collisions.
/// I'm not yet sure whether we want SHA256, SHA512 or something else
/// (Blake2?  SHA-3?) so for now we leave room for future expansion.
/// Expansion is hard though, since it means the address space and
/// such changes size, so we don't really want to upgrade if we can avoid it.
///
/// I am advised that the best general-purpose choice is currently Blake2,
/// since SHA3 is slow in software.
///
/// There's blake2s, which is 256 bits, and blake2b, which is 512.  We use
/// blake2b, 'cause I see no reason not to.
#[derive(Copy, Clone)]
pub struct Blake2Hash(pub [u8; BLAKE2_HASH_SIZE]);

impl PartialEq for Blake2Hash {
    fn eq(&self, other: &Self) -> bool {
        self.0[..] == other.0[..]
    }
}

impl Eq for Blake2Hash {}

impl PartialOrd for Blake2Hash {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Blake2Hash {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0[..].cmp(&other.0[..])
    }
}

impl ops::BitXor for Blake2Hash {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self {
        // We could probably optimize this but heck it.
        let mut out = [0; 64];
        for i in 0..self.0.len() {
            out[i] = self.0[i] ^ rhs.0[i];
        }
        Blake2Hash(out)
    }
}

impl Blake2Hash {
    /// The maximum power of 2 that the hash can hold.
    /// In this case, 512 for Blake2b.
    pub fn max_power() -> usize {
        BLAKE2_HASH_SIZE * 8
    }
}

/// A hash identifying a Peer.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PeerId(Blake2Hash);

/// Contact info for a peer, mapping the `PeerId` to an IP address and port.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    peer_id: PeerId,
    address: SocketAddr,
}

impl PartialOrd for ContactInfo {
    /// Contact info is ordered by `peer_id`
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ContactInfo {
    /// Contact info is ordered by `peer_id`
    fn cmp(&self, other: &Self) -> Ordering {
        self.peer_id.cmp(&other.peer_id)
    }
}

/// A "bucket" in the DHT, a collection of `ContactInfo`'s with
/// a fixed max size.
struct Bucket {
    /// The peers in the bucket.
    known_peers: Vec<ContactInfo>,
    /// The min nad max address range of the bucket; it stores peers with ID's
    /// in the range of `[2^min,2^max)`.
    ///
    /// TODO: u32 is way overkill here, but, KISS for now.
    address_range: (u32, u32),
}

impl Bucket {
    fn new(bucket_size: usize, min_address: u32, max_address: u32) -> Self {
        assert!(min_address < max_address);
        Self {
            known_peers: Vec::with_capacity(bucket_size),
            address_range: (min_address, max_address),
        }
    }
}

/// The peer's view of the DHT, a mapping of PeerId to contact info.
/// As per Kademila and Bittorrent DHT, the further away from the peer's
/// hash (as measured by the XOR distance metric), the lower resolution
/// it is.
pub struct PeerMap {
    buckets: Vec<Bucket>,
    /// The maximum number of peers per bucket.
    /// Currently hardwired at 8 in `new()`, but there's no reason we wouldn't
    /// want the ability to fiddle with it at runtime.
    bucket_size: usize,
}

impl PeerMap {
    pub fn new() -> Self {
        let bucket_size = 8;
        let initial_bucket = Bucket::new(bucket_size, 0, Blake2Hash::max_power() as u32);
        Self {
            buckets: vec![initial_bucket],
            bucket_size,
        }
    }

    /// Insert a new peer into the PeerMap,
    ///
    /// TODO: Should return an error or something if doing so
    /// would need to evict a current peer; that should be based
    /// on peer quality measures we don't track yet.
    ///
    /// For now though, we don't even bother splitting buckets or such.
    pub fn insert(&mut self, new_peer: ContactInfo) {
        self.buckets[0].known_peers.push(new_peer);
        self.buckets[0].known_peers.sort();
    }
}

/// All peer state stuff.
pub struct Peer {
    options: PeerOpt,
    runtime: Runtime,
    peermap: PeerMap,
}

impl Peer {
    pub fn new(options: PeerOpt) -> Self {
        let peermap = PeerMap::new();
        let runtime = Runtime::new().expect("Could not create runtime");
        Peer {
            options,
            runtime,
            peermap,
        }
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

    /// Creates a future that does all the communication stuff needed
    /// to talk to a new connection.
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

    /// Starts the client, spawning it on this `Peer`'s
    /// runtime.  Use `peer.runtime.run()` to actually drive it.
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
