//! Useful types used throughout the program, I suppose.

use hash::Blake2Hash;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;

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
        let distance = self.0 ^ other.0;
        distance.leading_zeros()
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
    /// TODO: HashMap?  We need to make `Blake2Hash` impl `Hash` and idgaf right now.
    known_peers: BTreeMap<PeerId, SocketAddr>,
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
            known_peers: BTreeMap::new(),
            address_range: (min_address, max_address),
        }
    }

    /// Adds the new peer to the bucket.
    ///
    /// TODO: Remove a peer if the bucket gets too big.
    /// ALSO TODO: Ponder using `replace()` instead of `insert()`, since
    /// we say ContactInfo's are equal if their PeerId's are equal; the address
    /// may have changed.
    fn insert(&mut self, peer: ContactInfo) {
        if self
            .known_peers
            .insert(peer.peer_id, peer.address)
            .is_none()
        {
            info!("Peer {:?} got new address: {}", peer.peer_id, peer.address);
        }
    }

    fn contains(&self, peer: PeerId) -> Option<(PeerId, SocketAddr)> {
        self.known_peers.get(&peer).map(|addr| (peer, *addr))
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
            buckets: vec![initial_bucket; Blake2Hash::max_power()],
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
        let distance_rank = self_id.distance_rank(peer_id);
        self.buckets[distance_rank as usize].insert(new_peer);
    }

    /// Returns a Vec of the `bucket_size` closest peers we can find to the given one,
    /// in no particular order.
    fn find_closest_peers(&self, self_id: PeerId, peer_id: PeerId) -> Vec<(PeerId, SocketAddr)> {
        // The closest peers will be in the same bucket as the target peer.
        let search_bucket = self_id.distance_rank(peer_id) as usize;
        let mut search_width = 1;
        let mut search_results = vec![];
        search_results.extend(self.buckets[search_bucket].known_peers.iter());
        while search_results.len() < self.bucket_size && search_width < Blake2Hash::max_power() {
            trace!("searching width {} from {}", search_width, search_bucket);
            trace!("search_results: {:?}", search_results);
            let target_behind = search_bucket.saturating_sub(search_width);
            // This one heckin' better not overflow.
            let target_ahead = search_bucket + search_width;

            // target_behind is negative, or would be if we weren't doing saturating_sub()
            // on an unsigned integer.
            if search_width > search_bucket {
                search_results.extend(self.buckets[target_behind].known_peers.iter());
            }

            if target_ahead < Blake2Hash::max_power() as usize {
                search_results.extend(self.buckets[target_ahead].known_peers.iter());
            }
            search_width += 1;
        }
        search_results
            .iter()
            .map(|ci: &(&PeerId, &SocketAddr)| (*ci.0, *ci.1))
            .take(self.bucket_size)
            .collect()
    }

    /// Just checks whether we know about the given peer
    pub fn contains(&self, self_id: PeerId, peer_id: PeerId) -> Option<(PeerId, SocketAddr)> {
        let search_bucket = self_id.distance_rank(peer_id) as usize;
        self.buckets[search_bucket].contains(peer_id)
    }

    /// Looks up the given peer id.  If we know about it, return the address we have,
    /// otherwise return a list of the nearest peers we know to it.
    pub fn lookup(
        &self,
        self_id: PeerId,
        peer_id: PeerId,
    ) -> Result<(PeerId, SocketAddr), Vec<(PeerId, SocketAddr)>> {
        if let Some(res) = self.contains(self_id, peer_id) {
            Ok(res)
        } else {
            Err(self.find_closest_peers(self_id, peer_id))
        }
    }
}
