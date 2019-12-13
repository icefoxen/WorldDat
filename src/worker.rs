//! Name is a bit in progress.
//! This is basically the bit that actually makes the decisions and sends messages
//! to/from the networking thread.

use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;

use log::*;

use crate::types::*;

/// A message that gets sent through a channel to
/// tell the PeerState thread to do something.
///
/// Obviates the need to have multiple channels.
#[derive(Debug, Clone)]
pub enum WorkerMessage {
    /// Stop the worker thread.
    Quit,
    // /// Wake up and check to see if there's anything that needs doing.
    // /// Crude but it'll work.
    // Wake,
    /// A message was received from a peer!
    Incoming(SocketAddr, Message),
}

/// A struct that lets another thread manipulate a `PeerState` which
/// is running in its own thread...
#[derive(Debug)]
pub struct WorkerHandle {
    peer_id: PeerId,
    control_sender: mpsc::Sender<WorkerMessage>,
    message_receiver: mpsc::Receiver<(SocketAddr, Message)>,
    thread_handle: thread::JoinHandle<()>,
}

impl WorkerHandle {
    /// Tells the running `PeerState` thread to die
    /// and blocks until it does.
    ///
    /// Returns Err if the thread didn't exist or if
    /// there was some problem joining to it.
    pub fn quit(self) -> Result<(), std::sync::mpsc::SendError<WorkerMessage>> {
        self.control_sender.send(WorkerMessage::Quit)?;
        self.thread_handle.join().expect("TODO");
        //.map_err(|e| format_err!("Error joining worker thread: {:?}", e))?;
        Ok(())
    }

    /// Pulls a message that the worker wants sent off the queue and returns it.
    /// Does not block.
    #[allow(dead_code)]
    pub fn recv_message(&self) -> Result<(SocketAddr, Message), std::sync::mpsc::TryRecvError> {
        self.message_receiver.try_recv()
    }

    /// Returns a copy of the control channel sender.
    /// Crude but we can't clone the whole handle, irritatingly.
    pub fn controller(&self) -> WorkerMessageHandle {
        WorkerMessageHandle {
            control_sender: self.control_sender.clone(),
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }
}

/// A handle to a worker that can only send messages to it.
/// This is kinda inconvenient but this can be cloned and moved
/// to different threads, and `WorkerHandle` can't be cloned
/// 'cause it contains a `thread::JoinHandle`.  Alas.
#[derive(Debug, Clone)]
pub struct WorkerMessageHandle {
    control_sender: mpsc::Sender<WorkerMessage>,
}

impl WorkerMessageHandle {
    /// Tell the worker thread a message has been recieved from the given source
    pub fn message(
        &self,
        source: SocketAddr,
        message: Message,
    ) -> Result<(), std::sync::mpsc::SendError<WorkerMessage>> {
        self.control_sender
            .send(WorkerMessage::Incoming(source, message))
    }
}

pub struct WorkerState {
    /// One channel per active connection.
    message_sender: mpsc::Sender<(SocketAddr, Message)>,
    /// Receiving a message on this handle tells us to stop our main loop.
    control_receiver: mpsc::Receiver<WorkerMessage>,

    // Stuff that has to do with actually making decisions instead of
    // communicating with other threads.
    /// The ID of the peer.
    peer_id: PeerId,
    peer_map: PeerMap,
}

impl WorkerState {
    /// Creates a new `PeerState` and runs it in its own thread,
    /// returns a handle to control it.
    pub fn start(peer_id: PeerId, addr: SocketAddr) -> WorkerHandle {
        let (control_sender, control_receiver) = mpsc::channel();
        let (message_sender, message_receiver) = mpsc::channel();
        let mut peer_map = PeerMap::default();
        // Add self to peer map
        peer_map.insert(peer_id, addr, peer_id);

        let state = WorkerState {
            message_sender,
            control_receiver,

            peer_id,
            peer_map,
        };
        let thread_handle = thread::spawn(|| state.run());
        WorkerHandle {
            message_receiver,
            control_sender,
            thread_handle,
            peer_id,
        }
    }

    pub fn send(&self, dest: SocketAddr, msg: Message) {
        self.message_sender.send((dest, msg)).expect("FIXME")
    }

    /// Run, forever.  You probably want to run this in a new thread.
    /// Will quit if a message is sent on the quit channel.
    /// Returns a `PeerStateHandle` which can be used to tell it to
    /// die.
    pub fn run(mut self) {
        loop {
            // TODO: Send occasional "wake up and do stuff if needed" messages.
            // `recv_timeout()` sucks more than advertised, it seems.
            match self.control_receiver.recv() {
                Ok(WorkerMessage::Quit) => {
                    // Stop the loop.
                    info!("Worker got quit message, quitting...");
                    break;
                }
                // Ok(WorkerMessage::Wake) => {
                //     // Just continue and see if there's anything else
                //     // we need to do...
                //     debug!("Worker woke up, anything to do?");
                //     ()
                // }
                Ok(WorkerMessage::Incoming(addr, msg)) => {
                    // TODO: Whatever else.
                    info!(
                        "Incoming message to {:?} from {}: {:?}",
                        self.peer_id, addr, msg
                    );
                    match msg {
                        Message::Ping { id: other_id } => {
                            debug!("Ping from {}", addr);
                            self.send(addr, Message::Pong { id: self.peer_id });

                            // If we don't know about this peer,
                            // ping the thing back to see if it's actually legit.
                            // Eventually maybe we want timeouts involved here, or such?
                            // Maybe not, that should happen on its own time.
                            if self.peer_map.contains(self.peer_id, other_id).is_none() {
                                self.send(addr, Message::Ping { id: self.peer_id });
                            }
                        }
                        Message::Pong { id: other_id } => {
                            // Pong ONLY happens in response to a ping, so we know
                            // this ID is legit.
                            // TODO: Make this actually unforgable; there's a few ways to do this,
                            // Bittorrent does it for example
                            self.peer_map.insert(self.peer_id, addr, other_id)
                        }
                        Message::FindPeer { id: desired_id } => {
                            // Do we know about the peer?
                            let reply = match self.peer_map.lookup(self.peer_id, desired_id) {
                                Ok((peer_id, socket_addr)) => Message::FindPeerResponsePeerFound {
                                    id: peer_id,
                                    addr: socket_addr,
                                },
                                Err(neighbors) => Message::FindPeerResponsePeerNotFound {
                                    id: desired_id,
                                    neighbors,
                                },
                            };
                            self.send(addr, reply)
                        }
                        _ => (),
                    }
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
