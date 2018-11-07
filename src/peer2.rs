use std::path::Path;

use failure::{err_msg, Error, ResultExt};
use futures::{Future, Stream};
use quinn;
use rustls;
use tokio;
use tokio::runtime::current_thread::Runtime;

use crate::PeerOpt;

pub struct Peer {
    options: PeerOpt,
    runtime: Runtime,
}

impl Peer {
    pub fn new(options: PeerOpt) -> Result<Self, Error> {
        let runtime = Runtime::new()?;
        Ok(Peer { options, runtime })
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

    /// Needs to return a `Result` so it can be used as a future.
    fn handle_incoming(conn: quinn::NewConnection) -> impl Future<Item = (), Error = ()> {
        info!(
            "Incoming connection from: {:?}",
            conn.connection.remote_address()
        );
        let quinn::NewConnection {
            connection,
            incoming,
        } = conn;
        Self::talk_to_peer(connection, incoming)
    }

    fn handle_outgoing(conn: quinn::NewClientConnection) -> impl Future<Item = (), Error = ()> {
        info!(
            "Connected to bootstrap peer: {:?}",
            conn.connection.remote_address()
        );

        let quinn::NewClientConnection {
            connection,
            incoming,
            session_tickets: _session_tickets,
        } = conn;
        Self::talk_to_peer(connection, incoming)
    }

    /// Implements the state machine of actually talking to a peer...
    fn talk_to_peer(
        conn: quinn::Connection,
        incoming: quinn::IncomingStreams,
    ) -> impl Future<Item = (), Error = ()> {
        let outgoing_stream: Box<dyn Future<Item = (), Error = ()>> = Box::new(
            conn.open_bi()
                .map_err(|e| format_err!("failed to open stream: {:?}", e))
                .then(move |stream| {
                    info!("Sending message to peer");
                    let s = stream.expect("Could not unwrap stream?");
                    let msg = b"Foo!";
                    tokio::io::write_all(s, msg)
                        .and_then(|(stream, _vec)| tokio::io::shutdown(stream))
                        .map_err(|e| warn!("Failed to send request: {}", e))
                        .map(move |_| debug!("Message send complete: {:X?}", msg))
                }),
        );

        // For each incoming stream, try to receive a message on it.
        // TODO: Should this spawn a new task for each stream?  mmmmmmmmaybe.
        let incoming_streams: Box<dyn Future<Item = (), Error = ()>> = Box::new(
            incoming
                .map_err(|e| warn!("Incoming stream failed: {:?}", e))
                .for_each(move |stream: quinn::NewStream| {
                    info!("Peer created incoming stream");
                    // Don't bother with uni-directional streams yet.
                    match stream {
                        quinn::NewStream::Bi(bi_stream) => quinn::read_to_end(bi_stream, 64 * 1024)
                            .map_err(|e| format_err!("failed reading request: {}", e))
                            .inspect(|(_stream, res)| {
                                info!("Got message: {:?}", res);
                            }).map_err(|e| warn!("Incoming stream failed: {:?}", e))
                            .and_then(|(stream, _res)| {
                                tokio::io::shutdown(stream)
                                    .map(|_| ())
                                    .map_err(|e| warn!("Error shutting down stream: {:?}", e))
                            }),
                        quinn::NewStream::Uni(_) => unimplemented!(),
                    }
                }),
        );

        let merged_stream_handlers = outgoing_stream.join(incoming_streams).map(|((), ())| ());

        merged_stream_handlers.and_then(move |()| {
            // info!("Closing connection to {:?}", conn.remote_address());
            // conn.close(0, b"done")
            Ok(())
        })
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
        self.runtime.spawn(incoming.for_each(Self::handle_incoming));

        // Client stuff.
        if let Some(ref bootstrap_addr) = self.options.bootstrap_peer {
            let mut client_builder = quinn::ClientConfigBuilder::new();
            // We basically by definition don't know the peer's cert, so
            // this is ok.
            client_builder.accept_insecure_certs();
            let client_config = client_builder.build();

            // This is the other peer's hostname for SSL verification.
            // We accept insecure certs, and don't follow CA's, so it's placeholder.
            let host_str = "some_peer";
            let bootstrap_connection_future: Box<dyn Future<Item = (), Error = ()>> = Box::new(
                endpoint
                    .connect_with(&client_config, &bootstrap_addr, host_str)?
                    .map_err(|e| error!("failed to connect: {}", e))
                    .and_then(Self::handle_outgoing)
                    .inspect(|()| info!("Disconnected from bootstrap")),
            );
            self.runtime.spawn(bootstrap_connection_future);
        }

        self.runtime.block_on(driver)?;

        Ok(())
    }
}
