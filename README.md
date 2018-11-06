
# WorldDat

Build a system that does something like what IPFS does, but simpler and
easier to integrate in other programs.

Basically, a content-addressed block store with a Kademila-like DHT to search for and distribute blocks.

Inspirations: ipfs, BitTorrent, dat, secure scuttlebutt, ...

To build (on Debian):

 * Install Rust, if it's not already installed.
 * cd into the repo directory.
 * `cargo build`

To run:

 * Start a node with no bootstrap node: 
   `cargo run -- --cert certs/server.chain --key certs/server.rsa`
 * Start a node listening on a different port and tell it to use the existing one as a bootstrap node: 
   `cargo run -- -l '[::]:5555' -b quic://127.0.0.1:4433 --cert certs/server.chain --key certs/server.rsa --ca certs/ca.der`

To test:

 * `cargo test`

## Goals

simple to use, composable, robust, swift, easy to figure out and debug and incorporate
into other systems, quite lightweight, data is tamperproof (censorproof or monitoring-proof
is harder), transmission encrypted by default should help tho.

Non-goals: permanent, anonymous, the fastest evar, does everything all on its own, replaces everything in teh world

Goals that are not things to worry about YET: Preventing sybil attacks/robust keygen, fairness in block allocation/incentives to seed/abuse prevention (see bittorrent, ipfs), NAT holepunching,

## Notes

Having data encrypted when stored on the nodes with the key contained in the lookup hash sounds very nice but also should not be the job of this layer; higher-level programs can do it.  If data is intentionally public (such as a funny cat image) then you'd need to include the key with every link you provide to it, and if you had a block but didn't know the key for it you could just heckin' google it and someone would have it written down somewhere.  Having crypto be part of a higher-level file-like abstraction is probably nicer anyway 'cause you can have the key be per file instead of per block.

I think that this particular system will *stop* at the level of retrieving blocks, and let other systems build higher-level abstractions atop it.

You should be able to easily specify particular hosts to try to seek a block from, both by
ip/dns or by node hash. Hopefully helps the bootstrapping process some.  Hosts can't lie to you about the contents of a block, so this is Safe.

Example of something which *can not* be stored by content-addressed merkle-dag structures like this: Anything with loops!  For example, a heckin' webcomic archive.  You want forward and back links on each page of your archive.  If you embed them in the page then it changes the hash of the page and breaks the links of other pages pointing to it.  You *can not* represent loops between unassociated objects.  IPFS allows you to have "directories" of associated objects which can then refer to each other by non-content-addressed name, but...

Okay, hm.  Kademila eagerly locates blocks on nodes with a similar node ID hash.  IPFS doesn't do that though.  It locates the node info of where to FIND the block on nodes with a similar ID hash.

Coral does something different; it distributes things differently.  Investigate more.

Possibly useful crates for bignums: `num`, `ramp`, `rug`,

From the conversation here: https://www.reddit.com/r/rust/comments/9oa2n9/whats_everyone_working_on_this_week/e7tjq2a/

 * Bittorrent's DHT has pathological behavior in some cases, find out more.
 * Maidsafe and cjdns are worth investigating more.

## Sketch of DHT operation

Based almost entirely on Bittorrent, since it works well and has good documentation.

Okay.  So we start with a bootstrap set, and a NodeID.  We have a routing table that covers the entire NodeID space, divided into power-of-2-size buckets of size K (split the buckets when K is exceeded, bittorrent uses K=8).  Each routing table entry contains a NodeID, some quality metrics if necessary (Bittorrent uses some: good/bad/doubtful based on pings, last changed time, etc), contact info for that node (IP address+port),

Operations:

 * Ping: contains sender's NodeID, response is the queried node's NodeID.
 * Find node: Contains sender's node ID, and the NodeID being searched for.  When received, if the recipient knows the NodeID's info it responds with it, if not it responds with the K closest NodeID's it knows about.
 * Get peers: Get the peers that have the block (well, torrent) with a particular hash.  Takes the NodeID and the DataID.  Like find node, if the recipient doesn't know where to find the block, it will respond with a list of the closest nodes it does know.  BT also returns a token?
 * Announce peer: Announce that the peer is downloading a torrent.  Arguments: NodeID of querying node, DataID, port and token (from a previous get_peers).  The queryied node should then store the IP address and port of the sender as having access to that torrent.

These are analogous to Kademila (ping, store, find node, find value).  So our operations are probably going to be quite similar:

 * Ping
 * Find node
 * Find peers that have value
 * Announce value


## To do

Make a more proper comparison to IPFS: fewer layers of abstraction, easier/simpler protocol, client-only
implementations, easier ffi for official implementation. Generally just tries to move blocks
around instead of doing Everything. Forms a useful basis instead of a whole system,
interoperates with existing systems instead of replacing them.

## To do later

 * Make a file-like API atop raw blocks
 * Play with erasure codes: https://storj.io/blog/2018/11/replication-is-bad-for-decentralized-storage-part-1-erasure-codes-for-fun-and-profit/
 * Make a C-compatible FFI API
 * Might the `crossbeam` crate have some useful bits and pieces in it for async programming?  Probably, but not worth checking out yet.

## References

 * [Bittorrent DHT](http://www.bittorrent.org/beps/bep_0005.html) (this one is by far the most complete and useful)
 * [IPFS Whitepaper](https://github.com/ipfs/ipfs/blob/master/papers/ipfs-cap2pfs/ipfs-p2p-file-system.pdf)
 * [Kademila paper](http://pdos.csail.mit.edu/~petar/papers/maymounkov-kademlia-lncs.pdf)
 * [S/Kademila](http://www.spovnet.de/files/publications/SKademlia2007.pdf)
 * Coral DSHT?
