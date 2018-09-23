Playing with Ralith's quicr crate.

The goal is to build a system that does something like what IPFS does, but simpler and
easier to integrate in other programs.

Inspirations: List ipfs, BitTorrent and dat as inspiration. Oh, and ssb

To build (on Debian):

 * Install `nix`
 * Run `nix-shell` in this directory.  That will build and patch the right version of OpenSSL if necessary and start a shell with it
   available.
  * If you've installed nix without a debian package or such it may just update your ~/.bash_profile; you might need to twiddle env vars
    if you're using fish or such.
 * `cargo build`


To do:

Settle on a name; worlddat, worlddata, worldblock

Comparison to IPFS: fewer layers of abstraction, easier/simpler protocol, client-only
implementations, easier ffi for official implementation. Generally just tries to move blocks
around instead of doing Everything. Forms a useful basis instead of a whole system,
interoperates with existing systems instead of replacing them.


Goals: simple to use, composable, robust, swift, easy to figure out and debug nd incorporate
into other systems, quite lightweight, data is tamperproof (censorproof or monitoring-proof
is harder), encrypted by default should help tho.

Non-goals: permanent, anonymous, the fastest evar, 

You should be able to easily specify particular hosts to try to seek a block from, both by
ip/dns or by node hash. Hopefully helps the bootstrapping process some.
