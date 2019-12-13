//! This should be the basic peer program that actually does stuff.
//!
//! TODO:
//! SSL certs: By default, we accept any SSL cert as trusted.  This
//! is actually kinda okay, 'cause while we can be MITM'ed, all our
//! actual data is content-addressed so a MITM can't do more than
//! waste our time.  So, we can OPTIONALLY provide a peer with a
//! list of SSL certs to trust.
//!
//! Run with:
//! env RUST_LOG=peer=trace cargo run --bin peer

use std::net::SocketAddr;

use structopt::StructOpt;

use tokio::runtime::Builder;

/// Command line options for a peer node.
#[derive(StructOpt, Debug, Clone)]
pub struct Opt {
    /// Initial node to connect to, if any.
    #[structopt(short = "b", long = "bootstrap", default_value = "[::1]:4433")]
    bootstrap_peer: SocketAddr,

    /// Address to listen on for incoming connections.
    /// TODO: it would be nice to have
    /// a fetch-only peer sometime.
    #[structopt(short = "l", long = "listen", default_value = "[::]:4433")]
    listen: SocketAddr,

    /// Number of threads to spawn.  Defaults to the
    /// number of CPU's on the machine.
    #[structopt(short = "t", long = "t")]
    num_threads: Option<u16>,
    /* TODO: Accept these instead of always making a (new) self-signed cert
    /// Certificate authority key, if any.
    #[structopt(parse(from_os_str), long = "ca")]
    ca: Option<PathBuf>,

    /// This node's TLS private key in PEM format.
    #[structopt(parse(from_os_str), short = "k", long = "key")]
    key: PathBuf,
    /// This node's TLS certificate in PEM format.
    #[structopt(parse(from_os_str), short = "c", long = "cert")]
    cert: PathBuf,
    */
}

trait ResultExt<T, E> {
    /// Lets you pass a thunk that does a THING to an Err
    /// without mutating it.  Mainly for logging.
    ///
    /// This is TOTALLY NECESSARY and not overkill at all.
    /// Honest.
    ///
    /// Also see the `tap` crate.
    fn inspect_err<F>(self, f: F) -> Self
    where
        F: FnOnce(&E);
}

impl<T, E> ResultExt<T, E> for Result<T, E>
where
    E: std::fmt::Debug,
{
    fn inspect_err<F>(self, f: F) -> Self
    where
        F: FnOnce(&E),
    {
        match self {
            Err(ref e) => f(e),
            _ => (),
        }
        self
    }
}

mod server {
    use std::{error::Error, net::SocketAddr, sync::Arc};

    use futures::{StreamExt, TryFutureExt};
    use log::*;
    use tokio::task::JoinHandle;

    use quinn::{
        Certificate, CertificateChain, Endpoint, EndpointDriver, Incoming, PrivateKey,
        ServerConfig, ServerConfigBuilder, TransportConfig,
    };

    use crate::ResultExt;

