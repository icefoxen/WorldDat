//! A pile of basic infrastructure stuff for dealing with hashes.  Whew!
//!
//! Would be 90% unnecessary if we had const generics, or just wimped out
//! and had them hold a Vec, but they should be cheap value types dammit!

use std::cmp::Ordering;
use std::fmt;
use std::ops;

use blake2;
use serde;

/// The number of bytes in a `Blake2Hash`, aka `Blake2b`
/// (the 512-bit version of blake2).
pub const BLAKE2_HASH_SIZE: usize = 64;

/// A hash uniquely identifying a peer or data type, barring hash collisions.
/// I'm not yet sure whether we want SHA256, SHA512 or something else
/// (Blake2?  SHA-3?) so for now we leave room for future expansion.
/// Expansion is hard though, since it means the address space and
/// such changes size, so we don't really want to upgrade if we can avoid it.
///
/// I am advised that the best general-purpose choice is currently Blake2,
/// since SHA3 is slow in software.
///
/// There's blake2s, which is 256 bits, and blake2b, which is 512.  We use
/// blake2b, 'cause I see no reason not to.
#[derive(Copy, Clone)]
pub struct Blake2Hash(pub [u8; BLAKE2_HASH_SIZE]);

impl fmt::Debug for Blake2Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "B{:?}", &self.0[..])
    }
}

impl PartialEq for Blake2Hash {
    fn eq(&self, other: &Self) -> bool {
        self.0[..] == other.0[..]
    }
}

impl Eq for Blake2Hash {}

impl PartialOrd for Blake2Hash {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Blake2Hash {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0[..].cmp(&other.0[..])
    }
}

impl ops::BitXor for Blake2Hash {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self {
        // We could probably optimize this but heck it.
        let mut out = [0; BLAKE2_HASH_SIZE];
        //for i in 0..self.0.len() {
        for (i, item) in out.iter_mut().enumerate().take(self.0.len()) {
            *item = self.0[i] ^ rhs.0[i];
        }
        Blake2Hash(out)
    }
}

/// An annoying struct for deserializing hashes.
struct Blake2HashVisitor;

impl<'a> serde::de::Visitor<'a> for Blake2HashVisitor {
    type Value = Blake2Hash;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "Expecting byte array.")
    }

    fn visit_bytes<E>(self, value: &[u8]) -> ::std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let mut out = [0; BLAKE2_HASH_SIZE];
        // TODO: This may panic?
        out[..].copy_from_slice(value);
        Ok(Blake2Hash(out))
    }
}

impl serde::Serialize for Blake2Hash {
    fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // self.0[..].serialize_bytes(serializer)
        serializer.serialize_bytes(&self.0[..])
    }
}

impl<'de> serde::Deserialize<'de> for Blake2Hash {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_bytes(Blake2HashVisitor)
    }
}

impl Blake2Hash {
    /// Create a new hash from the given seed data.
    pub fn new(seed: &[u8]) -> Self {
        use blake2::digest::{FixedOutput, Input};
        let mut hasher = blake2::Blake2b::default();
        hasher.input(seed);
        let res = hasher.fixed_result();
        let mut out = [0; BLAKE2_HASH_SIZE];
        out[..].copy_from_slice(res.as_slice());
        Blake2Hash(out)
    }

    /// Create a new `Blake2Hash` from a literal byte string.
    /// Panics if the input is not `BLAKE2_HASH_SIZE` bytes long.
    pub fn new_raw(bytes: &[u8]) -> Self {
        assert_eq!(bytes.len(), BLAKE2_HASH_SIZE);
        let mut out = [0; BLAKE2_HASH_SIZE];
        out[..].copy_from_slice(bytes);
        Blake2Hash(out)
    }

    /// The maximum power of 2 that the hash can hold.
    /// In this case, 512 for Blake2b.
    pub const fn max_power() -> usize {
        BLAKE2_HASH_SIZE * 8
    }

    pub fn to_base64(&self) -> String {
        base64::encode(&self.0[..])
    }

    /// Returns the number of leading zeros in the 512-bit
    /// number, minus one.
    /// If the answer would be -1, just returns 0.
    /// (The equivalent of doing log2(0) really).
    pub fn leading_zeros(&self) -> u32 {
        let mut res: u32 = Self::max_power() as u32;
        // I guess we store this kinda big-endian,
        // with the most-significant byte at array
        // index 0?  It doesn't really matter for us though.
        // To change that just reverse the slice.
        for (i, &itm) in self.0[..].iter().enumerate() {
            if itm == 0 {
                continue;
            } else {
                // The number of leading zeroes in the 512-byte number
                res = 8 * i as u32 + itm.leading_zeros();
            }
        }
        res.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_leading_zeroes() {
        let bytes = [0; BLAKE2_HASH_SIZE];
        let x = Blake2Hash::new_raw(&bytes[..]);
        assert_eq!(x.leading_zeros(), Blake2Hash::max_power() as u32 - 1);

        let bytes = [
            0x00, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let x = Blake2Hash::new_raw(&bytes[..]);
        assert_eq!(x.leading_zeros(), 7);

        let bytes = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF,
        ];
        let x = Blake2Hash::new_raw(&bytes[..]);
        assert_eq!(x.leading_zeros(), 503);

        let bytes = [
            0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let x = Blake2Hash::new_raw(&bytes[..]);
        assert_eq!(x.leading_zeros(), 22);
    }
}
