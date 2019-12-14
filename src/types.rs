//! Useful types used throughout the program, I suppose.

use crate::hash::{Blake2Hash, BLAKE2_HASH_SIZE};
use log::*;
use serde_derive::*;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;

/// A hash identifying a Peer.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerId(Blake2Hash);

impl PeerId {
    /// Creates a new `PeerId` from the given seed
    pub fn new(seed: &[u8]) -> PeerId {
        PeerId(Blake2Hash::new(seed))
    }

    /// Create a new `PeerId` that is exactly the given
    /// bytes.  Useful mainly for testing.
    #[allow(unused)]
    pub(crate) fn raw(bytes: [u8; BLAKE2_HASH_SIZE]) -> Self {
        PeerId(Blake2Hash(bytes))
    }

    /// Creates a new `PeerId` from an UNSECURE pRNG keykey.
    /// Useful for testing only!
    pub fn new_insecure_random(rng: &mut oorandom::Rand64) -> PeerId {
        // Blake2 hash needs 64 bytes, or 8 u64's
        // See https://github.com/Lokathor/bytemuck/issues/11
        let mut data = [0u64; BLAKE2_HASH_SIZE];
        data[..].iter_mut().for_each(|v| *v = rng.rand_u64());
        let data_bytes = bytemuck::bytes_of(&data);
        PeerId(Blake2Hash::new(data_bytes))
    }

    pub fn to_base64(&self) -> String {
        self.0.to_base64()
    }

    /// Returns some number N which is `floor(log2(the distance
    /// between this `PeerId` and the given one)`.
    pub fn distance_rank(&self, other: &PeerId) -> u32 {
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
pub struct Contact {
    peer_id: PeerId,
    address: SocketAddr,
}

impl Contact {
    /// New dummy contact with invalid address, for testing.
    #[allow(unused)]
    pub(crate) fn new_dummy(peer_id: PeerId) -> Self {
        use std::net::{Ipv6Addr, SocketAddrV6};
        let address = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 1234, 0, 0));
        Self { peer_id, address }
    }
}

impl PartialOrd for Contact {
    /// Contact info is ordered by `peer_id`,
    /// so this just calls `self.cmp`.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Contact {
    /// Contact info is ordered by `peer_id`
    fn cmp(&self, other: &Self) -> Ordering {
        self.peer_id.cmp(&other.peer_id)
    }
}

/// A "bucket" in the DHT, a collection of `Contact`'s with
/// a fixed max size.
#[derive(Debug, Clone)]
struct Bucket {
    /// The peers in the bucket.
    /// TODO: HashMap?  We need to make `Blake2Hash` impl `Hash` and idgaf right now.
    known_peers: HashMap<PeerId, SocketAddr>,
}

impl Bucket {
    fn new() -> Self {
        Self {
            known_peers: HashMap::new(),
        }
    }

