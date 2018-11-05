use std::net::ToSocketAddrs;
use std::thread;

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
                listen: "[::]:5544".to_socket_addrs().unwrap().next().unwrap(),
                ca: None,
                key: "certs/server.rsa".into(),
                cert: "certs/server.chain".into(),
                logproto: false,
            });

            peer.run().expect("Could not run peer?");
            Ok(())
        })
    };
}
#[test]
fn test_client_connection() {
    ::setup_logging();

    lazy_static::initialize(&SERVER_THREAD);

    // TODO: Make sure it actually fails when no server is running!
    // Currently it doesn't, it just times out... eventually.
    let mut peer = peer::Peer::new(PeerOpt {
        bootstrap_peer: Some(url::Url::parse("quic://[::1]:5544/").unwrap()),
        listen: "[::]:5545".to_socket_addrs().unwrap().next().unwrap(),
        ca: Some("certs/ca.der".into()),
        key: "certs/server.rsa".into(),
        cert: "certs/server.chain".into(),
        logproto: false,
    });

    let res = peer.start();

    // Block on futures and run them to completion.
    peer.runtime.run().map_err(Error::from).unwrap();
    assert!(res.is_ok());
}
