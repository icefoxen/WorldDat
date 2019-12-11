#!/bin/sh
#
# A script to start a node and have another one connect to it, to just
# try to simplify hand-testing a little.

env RUST_LOG=trace cargo run -- --cert certs/server.chain --key certs/server.rsa &

env RUST_LOG=trace cargo run -- -l '[::]:5555' -b quic://127.0.0.1:4433 --cert certs/server.chain --key certs/server.rsa --ca certs/ca.der
