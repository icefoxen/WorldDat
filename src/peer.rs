use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::result;
use std::str;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use failure::{err_msg, Error, ResultExt};
use futures::{self, future, Future};
use quinn;
use rmp_serde;
use tokio;

// TODO: We have to use current_thread here 'cause we pass
// around a lot of `quinn` connection info into futures,
// and those aren't Send due to having Rc's and such in them.
// That's PROBABLY fine, but make sure!
use tokio::runtime::current_thread::Runtime;
use tokio_io;

use rustls::internal::pemfile;

use hash::*;

/// The actual serializable messages that can be sent back and forth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Message {
    Ping {
        id: PeerId,
    },
    Pong {
        id: PeerId,
    },
    FindPeer {
        id: PeerId,
    },
    FindPeerResponsePeerFound {
        contact: ContactInfo,
    },
    FindPeerResponsePeerNotFound {
        id: PeerId,
        /// It's ok to have this be unbounded size 'cause we only receive a fixed-size
        /// buffer from any client...  I'm pretty sure.
        /// TODO: Verify!
        neighbors: Vec<ContactInfo>,
    },
}

pub trait SendStream: quinn::Write + io::Write + tokio_io::AsyncWrite {}
pub trait Stream:
    quinn::Write + io::Write + tokio_io::AsyncWrite + quinn::Read + io::Read + tokio_io::AsyncRead
{
}
pub trait ConnectionThingy {
    type SendStream: SendStream + 'static;
    type Stream: Stream + 'static;
    type Error: fmt::Debug + 'static;
    fn open_uni(&self) -> Box<dyn Future<Item = Self::SendStream, Error = Self::Error>>;
    fn open_bi(&self) -> Box<dyn Future<Item = Self::Stream, Error = Self::Error>>;
}
impl SendStream for quinn::SendStream {}
impl Stream for quinn::Stream {}

impl ConnectionThingy for quinn::Connection {
    type SendStream = quinn::SendStream;
    type Stream = quinn::Stream;
    type Error = quinn::ConnectionError;
    fn open_uni(&self) -> Box<dyn Future<Item = Self::SendStream, Error = Self::Error>> {
        Box::new(self.open_uni())
    }
    fn open_bi(&self) -> Box<dyn Future<Item = quinn::Stream, Error = Self::Error>> {
        Box::new(self.open_bi())
    }
}

use PeerOpt;
pub type Result<T> = result::Result<T, Error>;

fn duration_secs(x: &Duration) -> f32 {
    x.as_secs() as f32 + x.subsec_nanos() as f32 * 1e-9
}

/// A hash identifying a Peer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PeerId(Blake2Hash);

/// Contact info for a peer, mapping the `PeerId` to an IP address and port.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The min and max address range of the bucket; it stores peers with ID's
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
///
/// TODO: Can the `im` crate serve any purpose here?  I'm really not sure
/// if it can, but it's so *neat*...
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
    ///
    /// We DO prevent duplicates though; if a peer is given that has a peer_id
    /// that already exists in the map, it will replace the old one.
    #[allow(unused_variables)]
    pub fn insert(&mut self, new_peer: ContactInfo) {
        if let Some(i) = self.buckets[0]
            .known_peers
            .iter()
            .position(|ci| ci.peer_id == new_peer.peer_id)
        {
            self.buckets[0].known_peers[i].peer_id = new_peer.peer_id;
        } else {
            self.buckets[0].known_peers.push(new_peer);
            self.buckets[0].known_peers.sort();
        }
    }

    pub fn lookup(&self, _peer_id: PeerId) -> result::Result<ContactInfo, Vec<ContactInfo>> {
        Err(vec![])
    }
}

/// All parts of the peer state that get stuffed into an Arc
/// and potentially shared between threads.
pub struct PeerSharedState {
    pub id: PeerId,
    pub peermap: PeerMap,
}