    /// Returns default server configuration along with its certificate.
    fn configure_server() -> Result<(ServerConfig, Vec<u8>), Box<dyn Error>> {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_der = cert.serialize_der().unwrap();
        let priv_key = cert.serialize_private_key_der();
        let priv_key = PrivateKey::from_der(&priv_key)?;

        let server_config = ServerConfig {
            transport: Arc::new(TransportConfig {
                stream_window_uni: 0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut cfg_builder = ServerConfigBuilder::new(server_config);
        let cert = Certificate::from_der(&cert_der)?;
        cfg_builder.certificate(CertificateChain::from_certs(vec![cert]), priv_key)?;

        Ok((cfg_builder.build(), cert_der))
    }

    /// Constructs a QUIC endpoint configured to listen for incoming connections on a certain address
    /// and port.
    ///
    /// ## Returns
    ///
    /// - UDP socket driver
    /// - a stream of incoming QUIC connections
    /// - server certificate serialized into DER format
    pub fn make_server_endpoint(
        bind_addr: SocketAddr,
    ) -> Result<(EndpointDriver, Incoming, Vec<u8>), Box<dyn Error>> {
        let (server_config, server_cert) = configure_server()?;
        let mut endpoint_builder = Endpoint::builder();
        endpoint_builder.listen(server_config);
        info!("Listening on address {}", bind_addr);
        let (driver, _endpoint, incoming) = endpoint_builder.bind(&bind_addr)?;
        Ok((driver, incoming, server_cert))
    }

    /// Server function to handle an incoming request, write
    /// a response to it, and (I think) close the stream.
    async fn handle_request(
        (mut send, recv): (quinn::SendStream, quinn::RecvStream),
    ) -> Result<(), ()> {
        info!("Handling request");
        let req = recv
            .read_to_end(64 * 1024)
            .await
            .map_err(|e| error!("failed reading request: {:?}", e))?;
        info!("Data received");
        let resp = format!("Received: {}", String::from_utf8_lossy(&req));
        // Write the response
        send.write_all(resp.as_bytes())
            .await
            .map_err(|e| error!("failed to send response: {}", e))?;
        info!("Response sent");
        // Gracefully terminate the stream
        send.finish()
            .await
            .map_err(|e| error!("failed to shutdown stream: {}", e))?;
        info!("complete");
        Ok(())
    }

    /// Server handling an incoming connection
    async fn handle_connection(conn: quinn::Connecting) -> Result<(), quinn::ConnectionError> {
        // Think this actually accepts the connection
        let quinn::NewConnection {
            driver,
            connection,
            mut bi_streams,
            ..
        } = conn
            .await
            .inspect_err(|e| error!("Error in new connection: {:?}", e))?;
        info!(
            "connection established: id={} addr={}",
            connection.remote_id(),
            connection.remote_address()
        );

        // This is the part that handles the low-level UDP stuff
        // and passes it to/from the quinn state machine
        tokio::spawn(driver.unwrap_or_else(|e| error!("UDP I/O error: {}", e)));
        async move {
            info!("Connection established");
            // Client can open multiple streams, each a new request.
            while let Some(stream) = bi_streams.next().await {
                info!("Handling stream");
                let stream = match stream {
                    Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                        info!("connection closed");
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(e);
                    }
                    Ok(s) => s,
                };

                tokio::spawn(
                    handle_request(stream).unwrap_or_else(move |e| {
                        error!("Server could not handle request: {:?}", e)
                    }),
                );
            }
            Ok(())
        }
            .await
            .inspect_err(|e| error!("Something failed while handling streams: {:?}", e))
    }

    /// Runs a QUIC server bound to given address.
    pub fn run_server(addr: SocketAddr) -> JoinHandle<()> {
        let (driver, mut incoming, _server_cert) = make_server_endpoint(addr).expect("TODO");
        // drive UDP socket
        let endpoint_driver = tokio::spawn(driver.unwrap_or_else(|e| panic!("IO error: {}", e)));
        // accept incoming connections forever.
        tokio::spawn(async move {
            // `incoming` is not actually an iterator, so, we do
            // while instead of for
            while let Some(incoming_conn) = incoming.next().await {
                info!("Connection incoming");
                tokio::spawn(
                    handle_connection(incoming_conn)
                        .unwrap_or_else(|_| error!("Connection handling failed")),
                );
            }
        });
        endpoint_driver
    }
}

mod client {
    use std::{
        net::{Ipv6Addr, SocketAddr},
        sync::Arc,
    };

    use futures::TryFutureExt;
    use log::*;
    use tokio::task::JoinHandle;

    use quinn::{ClientConfig, ClientConfigBuilder, Endpoint};
    /// Dummy certificate verifier that treats any certificate as valid.
    /// NOTE, such verification is vulnerable to MITM attacks, but convenient for testing.
    /// TODO: We still need to think about this more, tbh.
    struct SkipServerVerification;

    impl SkipServerVerification {
        fn new() -> Arc<Self> {
            Arc::new(Self)
        }
    }

    impl rustls::ServerCertVerifier for SkipServerVerification {
        fn verify_server_cert(
            &self,
            _roots: &rustls::RootCertStore,
            _presented_certs: &[rustls::Certificate],
            _dns_name: webpki::DNSNameRef,
            _ocsp_response: &[u8],
        ) -> Result<rustls::ServerCertVerified, rustls::TLSError> {
            Ok(rustls::ServerCertVerified::assertion())
        }
    }
    fn configure_client() -> ClientConfig {
        let mut cfg = ClientConfigBuilder::default().build();
        let tls_cfg: &mut rustls::ClientConfig = Arc::get_mut(&mut cfg.crypto).unwrap();
        // this is only available when compiled with "dangerous_configuration" feature
        tls_cfg
            .dangerous()
            .set_certificate_verifier(SkipServerVerification::new());
        cfg
    }

    pub fn run_client(server_addr: SocketAddr, message: Box<[u8]>) -> Result<JoinHandle<()>, ()> {
        let client_cfg = configure_client();
        let mut endpoint_builder = Endpoint::builder();
        endpoint_builder.default_client_config(client_cfg);

        let (driver, endpoint, _) = endpoint_builder
            .bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)))
            .expect("Could not build client endpoint; is the port already used or something?");
        tokio::spawn(driver.unwrap_or_else(|e| error!("Probably fatal IO error in client: {}", e)));

