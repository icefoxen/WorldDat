//! Name is a bit in progress.
//! This is basically the bit that actually makes the decisions and sends messages
//! to/from the networking thread.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;

use failure::Error;

use crate::types::*;

/// A message that gets sent through a channel to
/// tell the PeerState thread to do something.
///
/// Obviates the need to have multiple channels.
enum ControlMessage {
    /// Stop the worker thread.
    Quit,
    /// Wake up and check to see if there's anything that needs doing.
    /// Crude but it'll work.
    Wake,
    /// A message was received from a peer!
    Incoming(SocketAddr, Message),
}

/// A struct that lets another thread manipulate a `PeerState` which
/// is running in its own thread...
#[derive(Debug)]
pub struct PeerStateHandle {
    control_sender: mpsc::Sender<ControlMessage>,
    thread_handle: thread::JoinHandle<()>,
}

impl PeerStateHandle {
    /// Tells the running `PeerState` thread to die
    /// and blocks until it does.
    ///
    /// Returns Err if the thread didn't exist or if
    /// there was some problem joining to it.
    pub fn quit(self) -> Result<(), Error> {
        self.control_sender.send(ControlMessage::Quit)?;
        self.thread_handle
            .join()
            .map_err(|e| format_err!("Error joining worker thread: {:?}", e))?;
        Ok(())
    }

    /// Returns a copy of the control channel sender.
    /// Crude but we can't clone the whole handle, irritatingly.
    pub fn controller(&self) -> PeerMessageHandle {
        PeerMessageHandle {
            control_sender: self.control_sender.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerMessageHandle {
    control_sender: mpsc::Sender<ControlMessage>,
}

impl PeerMessageHandle {
    /// Tell the worker thread a message has been recieved from the given source
    pub fn message(&self, source: SocketAddr, message: Message) -> Result<(), Error> {
        self.control_sender
            .send(ControlMessage::Incoming(source, message))
            .map_err(|e| format_err!("FIXME {:?}", e))
    }
}

pub struct PeerState {
    /// One channel per active connection.
    outgoing_messages_map: HashMap<SocketAddr, mpsc::Sender<Message>>,
    /// Receiving a message on this handle tells us to stop our main loop.
    control_receiver: mpsc::Receiver<ControlMessage>,
}

impl PeerState {
    /// Creates a new `PeerState` and runs it in its own thread,
    /// returns a handle to control it.
    pub fn start() -> PeerStateHandle {
        let (control_sender, control_receiver) = mpsc::channel();
        let state = Self {
            outgoing_messages_map: HashMap::new(),
            control_receiver,
        };
        let thread_handle = thread::spawn(|| state.run());
        let handle = PeerStateHandle {
            control_sender,
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
        loop {
            // TODO: Send occasional "wake up and do stuff if needed" messages.
            // `recv_timeout()` sucks more than advertised, it seems.
            match self.control_receiver.recv() {
                Ok(ControlMessage::Quit) => {
                    // Stop the loop.
                    break;
                }
                Ok(ControlMessage::Wake) => {
                    // Just continue and see if there's anything else
                    // we need to do...
                    ()
                }
                Ok(_) => {
                    // TODO: Whatever else.
                    info!("Got a message");
                    ()
                }
                Err(_) => {
                    // Sender is gone.
                    // We can never receive more messages, soooo, we're done!
                    break;
                }
            }
        }
    }
}
