use crate::PeerOpt;
use failure::Error;

pub struct Peer {}

impl Peer {
    pub fn new(opt: PeerOpt) -> Self {
        Peer {}
    }

    pub fn run(&mut self) -> Result<(), Error> {
        Ok(())
    }
}