        // connect to server
        let handle = tokio::spawn(async move {
            let quinn::NewConnection {
                driver, connection, ..
            } = endpoint
                // Server name is used for cert checking
                .connect(&server_addr, "localhost")
                .unwrap()
                .await
                .unwrap();
            tokio::spawn(driver);
            info!(
                "connected: id={}, addr={}",
                connection.remote_id(),
                connection.remote_address()
            );

            let (mut send, recv) = connection
                .open_bi()
                .await
                .map_err(|e| info!("failed to open stream: {}", e))
                .expect("TODO");
            {
                info!("Streams opened");

                send.write_all(&*message)
                    .await
                    .map_err(|e| info!("failed to send request: {}", e))
                    .expect("TODO");
                info!("Wrote data");
                send.finish()
                    .await
                    .map_err(|e| info!("failed to shutdown stream: {}", e))
                    .expect("TODO");
                info!("Send stream shutdown");

                let resp = recv
                    .read_to_end(usize::max_value())
                    .await
                    .map_err(|e| info!("failed to read response: {}", e))
                    .expect("TODO");
                info!("Data received");

                use std::io;
                use std::io::Write;
                io::stdout().write_all(&resp).unwrap();
                io::stdout().flush().unwrap();
            }
            // Dropping handles allows the corresponding objects to automatically shut down
            drop((endpoint, connection));
        });

        Ok(handle)
    }
}

fn main() -> Result<(), ()> {
    let opt = Opt::from_args();
    pretty_env_logger::init();
    // server and client are running on the same thread asynchronously
    // To use a thread pool switch basic_scheduler() for threaded_scheduler()
    let mut runtime = Builder::new()
        .basic_scheduler()
        //.threaded_scheduler()
        .thread_name("peer-io-worker")
        .enable_all()
        .build()
        .expect("Could not build runtime");

    let handle = runtime.enter(|| {
        for i in 0..10 {
            let msg = format!("This is client {}", i);
            let msg_bytes = msg.into_boxed_str().into_boxed_bytes();
            tokio::spawn(client::run_client(opt.bootstrap_peer, msg_bytes).expect("Could not run client"));
        }
        tokio::spawn(server::run_server(opt.listen))
    });

    runtime
        .block_on(handle)
        .expect("Runtime errored while waiting for service to finish")
        .expect("???");
    Ok(())
}
