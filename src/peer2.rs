use std::path::Path;

use failure::{err_msg, Error, ResultExt};
use futures::Stream;
use quinn;
use rustls;
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
        let (endpoint, driver, incoming) = builder.bind(self.options.listen)?;
        self.runtime.spawn(incoming.for_each(move |conn| {
            info!(
                "Incoming connection from: {:?}",
                conn.connection.remote_address()
            );
            Ok(())
        }));

        self.runtime.block_on(driver)?;

        Ok(())
    }
}
