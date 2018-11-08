use std::net::SocketAddr;
use std::path::Path;
use std::sync::mpsc;

use failure::{err_msg, Error, ResultExt};
use futures::{Future, Stream};
use quinn;
use rustls;
use tokio;
use tokio::runtime::current_thread::{self, Runtime};

use crate::types::*;
use crate::PeerOpt;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Peer {
    options: PeerOpt,
    runtime: Runtime,
    incoming_messages: mpsc::Sender<(SocketAddr, Message)>,
    outgoing_messages: mpsc::Receiver<(SocketAddr, Message)>,
    connections: Rc<RefCell<HashMap<SocketAddr, quinn::Connection>>>,
}

/// Creates an escaped ASCII string from the given bytes.
fn escaped(bytes: &[u8]) -> String {
    use std::ascii;
    use std::str;
    let mut escaped = String::new();
    for &b in &bytes[..] {
        let part = ascii::escape_default(b).collect::<Vec<_>>();
        escaped.push_str(str::from_utf8(&part).unwrap());
    }
    escaped
}

impl Peer {
    pub fn new(options: PeerOpt) -> Result<Self, Error> {
        // We must create the `Runtime` even though we're just handling
        // `current_thread::spawn()` stuff, 'cause otherwise... I don't even
        // know.  Bah.
        let runtime = Runtime::new()?;
        let connections = Rc::new(RefCell::new(HashMap::new()));
        let (_, outgoing) = mpsc::channel();
        let (incoming, _) = mpsc::channel();
        Ok(Peer {
            options,
            runtime,
            incoming_messages: incoming,
            outgoing_messages: outgoing,
            connections,
        })
    }

    /// Loads RSA private keys and cert chains from the files given in the options,
    /// and returns them.
    fn load_tls_keys(
        keyfile: &Path,
        certfile: &Path,
    ) -> Result<(Vec<rustls::PrivateKey>, Vec<rustls::Certificate>), Error> {
        use rustls::internal::pemfile;
        use std::fs;
        use std::io;
        let keys = {
            let mut reader =
                io::BufReader::new(fs::File::open(keyfile).context("failed to read private key")?);
            pemfile::rsa_private_keys(&mut reader)
                .map_err(|_| err_msg("failed to read private key"))?
        };
        let cert_chain = {
            let mut reader =
                io::BufReader::new(fs::File::open(certfile).context("failed to read private key")?);
            pemfile::certs(&mut reader).map_err(|_| err_msg("failed to read certificates"))?
        };
        Ok((keys, cert_chain))
    }

    /// We use `log`, quinn uses `slog`.  This logger setup uses `slog_stdlog`
    /// to feed all `slog` logging messages into `log`, so we get all of
    /// `quinn`'s error messages along with our own logging.
    ///
    /// TODO: Figure out how to fix the module paths!  All of quinn's logging
    /// messages show up as having the module "".
    fn enable_protocol_logging(builder: &mut quinn::EndpointBuilder) {
        use slog;
        use slog::Drain;

        let drain = slog_stdlog::StdLog
            // TODO: Apparently `quinn` only ever uses compile-time feature settings for
            // setting logging levels, we want something that doesn't require recompiling
            // the world.
            // So this sets the max at runtime, while we also use the feature setting for
            // `slog` to tell it what to make possible.
            .filter_level(slog::Level::Trace)
            // fuse() makes the logger panic if we receive an error in logging, such as
            // an I/O error.  Otherwise we have to give it some other method of handling
            // an error.
            .fuse();

        let root = slog::Logger::root(drain, slog_o!());
        trace!("Protocol logging started.");
        builder.logger(root);
    }

    fn read_message(
        remote_address: SocketAddr,
        stream: quinn::NewStream,
        channel: mpsc::Sender<(SocketAddr, Message)>,
    ) {
        // TODO: Honestly probably should just do uni streams all the way
        let stream = match stream {
            quinn::NewStream::Bi(stream) => stream,
            quinn::NewStream::Uni(_) => unreachable!(), // config.max_remote_uni_streams is defaulted to 0
        };

        current_thread::spawn(
            quinn::read_to_end(stream, 64 * 1024) // Read the request, which must be at most 64KiB
                .map_err(|e| format_err!("failed reading request: {}", e))
                .and_then(move |(stream, req)| {
                    let msg = escaped(&req);
                    info!("got request: \"{}\"", msg);

                    // TODO: Handle real data
                    channel
                        .send((remote_address, Message::Ping {}))
                        .expect("FDSAFDSAFA");
                    // Create a response
                    let resp = b"Bar!!";
                    // Write the response
                    tokio::io::write_all(stream, resp)
                        .map_err(|e| format_err!("failed to send response: {}", e))
                })
                // Gracefully terminate the stream
                .and_then(|(stream, _req)| {
                    tokio::io::shutdown(stream)
                        .map_err(|e| format_err!("failed to shutdown stream: {}", e))
                }).map(move |_| info!("request complete"))
                .map_err(move |e| error!("request failed: {:?}", e)),
        )
    }

