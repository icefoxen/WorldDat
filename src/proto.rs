use std::net::SocketAddr;

use serde_derive::*;

use crate::types::*;

/// The actual serializable messages that can be sent back and forth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {}

pub enum PingRequest {
    Ping { id: PeerId },
}

pub enum PingResponse {
    Pong { id: PeerId },
}

pub enum FindPeerRequest {
    FindPeer { id: PeerId },
}

pub enum FindPeerResponse {
    FindPeerResponsePeerFound {
        id: PeerId,
        addr: SocketAddr,
    },
    FindPeerResponsePeerNotFound {
        id: PeerId,
        /// It's ok to have this be unbounded size 'cause we only receive a fixed-size
        /// buffer from any client...  I'm pretty sure.
        /// TODO: Verify!
        neighbors: Vec<Contact>,
    },
}
