//! Useful types used throughout the program, I suppose.

/// The actual serializable messages that can be sent back and forth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    Ping {},
    Pong {},
    Raw { data: Box<[u8]> },
}