/// All peer state stuff.
pub struct Peer {
    pub options: PeerOpt,
    pub runtime: Runtime,
    pub shared: Arc<RwLock<PeerSharedState>>,
}

impl Peer {
    pub fn new(options: PeerOpt) -> Self {
        let id_seed: [u8; 64] = [0; 64];
        let id = PeerId(Blake2Hash::new(&id_seed));
        let shared_state = PeerSharedState {
            id,
            peermap: PeerMap::new(),
        };
        let shared = Arc::new(RwLock::new(shared_state));
        let runtime = Runtime::new().expect("Could not create runtime");
        Peer {
            options,
            runtime,
            shared,
        }
    }

    /// Actually starts the node, blocking the current thread.
    pub fn run(&mut self) -> Result<()> {
        // For now we always listen...
        info!("Bootstrap peer: {:?}", self.options.bootstrap_peer);

        // Start each the client and server futures.
        info!("Starting server on {}", self.options.listen);
        self.start()?;

        // Block on futures and run them to completion.
        self.runtime.run().map_err(Error::from)
    }

    fn handle_message_from(
        &mut self,
        addr: SocketAddr,
        msg: &Message,
        state: Arc<RwLock<PeerSharedState>>,
    ) {
        match msg {
            Message::Ping { id } => {
                let my_id = state.read().unwrap().id;
                self.send_to_peer(*id, Message::Pong { id: my_id });
            }
            Message::Pong { id } => {
                let info = ContactInfo {
                    peer_id: *id,
                    address: addr,
                };
                let mut state = state.write().unwrap();
                state.peermap.insert(info);
            }
            Message::FindPeer { id } => {
                // if we know about that peer, send it back.
                // Else, send findPeerResponsePeerNotFound
                let map = &state.read().unwrap().peermap;
                let resp = match map.lookup(*id) {
                    Ok(contact) => Message::FindPeerResponsePeerFound { contact },
                    Err(neighbors) => Message::FindPeerResponsePeerNotFound { id: *id, neighbors },
                };
                self.send_to_peer(*id, resp);
            }
            Message::FindPeerResponsePeerFound { contact } => {
                // ping contact to see if it's legit
                let my_id = state.read().unwrap().id;
                self.send_to_peer(contact.peer_id, Message::Ping { id: my_id });
            }
            Message::FindPeerResponsePeerNotFound { id, neighbors } => {
                for neighbor in neighbors {
                    self.send_to_peer(neighbor.peer_id, Message::FindPeer { id: *id });
                }
            }
        }
    }

    fn send_to_peer(&mut self, _peer_id: PeerId, _msg: Message) {}

    /// Sends the given message on the given stream, handling errors and such.
    /// Or rather, returns a new future which does that.
    ///
    /// The stream is closed after the message is sent, since streams are lightweight.
    /// All messages should be stateless, I believe, so that we never have to remember
    /// what the heck is going on.  (Unfortunately I think this that might have problems
    /// for security and such, but, we'll give it a try.)
    ///
    /// This can't take `&self` since it would have to be moved into the future.
    /// If necessary it can take `Arc<RwLock<PeerSharedState>>`.
    fn send_message<S>(stream: S, message: Message) -> impl Future<Item = (), Error = ()>
    where
        S: Stream + 'static,
    {
        let serialized_message =
            rmp_serde::to_vec(&message).expect("Could not serialize message?!");
        let de: Message =
            rmp_serde::from_slice(&serialized_message).expect("Could not deserialize message?!");
        assert_eq!(message, de);
        tokio::io::write_all(stream, serialized_message)
            .and_then(|(stream, _vec)| tokio::io::shutdown(stream))
            .map_err(|e| warn!("Failed to send request: {}", e))
            .map(move |_| debug!("Message send complete: {:X?}", message))
    }

