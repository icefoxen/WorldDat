
# WorldDat

Build a system that does something like what IPFS does, but simpler and
easier to integrate in other programs.

Basically, a content-addressed block store with a Kademila-like DHT to search for and distribute blocks.

Inspirations: ipfs, BitTorrent, dat, secure scuttlebutt, ...

To build (on Debian):

 * Install `nix` from <https://nixos.org/nix/>
 * Run `nix-shell` in the root directory of the project.  That will build and patch the right version of OpenSSL if necessary and start a shell with it available.
  * If you've installed nix without a debian package or such it may just update your ~/.bash_profile; you might need to twiddle env vars
    if you're using fish or such.
 * `cargo build`


## To do

Make a more proper comparison to IPFS: fewer layers of abstraction, easier/simpler protocol, client-only
implementations, easier ffi for official implementation. Generally just tries to move blocks
around instead of doing Everything. Forms a useful basis instead of a whole system,
interoperates with existing systems instead of replacing them.

## Goals

simple to use, composable, robust, swift, easy to figure out and debug and incorporate
into other systems, quite lightweight, data is tamperproof (censorproof or monitoring-proof
is harder), transmission encrypted by default should help tho.

Non-goals: permanent, anonymous, the fastest evar, does everything all on its own, changes teh world

## Notes

Having data encrypted when stored on the nodes with the key contained in the lookup hash sounds very nice but also should not be the job of this layer; higher-level programs can do it.  If data is intentionally public (such as a funny cat image) then you'd need to include the key with every link you provide to it, and if you had a block but didn't know the key for it you could just heckin' google it and someone would have it written down somewhere.  Having crypto be part of a higher-level file-like abstraction is probably nicer anyway 'cause you can have the key be per file instead of per block.

I think that this particular system will *stop* at the level of retrieving blocks, and let other systems build higher-level abstractions atop it.

You should be able to easily specify particular hosts to try to seek a block from, both by
ip/dns or by node hash. Hopefully helps the bootstrapping process some.  Hosts can't lie to you about the contents of a block, so this is Safe.

Example of something which *can not* be stored by content-addressed merkle-dag structures like this: Anything with loops!  For example, a heckin' webcomic archive.  You want forward and back links on each page of your archive.  If you embed them in the page then it changes the hash of the page and breaks the links of other pages pointing to it.  You *can not* represent loops between unassociated objects.  IPFS allows you to have "directories" of associated objects which can then refer to each other by non-content-addressed name, but...
