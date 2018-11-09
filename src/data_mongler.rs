//! Name is a bit in progress.
//! This is basically the bit that actually makes the decisions and sends messages
//! to/from the networking thread.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use failure::Error;

use crate::types::*;

/// A struct that lets another thread manipulate a `PeerState` which
/// is running in its own thread...
pub struct PeerStateHandle {
    quit_channel: mpsc::Sender<()>,
    thread_handle: thread::JoinHandle<()>,
}

impl PeerStateHandle {
    /// Tells the running `PeerState` thread to die
    /// and blocks until it does.
    ///
    /// Returns Err if the thread didn't exist or if
    /// there was some problem joining to it.
    pub fn quit(self) -> Result<(), Error> {
        self.quit_channel.send(())?;
        self.thread_handle
            .join()
            .map_err(|e| format_err!("Error joining worker thread: {:?}", e))?;
        Ok(())
    }
}

pub struct PeerState {
    /// The stream of all messages we receive from all peers.
    incoming_messages: mpsc::Receiver<(SocketAddr, Message)>,
    /// One channel per active connection.
    outgoing_messages_map: HashMap<SocketAddr, mpsc::Sender<Message>>,
    /// Receiving a message on this handle tells us to stop our main loop.
    quit_channel: mpsc::Receiver<()>,
}

impl PeerState {
    /// Creates a new `PeerState` and runs it in its own thread,
    /// returns a handle to control it.
    pub fn start(message_channel: mpsc::Receiver<(SocketAddr, Message)>) -> PeerStateHandle {
        let (quit_sender, quit_receiver) = mpsc::channel();
        let state = Self {
            incoming_messages: message_channel,
            outgoing_messages_map: HashMap::new(),
            quit_channel: quit_receiver,
        };
        let thread_handle = thread::spawn(|| state.run());
        let handle = PeerStateHandle {
            quit_channel: quit_sender,
            thread_handle,
        };
        handle
    }

    pub fn _send(_dest: SocketAddr, _msg: Message) {
        // If socketaddr in map, send message through channel.
        // If channel doesn't accept it, then that connection
        // got closed and we have to re-open it.

        // If we don't have an open connection to that address,
        // either 'cause the connection closed or we never had one,
        // we have to ask the networking side of the protocol
        // to open one.
    }

    /// Run, forever.  You probably want to run this in a new thread.
    /// Will quit if a message is sent on the quit channel.
    /// Returns a `PeerStateHandle` which can be used to tell it to
    /// die.
    pub fn run(self) {
        let timeout = Duration::from_secs(1);
        loop {
            // There's no select() for mpsc channels.... so we kinda
            // just poll like a noob.  I'm fine with it for now.
            match self.incoming_messages.recv_timeout(timeout) {
                Ok(_msg) => {
                    // Do stuff with message
                    ()
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Sender is gone.
                    // We can never receive more messages, soooo, we're done!
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Shrug, no messages, stop and do other things before looping.
                    ()
                }
            }

            match self.quit_channel.try_recv() {
                Ok(()) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
    }
}