    /// Reads a message from the given stream and updates the `Peer`'s internal
    /// state accordingly, and returns a future of what to do next (if anything).
    ///
    /// This can't take `&self` since it would have to be moved into the future.
    /// If necessary it can take `Arc<RwLock<PeerSharedState>>`.
    /// fn receive_message(stream: quinn::Stream) -> impl Future<Item = (), Error = ()> {
    fn receive_message<S>(stream: S) -> impl Future<Item = (), Error = ()>
    where
        S: Stream + 'static,
    {
        quinn::read_to_end(stream, 1024 * 64)
            .map_err(|e| warn!("failed to read response: {}", e))
            .and_then(move |(stream, req)| {
                let msg: ::std::result::Result<Message, rmp_serde::decode::Error> =
                    rmp_serde::from_slice(&req);
                debug!("Got message: {:?}", msg);
                let to_do_next: Box<dyn Future<Item = S, Error = ()>> = match msg {
                    Ok(Message::Ping { id }) => {
                        info!("Got ping, trying to send pong");
                        let message = Message::Pong { id };
                        let to_send = rmp_serde::to_vec(&message)
                            .expect("Could not serialize message; should never happen!");
                        Box::new(
                            tokio::io::write_all(stream, to_send)
                                .map_err(|e| warn!("Failed to send request: {}", e))
                                .map(|(stream, _vec)| stream),
                        )
                    }
                    Ok(_val) => {
                        info!("Unknown message, not doing anything with it");
                        Box::new(future::ok(stream))
                    }
                    Err(e) => {
                        info!("Error getting message, error {:?}", e);
                        Box::new(future::ok(stream))
                    }
                };
                to_do_next
                    .and_then(|stream| {
                        trace!("Trying to shut down stream");
                        tokio::io::shutdown(stream)
                            .inspect(|_v| {
                                trace!("Done!");
                            }).map_err(|e| warn!("Failed to shut down stream: {}", e))
                    }).map(move |_| info!("request complete"))
            })
    }

    /// Returns a future which does all the talking necessary to communicate
    /// with another peer, regardless of who started it.
    ///
    /// This should basically implement the state machine of the core protocol.
    fn talk_to_peer<Conn, FS>(
        connection: Conn,
        incoming: FS,
        state: Arc<RwLock<PeerSharedState>>,
    ) -> impl Future<Item = (), Error = ()>
    where
        Conn: ConnectionThingy,
        // ST: Stream + 'static,
        FS: futures::stream::Stream<Item = quinn::NewStream, Error = quinn::ConnectionError>
            + 'static,
    {
        use futures::Stream;
        let outgoing_stream: Box<dyn Future<Item = (), Error = ()>> = Box::new(
            connection
                .open_bi()
                .map_err(|e| format_err!("failed to open stream: {:?}", e))
                .then(move |stream| {
                    info!("Sending message to peer");
                    let s = stream.expect("Could not unwrap stream?");
                    let msg = Message::Ping {
                        id: state
                            .read()
                            .expect("RwLock poisoned; should never happen!")
                            .id,
                    };
                    Self::send_message(s, msg)
                }),
        );

        // For each incoming stream, try to receive a message on it.
        // TODO: Should this spawn a new task for each stream?  mmmmmmmmaybe.
        let incoming_streams: Box<dyn Future<Item = (), Error = ()>> = Box::new(
            incoming
                .map_err(|e| warn!("Incoming stream failed: {:?}", e))
                .for_each(move |stream| {
                    info!("Peer created incoming stream");
                    // Don't bother with uni-directional streams yet.
                    match stream {
                        quinn::NewStream::Bi(bi_stream) => Self::receive_message(bi_stream)
                            .map_err(|e| warn!("Incoming stream failed: {:?}", e)),
                        quinn::NewStream::Uni(_) => unimplemented!(),
                    }
                    // Self::receive_message(stream)
                }),
        );

        let merged_stream_handlers = outgoing_stream.join(incoming_streams).map(|((), ())| ());

        merged_stream_handlers
    }

