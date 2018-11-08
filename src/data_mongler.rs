//! Name is a bit in progress.
//! This is basically the bit that actually makes the decisions and sends messages
//! to/from the networking thread.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc;

use crate::types::*;

pub struct PeerState {
    /// The stream of all messages we receive from all peers.
    incoming_messages: mpsc::Receiver<(SocketAddr, Message)>,
    /// One channel per active connection.
    outgoing_messages_map: HashMap<SocketAddr, mpsc::Sender<Message>>,
}

impl PeerState {
    pub fn new(message_channel: mpsc::Receiver<(SocketAddr, Message)>) -> Self {
        Self {
            incoming_messages: message_channel,
            outgoing_messages_map: HashMap::new(),
        }
    }

    pub fn send(dest: SocketAddr, msg: Message) {
        // If socketaddr in map, send message through channel.
        // If channel doesn't accept it, then that connection
        // got closed and we have to re-open it.

        // If we don't have an open connection to that address,
        // either 'cause the connection closed or we never had one,
        // we have to ask the networking side of the protocol
        // to open one.
    }
}