    /// Adds the new peer to the bucket.
    ///
    /// TODO: Remove a peer if the bucket gets too big.
    /// ALSO TODO: Ponder using `replace()` instead of `insert()`, since
    /// we say Contact's are equal if their PeerId's are equal; the address
    /// may have changed.
    fn insert(&mut self, peer: Contact) {
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
pub struct PeerMap {
    buckets: Vec<Bucket>,
    /// The maximum number of peers per bucket.
    /// Currently hardwired at 8 in `new()`, but there's no reason we wouldn't
    /// want the ability to fiddle with it at runtime.
    bucket_size: usize,
}

impl Default for PeerMap {
    fn default() -> Self {
        let bucket_size = 16;
        let initial_bucket = Bucket::new();
        Self {
            buckets: vec![initial_bucket; Blake2Hash::max_power()],
            bucket_size,
        }
    }
}

impl PeerMap {
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
        let new_peer = Contact { peer_id, address };
        let distance_rank = self_id.distance_rank(&peer_id);
        self.buckets[distance_rank as usize].insert(new_peer);
        debug!(
            "Inserted peer {:?} at distance {}, now have {} peers",
            peer_id,
            distance_rank,
            self.count()
        );
    }

    /// Returns a Vec of the `bucket_size` closest peers we can find to the given one,
    /// in no particular order.
    fn find_closest_peers(&self, self_id: PeerId, peer_id: PeerId) -> Vec<(PeerId, SocketAddr)> {
        // The closest peers will be in the same bucket as the target peer.
        let search_bucket = self_id.distance_rank(&peer_id) as usize;
        let mut search_width = 1;
        let mut search_results = vec![];
        search_results.extend(self.buckets[search_bucket].known_peers.iter());
        while search_results.len() < self.bucket_size && search_width < Blake2Hash::max_power() {
            // trace!("searching width {} from {}", search_width, search_bucket);
            // trace!("search_results: {:?}", search_results);
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
        let search_bucket = self_id.distance_rank(&peer_id) as usize;
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

    /// Returns total number of peers known.
    /// TODO: usize? u64?
    pub fn count(&self) -> u32 {
        self.buckets
            .iter()
            .fold(0, |acc, bucket| acc + (bucket.known_peers.len() as u32))
    }
}

/// The peer's view of the DHT, a mapping of PeerId to contact info.
/// As per Kademila and Bittorrent DHT, the further away from the peer's
/// hash (as measured by the XOR distance metric), the lower resolution
/// it is.
///
/// This is implemented as a flat vec of Contacts.  Each Option<Contact> is
/// 64 bytes, so with our default bucket_size of 16 this is 256 kb.
/// Its size is `bits in the PeerID hash * bucket size * size of Contact`.
pub struct FlatPeerMap {
    /// All our known contacts, stored as a flat, sparse array.
    /// We only store a few thousand (with default bucket_size = 8)
    /// and most of what we do is rummage through a single bucket_size
    /// chunk, usually in order, and so this should be a fairly
    /// simple and fast representation.
    ///
    /// Very much an open-address hash table with a fixed size,
    /// actually.
    contacts: Vec<Option<Contact>>,
    /// Number of contacts contained.
    count: usize,
    /// The maximum number of peers per bucket.
    bucket_size: usize,
    /// Our peer ID.  We usually need this to calculate distances.
    peer_id: PeerId,
}

impl FlatPeerMap {
    pub fn new(peer_id: PeerId, bucket_size: usize) -> Self {
        Self {
            contacts: vec![None; Blake2Hash::max_power() as usize * bucket_size],
            count: 0,
            bucket_size,
            peer_id,
        }
    }
    pub fn default(peer_id: PeerId) -> Self {
        // 16 might actually be a somewhat conservative minimum.
        // On average we store like, log2 of the things we actually
        // see or something...
        Self::new(peer_id, 16)
    }

    /// Fetches the bucket that the contact should be in.
    fn bucket_for(&self, peer_id: &PeerId) -> &[Option<Contact>] {
        let distance_rank = self.peer_id.distance_rank(&peer_id) as usize;
        let bucket_start = distance_rank * self.bucket_size;
        let bucket_end = bucket_start + self.bucket_size;
        debug!("Peer {:?} is at distance {}", peer_id, distance_rank,);
        &self.contacts[bucket_start..bucket_end]
    }

    /// Fetches the bucket that the contact should be in.
    /// I really wish functions could be generic over mutability.
    fn bucket_for_mut(&mut self, peer_id: &PeerId) -> &mut [Option<Contact>] {
        let distance_rank = self.peer_id.distance_rank(&peer_id) as usize;
        let bucket_start = distance_rank * self.bucket_size;
        let bucket_end = bucket_start + self.bucket_size;
        debug!("Peer {:?} is at distance {}", peer_id, distance_rank,);
        &mut self.contacts[bucket_start..bucket_end]
    }

    /// Insert a new peer into the PeerMap.
    ///
    /// TODO: Should return an error or something if doing so
    /// would need to evict a current peer; that should be based
    /// on peer quality measures we don't track yet.
    ///
    /// For now though, we don't even bother splitting buckets or such.
    ///
    /// We DO prevent duplicates though; if a peer is given that has a peer_id
    /// that already exists in the map, it will replace the old one.
    pub fn insert(&mut self, new_peer: Contact) {
        let bucket = self.bucket_for_mut(&new_peer.peer_id);
        for entry in bucket {
            match entry {
                // We know this peer, update it
                Some(Contact { peer_id, .. }) if *peer_id == new_peer.peer_id => {
                    *entry = Some(new_peer);
                    break;
                }
                // Wrong peer, carry on
                Some(_) => (),
                // Empty slot, snag it.
                None => {
                    *entry = Some(new_peer);
                    self.count += 1;
                    break;
                }
            };
        }
        // If we've gotten here, the bucket is full!
        // TODO: Evict something!
    }

    /// Checks whether we know about the given peer
    pub fn contains(&self, peer_id: &PeerId) -> Option<Contact> {
        let bucket = self.bucket_for(&peer_id);
        bucket
            .iter()
            .filter_map(|maybe_contact| maybe_contact.as_ref())
            .find(|contact| contact.peer_id == *peer_id)
            .cloned()
    }

    /// Return a `Vec<Contact>` containing the `count peers (or the most we have anyway)
    /// closest to the given one.  Best case `O(1)`, if `count == bucket_size` and
    /// our peer map is dense/full.  Worst case `O(N)` if we have to search our
    /// whole peer map to scrape together `count` peers.
    pub fn find_closest_peers(&self, needle: &PeerId, count: usize) -> Vec<Contact> {
        assert!(count <= self.contacts.len(), "Good luck with that buddy");

        let mut results: Vec<Contact> = Vec::with_capacity(count);
        results.extend(self.bucket_for(&needle).iter().filter_map(|x| *x));
        let target_bucket = self.peer_id.distance_rank(&needle) as usize;
        let mut search_width = 1;
        // We step incrementally further away from the needle's bucket, just
        // tossing one bucket at a time into the results vec.  This is sloppy
        // and I kinda expect will kinda thrash the cache, but, it's simple and
        // it works.
        //
        // A BTreeMap or something like it might be a better solution...
        // More complex though.
        while results.len() < count && search_width < Blake2Hash::max_power() {
            // Add bucket ahead of target
            {
                // TODO: I am annoyed that this duplicates part of `bucket_for()`
                let target = target_bucket.wrapping_add(search_width) % Blake2Hash::max_power();
                let bucket_start = target * self.bucket_size;
                let bucket_end = bucket_start + self.bucket_size;
                let bucket = &self.contacts[bucket_start..bucket_end];
                results.extend(bucket.iter().filter_map(|x| *x));
            }

            // Add bucket behind target
            {
                let target = target_bucket.wrapping_sub(search_width) % Blake2Hash::max_power();
                let bucket_start = target * self.bucket_size;
                let bucket_end = bucket_start + self.bucket_size;
                let bucket = &self.contacts[bucket_start..bucket_end];
                results.extend(bucket.iter().filter_map(|x| *x));
            }

            search_width += 1;
        }
        // Trim our results down to the desired count
        results.truncate(count);
        results
    }

    /// Looks up the given peer id.  If we know about it, return the address we have,
    /// otherwise return a list of the nearest peers we know to it.
    pub fn lookup(&self, peer_id: &PeerId) -> Result<Contact, Vec<Contact>> {
        if let Some(res) = self.contains(peer_id) {
            Ok(res)
        } else {
            Err(self.find_closest_peers(peer_id, self.bucket_size))
        }
    }

    /// Returns total number of peers known.
    pub fn count(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oorandom::Rand64;
    // TODO: Verify
    #[test]
    fn test_distance_rank() {
        let p1 = PeerId::raw([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, //
        ]);
        let p2 = PeerId::raw([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, //
        ]);
        assert_eq!(p1.distance_rank(&p2), 255);
        let p3 = PeerId::raw([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, //
        ]);
        assert_eq!(p1.distance_rank(&p3), 246);
    }

    // TODO: Verify harder
    #[test]
    fn test_distance_rank2() {
        let p1 = PeerId::raw([
            0, 1, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, //
            0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, //
        ]);
        let p2 = PeerId::raw([
            0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, //
            0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, //
        ]);
        assert_eq!(p1.distance_rank(&p2), 38);
    }

    #[test]
    fn peermap_contains() {
        let rng = &mut Rand64::new(235445);
        let id = PeerId::new_insecure_random(rng);
        let mut map = FlatPeerMap::default(id);
        // TODO: Using random ID's is not actually a great test,
        // since statistically half of them will end up in the furthest-most
        // bucket and having more than 12 of them or so will overflow the
        // bucket and get evicted.
        let contains: Vec<Contact> = (0..12)
            .map(|_| Contact::new_dummy(PeerId::new_insecure_random(rng)))
            .collect();
        for c in &contains {
            map.insert(*c);
        }
        for c in &contains {
            assert!(map.contains(&c.peer_id).is_some());
        }

        let does_not_contain: Vec<Contact> = (0..100)
            .map(|_| Contact::new_dummy(PeerId::new_insecure_random(rng)))
            .collect();
        for c in &does_not_contain {
            assert!(map.contains(&c.peer_id).is_none());
        }
    }

    #[test]
    fn peermap_find_closest() {
        let rng = &mut Rand64::new(235445);
        let id = PeerId::new_insecure_random(rng);
        let mut map = FlatPeerMap::new(id, 16);
        let contains: Vec<Contact> = (0..0xFF)
            .map(|_| Contact::new_dummy(PeerId::new_insecure_random(rng)))
            .collect();
        for c in &contains {
            map.insert(*c);
        }

        let itms = map.find_closest_peers(&id, 16);
        // TODO: Hmm how to test this.  :|
        assert_eq!(itms.len(), 16);
        assert_eq!(map.count(), 84);
    }
}