    /// Start listening on a port for other peers to come
    /// talk to us,
    /// and if we know about a bootstrap peer then we also attempt to talk to it.
    pub fn start(&mut self) -> Result<()> {
        // SETUP BIG PILES OF CONFIG STUFF

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

        // TODO: prefer `listen_with_keys`?
        let mut builder = quinn::EndpointBuilder::from_config(quinn::Config {
            max_remote_bi_streams: 64,
            ..quinn::Config::default()
        });
        builder
            .set_certificate(cert_chain, keys[0].clone())?
            // TODO: Use listen_with_keys() instead?
            .listen();

        if self.options.logproto {
            use slog;
            use slog::Drain;
            // We use `log`, quinn uses `slog`.  This logger setup uses `slog_stdlog`
            // to feed all `slog` logging messages into `log`.
            // TODO: Figure out how to fix the module paths!  All of quinn's logging
            // messages show up as having the module "".

            let drain = slog_stdlog::StdLog
                // TODO: Apparently `quinn` only ever uses compile-time feature settings for
                // setting logging levels, we want something that doesn't require recompiling
                // the world.
                // So this sets the max at runtime, while we also use the feature setting for
                // `slog` to tell it what to make possible.
                .filter_level(slog::Level::Trace)
                // fuse() makes the logger panic if we receive an error in logging, such as
                // an I/O error.
                .fuse();

            let root = slog::Logger::root(drain, slog_o!());
            trace!("Protocol logging started.");
            builder.logger(root);
        }

        let mut client_config_builder = quinn::ClientConfigBuilder::new();
        // We basically by definition don't know the peer's cert, so
        // this is ok.
        client_config_builder.accept_insecure_certs();
        let client_config = client_config_builder.build();

        // ACTUALLY CREATE THE CONNECTION

        let (endpoint, driver, incoming) = builder.bind(self.options.listen)?;

        // SETUP OUTGOING CONNECTION, IF ANY
        if let Some(ref bootstrap_peer) = self.options.bootstrap_peer {
            let addr = bootstrap_peer
                .with_default_port(|_| Ok(4433))?
                .to_socket_addrs()?
                .next()
                .ok_or(format_err!(
                    "Couldn't resolve bootstrap peer input to an address"
                ))?;
            // Copy self.shared to move into closure
            let shared = self.shared.clone();
            let bootstrap_connection_future: Box<dyn Future<Item = (), Error = ()>> = Box::new(
                endpoint
                    .connect_with(&client_config, &addr, "localhost")?
                    .map_err(|e| error!("failed to connect: {}", e))
                    .and_then(move |conn: quinn::NewClientConnection| {
                        info!("Connection to bootstrap peer established");
                        let quinn::NewClientConnection {
                            connection,
                            incoming,
                            session_tickets: _session_tickets,
                        } = conn;
                        Self::talk_to_peer(connection, incoming, shared)
                    }),
            );
            self.runtime.spawn(bootstrap_connection_future);
        };

        // SETUP LISTENER FOR INCOMING CONNECTIONS
        info!(
            "Bound to {}, listening for incoming connections.",
            self.options.listen
        );

        let shared = self.shared.clone();
        use futures::Stream as FutureStream;
        self.runtime.spawn(incoming.for_each(move |conn| {
            let quinn::NewConnection {
                incoming,
                connection,
            } = conn;
            let shared = shared.clone();
            info!(
                "got connection: {}, {}, {:?}",
                connection.remote_id(),
                connection.remote_address(),
                connection.protocol()
            );
            Self::talk_to_peer(connection, incoming, shared)
        }));

        // TODO: Is this block_on() what we actually want?
        // It is now, yes.  That will run ALL futures, and return
        // when the `driver` future finishes -- ie, when the listening
        // socket closes.
        self.runtime.block_on(driver)?;
        Ok(())
    }
}
