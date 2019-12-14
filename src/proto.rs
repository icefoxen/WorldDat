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
    /// We know the peer, here it is
    PeerFound { id: PeerId, addr: SocketAddr },
    /// We don't know the peer, will send the closest ones we know.
    PeerNotFound {
        id: PeerId,
        /// It's ok to have this be unbounded size 'cause we only receive a fixed-size
        /// buffer from any client...  I'm pretty sure.
        /// TODO: Verify!
        neighbors: Vec<Contact>,
    },
}

/// Ask if the destination has the given value
pub enum FindValueRequest {}

pub enum FindValueResponse {
    /// We have it, here it is
    ValueFound {},
    /// We don't have it, here's peers who...
    /// TODO: We know have it?  We know of who might have it?
    ValueNotFound { neighbors: Vec<Contact> },
}

/// We have this value
pub enum ValueAnnounce {
    Value,
}