    fn handle_connection(
        incoming: quinn::IncomingStreams,
        connection: quinn::Connection,
        channel: mpsc::Sender<(SocketAddr, Message)>,
        connection_map: Rc<RefCell<HashMap<SocketAddr, quinn::Connection>>>,
    ) {
        let c2 = connection.clone();

        let remote_address = connection.remote_address();
        connection_map
            .borrow_mut()
            .insert(connection.remote_address(), connection);

        current_thread::spawn(
            incoming
                .map_err(move |e| info!("connection terminated: {:?}", e))
                .for_each(move |stream| {
                    Self::read_message(remote_address, stream, channel.clone());
                    Ok(())
                }),
        );

        // TODO NEXT: This needs to do something useful, and probably not close the
        // connection (stream?) when finished; we're a bit loosey goosey with all that
        // BS, when really all streams can just be uni-directional.
        // Apparently cloning a connection is okay though, which is... interesting.
        // I think the idea is going to have to be that for each new connection we
        // create a channel and pass the Sender end of it to the worker thread, and
        // the worker uses that channel to send a message to that particular connection.
        //
        // A bit of a PITA, but it should work?  We will get a send error if the
        // receiving side of the channel gets dropped, ie by the connection
        // closing.
        // We might have to use futures::sync::mpsc::unbounded() instead of
        // std::sync::mpsc... idk why the hell they just don't impl Stream for
        // std::sync::mpsc.
        let outgoing_stream: Box<dyn Future<Item = (), Error = ()>> = Box::new(
            c2.open_bi()
                .map_err(|e| format_err!("failed to open stream: {:?}", e))
                .then(move |stream| {
                    info!("Sending message to peer");
                    let s = stream.expect("Could not unwrap stream?");
                    let msg = b"Foo!";
                    tokio::io::write_all(s, msg)
                        .and_then(|(stream, _vec)| tokio::io::shutdown(stream))
                        .map_err(|e| warn!("Failed to send request: {}", e))
                        .map(move |_| debug!("Message send complete: {:X?}", msg))
                }).inspect(|_| info!("Disconnecting from bootstrap")),
        );
        current_thread::spawn(outgoing_stream);
    }

    fn handle_incoming_connection(
        conn: quinn::NewConnection,
        channel: mpsc::Sender<(SocketAddr, Message)>,
        connection_map: Rc<RefCell<HashMap<SocketAddr, quinn::Connection>>>,
    ) {
        let quinn::NewConnection {
            incoming,
            connection,
        } = conn;
        info!("Got incoming connection: {:?}", connection.remote_address());

        Self::handle_connection(incoming, connection, channel, connection_map);
    }

    fn handle_outgoing_connection(
        conn: quinn::NewClientConnection,
        channel: mpsc::Sender<(SocketAddr, Message)>,
        connection_map: Rc<RefCell<HashMap<SocketAddr, quinn::Connection>>>,
    ) {
        info!(
            "Outgoing connection established to peer: {:?}",
            conn.connection.remote_address()
        );

        let quinn::NewClientConnection {
            connection,
            incoming,
            session_tickets: _session_tickets,
        } = conn;

        Self::handle_connection(incoming, connection, channel, connection_map)
    }

    pub fn run(&mut self) -> Result<(), Error> {
        // Initial setup and config.
        let mut builder = quinn::EndpointBuilder::from_config(quinn::Config {
            max_remote_bi_streams: 64,
            ..Default::default()
        });
        builder.listen();

        let (keys, cert_chain) = Self::load_tls_keys(&self.options.key, &self.options.cert)?;
        builder.set_certificate(cert_chain, keys[0].clone())?;

        if self.options.logproto {
            Self::enable_protocol_logging(&mut builder);
        };

        // Start listening for connections.
        info!("Binding to {:?}", self.options.listen);
        let (endpoint, driver, incoming) = builder.bind(self.options.listen)?;
        let incoming_message_channel = self.incoming_messages.clone();
        let connection_map = self.connections.clone();
        self.runtime.spawn(incoming.for_each(move |conn| {
            Self::handle_incoming_connection(
                conn,
                incoming_message_channel.clone(),
                connection_map.clone(),
            );
            Ok(())
        }));

        // Client stuff.
        if let Some(ref bootstrap_addr) = self.options.bootstrap_peer {
            let mut client_builder = quinn::ClientConfigBuilder::new();
            // We basically by definition don't know the peer's cert, so
            // this is ok.
            client_builder.accept_insecure_certs();
            let client_config = client_builder.build();

            // This is the other peer's hostname for SSL verification.
            // We accept insecure certs, and don't follow CA's, so it's a
            // placeholder.
            let host_str = "some_peer";

            let incoming_message_channel = self.incoming_messages.clone();
            let connection_map = self.connections.clone();
            let bootstrap_connection_future: Box<dyn Future<Item = (), Error = ()>> = Box::new(
                endpoint
                    .connect_with(&client_config, &bootstrap_addr, host_str)?
                    .map_err(|e| error!("failed to connect: {}", e))
                    .and_then(move |conn| {
                        Self::handle_outgoing_connection(
                            conn,
                            incoming_message_channel.clone(),
                            connection_map.clone(),
                        );
                        Ok(())
                    }),
            );
            self.runtime.spawn(bootstrap_connection_future);
        }

        self.runtime.block_on(driver)?;

        Ok(())
    }
}
