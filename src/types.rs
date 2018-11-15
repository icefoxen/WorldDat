//! Useful types used throughout the program, I suppose.

use hash::{self, Blake2Hash};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;
use std::net::SocketAddr;
use std::ops::BitXor;

/// A hash identifying a Peer.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PeerId(Blake2Hash);

impl PeerId {
    /// Creates a new `PeerId` from the given seed
    pub fn new(seed: &[u8]) -> PeerId {
        PeerId(Blake2Hash::new(seed))
    }

    /// Creates a new `PeerId` from an UNSECURE pRNG keykey.
    /// Useful for testing only!
    pub fn new_insecure_random() -> PeerId {
        use rand::{self, Rng};
        let v = &mut vec![0; 64];
        rand::thread_rng().fill(v.as_mut_slice());
        PeerId(Blake2Hash::new(v))
    }

    pub fn to_base64(&self) -> String {
        self.0.to_base64()
    }

    /// Returns some number N which is `floor(log2(the distance
    /// between this `PeerId` and the given one)`.
    pub fn distance_rank(&self, other: PeerId) -> u32 {
        let mut res: u32 = 0;
        let distance = self.0 ^ other.0;
        fn log2(x: u8) -> u32 {
            8 - x.leading_zeros() - 1
        }
        for d in &distance.0[..] {
            // TODO: Double check
            res += log2(*d);
        }
        res
    }

    pub fn bytes(&self) -> &[u8] {
        &(self.0).0[..]
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PEERID:{}", self.to_base64())
    }
}

/// The actual serializable messages that can be sent back and forth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
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
        id: PeerId,
        addr: SocketAddr,
    },
    FindPeerResponsePeerNotFound {
        id: PeerId,
        /// It's ok to have this be unbounded size 'cause we only receive a fixed-size
        /// buffer from any client...  I'm pretty sure.
        /// TODO: Verify!
        neighbors: Vec<(PeerId, SocketAddr)>,
    },
}

/// Contact info for a peer, mapping the `PeerId` to an IP address and port.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ContactInfo {
    peer_id: PeerId,
    address: SocketAddr,
}

impl PartialOrd for ContactInfo {
    /// Contact info is ordered by `peer_id`,
    /// so this just calls `self.cmp`.
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
#[derive(Debug, Clone)]
struct Bucket {
    /// The peers in the bucket.
    known_peers: BTreeSet<ContactInfo>,
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
            known_peers: BTreeSet::new(),
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
            buckets: vec![initial_bucket; hash::BLAKE2_HASH_SIZE * 8],
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
    pub fn insert(&mut self, self_id: PeerId, address: SocketAddr, peer_id: PeerId) {
        let new_peer = ContactInfo { peer_id, address };

        // /////
        // Find which bucket the peer SHOULD be in.
        // The list of buckets should be short, so linear search should be fast.
        let target_bucket = self
            .buckets
            .iter_mut()
            .find(|bucket| true)
            .expect("Can't find bucket to add new peer to; should never happen!!");
        if target_bucket.known_peers.len() > self.bucket_size {
            // Split bucket
        } else {
            // Just insert the thing
            target_bucket.known_peers.insert(new_peer);
        }
        // /////

        /*
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
        */
    }

    pub fn lookup(
        &self,
        _peer_id: PeerId,
    ) -> Result<(PeerId, SocketAddr), Vec<(PeerId, SocketAddr)>> {
        Err(vec![])
    }
}
