//! Deterministic hashing and canonical length-prefixed encoding.

use core::fmt::Write as _;

/// SHA-256 digest used for simulator state and event chains.
pub type Hash32 = [u8; 32];

/// All-zero sentinel used before the first chained event.
pub const ZERO_HASH: Hash32 = [0; 32];

// These values are the SHA-256 specification constants; keeping the published hexadecimal
// spelling makes auditing them against FIPS 180-4 substantially easier than digit-grouping them.
#[allow(clippy::unreadable_literal)]
const INITIAL: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

#[allow(clippy::unreadable_literal)]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Computes SHA-256 over exactly the supplied bytes.
///
/// # Panics
/// Panics only if an in-memory slice cannot be represented as the SHA-256 64-bit bit length.
/// That requires a slice larger than the addressable memory of supported 64-bit targets.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn sha256(input: &[u8]) -> Hash32 {
    let byte_len = u64::try_from(input.len()).expect("in-memory slice length exceeds u64");
    let bit_len = byte_len
        .checked_mul(8)
        .expect("SHA-256 input bit length exceeds u64");
    let mut padded = Vec::with_capacity(input.len().saturating_add(72));
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = INITIAL;
    for chunk in padded.as_chunks::<64>().0 {
        let mut words = [0_u32; 64];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let sigma1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sigma1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let sigma0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sigma0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut digest = [0_u8; 32];
    for (index, value) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    digest
}

/// Lowercase hexadecimal digest rendering for wire/debug output.
#[must_use]
pub fn hash_hex(hash: &Hash32) -> String {
    let mut output = String::with_capacity(64);
    for byte in hash {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

/// Canonical binary writer with fixed-width integers and length-prefixed byte strings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CanonicalWriter {
    bytes: Vec<u8>,
}

impl CanonicalWriter {
    /// Creates an empty canonical buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Appends a fixed domain-separation tag.
    pub fn tag(&mut self, tag: &[u8]) {
        self.bytes.extend_from_slice(tag);
    }

    /// Appends a big-endian unsigned 64-bit integer.
    pub fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// Appends a big-endian signed 64-bit integer.
    pub fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// Appends bytes prefixed by an unsigned 64-bit length.
    ///
    /// # Panics
    /// Panics only on platforms where an in-memory byte slice can exceed `u64::MAX` bytes.
    pub fn bytes(&mut self, value: &[u8]) {
        let length = u64::try_from(value.len()).expect("in-memory slice length exceeds u64");
        self.u64(length);
        self.bytes.extend_from_slice(value);
    }

    /// Appends UTF-8 text as length-prefixed bytes.
    pub fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    /// Appends a fixed 32-byte hash without a length prefix.
    pub fn hash(&mut self, value: &Hash32) {
        self.bytes.extend_from_slice(value);
    }

    /// Returns the completed canonical byte sequence.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_standard_vectors() {
        assert_eq!(
            hash_hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn canonical_writer_distinguishes_field_boundaries() {
        let mut left = CanonicalWriter::new();
        left.text("ab");
        left.text("c");
        let mut right = CanonicalWriter::new();
        right.text("a");
        right.text("bc");
        assert_ne!(left.finish(), right.finish());
    }
}
