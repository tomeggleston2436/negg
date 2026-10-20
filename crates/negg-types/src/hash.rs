// SPDX-License-Identifier: MPL-2.0

//! Content hashing for settings.
//!
//! Settings are immutable and identified by a [`ContentHash`]: the SHA-256 of
//! a canonical byte stream produced by driving the value's `Serialize`
//! implementation into [`HashSerializer`]. The hash therefore changes
//! whenever any field value changes, and is stable across runs, platforms and
//! Rust versions. It does not need to be interoperable with anything else,
//! only deterministic. Maps are hashed in iteration order, so a hashed type
//! must not contain an unordered map or set (`HashMap`, `HashSet`); use a
//! `BTreeMap` or a sorted `Vec` instead.
//!
//! # Encoding
//!
//! Every `Serializer` method writes a one-byte type tag followed by its
//! payload:
//!
//! * `bool`: tag, then one byte (0 or 1).
//! * Integers: tag for the width and signedness, then the value as fixed-width
//!   little-endian bytes (`u128` and `i128` as 16 bytes).
//! * Floats: tag, then `to_bits()` as little-endian bytes, so `-0.0`, `0.0`
//!   and each NaN payload hash differently.
//! * `char`: tag, then the code point as a little-endian `u32`.
//! * Strings and byte slices: tag, `u64` length, then the bytes.
//! * `None`, `()` and unit structs: the tag alone (a unit struct also carries
//!   its name). `Some(v)`: tag, then `v`.
//! * Names (of structs, enums, variants and fields) are written as a `u64`
//!   length then the UTF-8 bytes. Enum variants are written as the enum
//!   name, the variant index as a `u32`, then the variant name.
//! * Sequences, tuples and maps: tag, a length prefix (one byte `1` then a
//!   `u64` when the length is known, or one byte `0`), the elements, then an
//!   end tag. Map keys and values each carry their own tag.
//! * Structs and struct variants: tag, name, `u64` field count, then for each
//!   field a field tag, the field name, and the value; then an end tag.
//!
//! The SHA-256 used is a plain FIPS 180-4 implementation kept in this file so
//! that the crate has no dependency beyond serde.

use std::fmt;

use serde::de::{self, Visitor};
use serde::ser::{
    self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant,
};

use crate::error::{Error, Result};

/// A 32-byte SHA-256 digest identifying one settings value.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// The raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The digest as 64 lowercase hexadecimal characters.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for byte in self.0 {
            s.push(HEX[(byte >> 4) as usize] as char);
            s.push(HEX[(byte & 0x0f) as usize] as char);
        }
        s
    }

    /// Parses 64 hexadecimal characters (either case) into a hash.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Hex`] if the text is not exactly 64 hexadecimal
    /// characters.
    pub fn from_hex(text: &str) -> Result<ContentHash> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return Err(Error::Hex(format!(
                "expected 64 hexadecimal characters, got {}",
                bytes.len()
            )));
        }
        let mut out = [0u8; 32];
        for (i, pair) in bytes.chunks_exact(2).enumerate() {
            let hi = hex_digit(pair[0])?;
            let lo = hex_digit(pair[1])?;
            out[i] = (hi << 4) | lo;
        }
        Ok(ContentHash(out))
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_digit(c: u8) -> Result<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(Error::Hex(format!(
            "{:?} is not a hexadecimal digit",
            c as char
        ))),
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.to_hex())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for ContentHash {
    fn deserialize<D: de::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct HexVisitor;

        impl Visitor<'_> for HexVisitor {
            type Value = ContentHash;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("64 hexadecimal characters")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<ContentHash, E> {
                ContentHash::from_hex(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(HexVisitor)
    }
}

/// Hashes any serialisable value with the canonical encoding described in
/// the module documentation.
///
/// # Panics
///
/// Panics if `value`'s `Serialize` implementation reports an error. The
/// serializer itself never fails, and derived implementations never raise
/// their own errors, so this only happens for a hand-written `Serialize`
/// that calls `Error::custom`.
pub fn hash_value<T: Serialize + ?Sized>(value: &T) -> ContentHash {
    let mut serializer = HashSerializer::new();
    value
        .serialize(&mut serializer)
        .expect("Serialize implementation reported an error while hashing");
    ContentHash(serializer.finish())
}

// Type tags. Each serializer method writes exactly one of these first.
const TAG_BOOL: u8 = 0x01;
const TAG_I8: u8 = 0x02;
const TAG_I16: u8 = 0x03;
const TAG_I32: u8 = 0x04;
const TAG_I64: u8 = 0x05;
const TAG_I128: u8 = 0x06;
const TAG_U8: u8 = 0x07;
const TAG_U16: u8 = 0x08;
const TAG_U32: u8 = 0x09;
const TAG_U64: u8 = 0x0a;
const TAG_U128: u8 = 0x0b;
const TAG_F32: u8 = 0x0c;
const TAG_F64: u8 = 0x0d;
const TAG_CHAR: u8 = 0x0e;
const TAG_STR: u8 = 0x0f;
const TAG_BYTES: u8 = 0x10;
const TAG_NONE: u8 = 0x11;
const TAG_SOME: u8 = 0x12;
const TAG_UNIT: u8 = 0x13;
const TAG_UNIT_STRUCT: u8 = 0x14;
const TAG_UNIT_VARIANT: u8 = 0x15;
const TAG_NEWTYPE_STRUCT: u8 = 0x16;
const TAG_NEWTYPE_VARIANT: u8 = 0x17;
const TAG_SEQ: u8 = 0x18;
const TAG_TUPLE: u8 = 0x19;
const TAG_TUPLE_STRUCT: u8 = 0x1a;
const TAG_TUPLE_VARIANT: u8 = 0x1b;
const TAG_MAP: u8 = 0x1c;
const TAG_MAP_KEY: u8 = 0x1d;
const TAG_MAP_VALUE: u8 = 0x1e;
const TAG_STRUCT: u8 = 0x1f;
const TAG_STRUCT_VARIANT: u8 = 0x20;
const TAG_FIELD: u8 = 0x21;
const TAG_END: u8 = 0x22;

/// Error type for [`HashSerializer`]. The serializer never produces one; it
/// exists only so that a `Serialize` implementation can call `custom`.
#[derive(Debug)]
struct HashError(String);

impl fmt::Display for HashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HashError {}

impl ser::Error for HashError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        HashError(msg.to_string())
    }
}

/// A `serde::Serializer` that feeds the canonical encoding into SHA-256.
/// Used only through [`hash_value`].
#[derive(Debug)]
struct HashSerializer {
    sha: Sha256,
}

impl HashSerializer {
    /// Starts a fresh digest.
    fn new() -> Self {
        HashSerializer { sha: Sha256::new() }
    }

    /// Finishes the digest, consuming the serializer.
    fn finish(self) -> [u8; 32] {
        self.sha.finalize()
    }

    fn tag(&mut self, tag: u8) {
        self.sha.update(&[tag]);
    }

    fn u64(&mut self, v: u64) {
        self.sha.update(&v.to_le_bytes());
    }

    fn name(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.sha.update(s.as_bytes());
    }

    fn len_prefix(&mut self, len: Option<usize>) {
        match len {
            Some(n) => {
                self.sha.update(&[1]);
                self.u64(n as u64);
            }
            None => self.sha.update(&[0]),
        }
    }

    fn variant(&mut self, name: &str, index: u32, variant: &str) {
        self.name(name);
        self.sha.update(&index.to_le_bytes());
        self.name(variant);
    }
}

type HashResult = std::result::Result<(), HashError>;

impl ser::Serializer for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> HashResult {
        self.tag(TAG_BOOL);
        self.sha.update(&[u8::from(v)]);
        Ok(())
    }

    fn serialize_i8(self, v: i8) -> HashResult {
        self.tag(TAG_I8);
        self.sha.update(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i16(self, v: i16) -> HashResult {
        self.tag(TAG_I16);
        self.sha.update(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i32(self, v: i32) -> HashResult {
        self.tag(TAG_I32);
        self.sha.update(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i64(self, v: i64) -> HashResult {
        self.tag(TAG_I64);
        self.sha.update(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i128(self, v: i128) -> HashResult {
        self.tag(TAG_I128);
        self.sha.update(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_u8(self, v: u8) -> HashResult {
        self.tag(TAG_U8);
        self.sha.update(&[v]);
        Ok(())
    }

    fn serialize_u16(self, v: u16) -> HashResult {
        self.tag(TAG_U16);
        self.sha.update(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_u32(self, v: u32) -> HashResult {
        self.tag(TAG_U32);
        self.sha.update(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_u64(self, v: u64) -> HashResult {
        self.tag(TAG_U64);
        self.u64(v);
        Ok(())
    }

    fn serialize_u128(self, v: u128) -> HashResult {
        self.tag(TAG_U128);
        self.sha.update(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_f32(self, v: f32) -> HashResult {
        self.tag(TAG_F32);
        self.sha.update(&v.to_bits().to_le_bytes());
        Ok(())
    }

    fn serialize_f64(self, v: f64) -> HashResult {
        self.tag(TAG_F64);
        self.sha.update(&v.to_bits().to_le_bytes());
        Ok(())
    }

    fn serialize_char(self, v: char) -> HashResult {
        self.tag(TAG_CHAR);
        self.sha.update(&u32::from(v).to_le_bytes());
        Ok(())
    }

    fn serialize_str(self, v: &str) -> HashResult {
        self.tag(TAG_STR);
        self.name(v);
        Ok(())
    }

    fn serialize_bytes(self, v: &[u8]) -> HashResult {
        self.tag(TAG_BYTES);
        self.u64(v.len() as u64);
        self.sha.update(v);
        Ok(())
    }

    fn serialize_none(self) -> HashResult {
        self.tag(TAG_NONE);
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> HashResult {
        self.tag(TAG_SOME);
        value.serialize(self)
    }

    fn serialize_unit(self) -> HashResult {
        self.tag(TAG_UNIT);
        Ok(())
    }

    fn serialize_unit_struct(self, name: &'static str) -> HashResult {
        self.tag(TAG_UNIT_STRUCT);
        self.name(name);
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> HashResult {
        self.tag(TAG_UNIT_VARIANT);
        self.variant(name, variant_index, variant);
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> HashResult {
        self.tag(TAG_NEWTYPE_STRUCT);
        self.name(name);
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> HashResult {
        self.tag(TAG_NEWTYPE_VARIANT);
        self.variant(name, variant_index, variant);
        value.serialize(self)
    }

    fn serialize_seq(self, len: Option<usize>) -> std::result::Result<Self, HashError> {
        self.tag(TAG_SEQ);
        self.len_prefix(len);
        Ok(self)
    }

    fn serialize_tuple(self, len: usize) -> std::result::Result<Self, HashError> {
        self.tag(TAG_TUPLE);
        self.len_prefix(Some(len));
        Ok(self)
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> std::result::Result<Self, HashError> {
        self.tag(TAG_TUPLE_STRUCT);
        self.name(name);
        self.len_prefix(Some(len));
        Ok(self)
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> std::result::Result<Self, HashError> {
        self.tag(TAG_TUPLE_VARIANT);
        self.variant(name, variant_index, variant);
        self.len_prefix(Some(len));
        Ok(self)
    }

    fn serialize_map(self, len: Option<usize>) -> std::result::Result<Self, HashError> {
        self.tag(TAG_MAP);
        self.len_prefix(len);
        Ok(self)
    }

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> std::result::Result<Self, HashError> {
        self.tag(TAG_STRUCT);
        self.name(name);
        self.len_prefix(Some(len));
        Ok(self)
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> std::result::Result<Self, HashError> {
        self.tag(TAG_STRUCT_VARIANT);
        self.variant(name, variant_index, variant);
        self.len_prefix(Some(len));
        Ok(self)
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

impl SerializeSeq for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }

    fn end(self) -> HashResult {
        self.tag(TAG_END);
        Ok(())
    }
}

impl SerializeTuple for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }

    fn end(self) -> HashResult {
        self.tag(TAG_END);
        Ok(())
    }
}

impl SerializeTupleStruct for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }

    fn end(self) -> HashResult {
        self.tag(TAG_END);
        Ok(())
    }
}

impl SerializeTupleVariant for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }

    fn end(self) -> HashResult {
        self.tag(TAG_END);
        Ok(())
    }
}

impl SerializeMap for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> HashResult {
        self.tag(TAG_MAP_KEY);
        key.serialize(&mut **self)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> HashResult {
        self.tag(TAG_MAP_VALUE);
        value.serialize(&mut **self)
    }

    fn end(self) -> HashResult {
        self.tag(TAG_END);
        Ok(())
    }
}

impl SerializeStruct for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> HashResult {
        self.tag(TAG_FIELD);
        self.name(key);
        value.serialize(&mut **self)
    }

    fn end(self) -> HashResult {
        self.tag(TAG_END);
        Ok(())
    }
}

impl SerializeStructVariant for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> HashResult {
        self.tag(TAG_FIELD);
        self.name(key);
        value.serialize(&mut **self)
    }

    fn end(self) -> HashResult {
        self.tag(TAG_END);
        Ok(())
    }
}

// SHA-256 (FIPS 180-4).

const SHA256_INIT: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

const SHA256_K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// Streaming SHA-256 state.
#[derive(Clone, Debug)]
struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffered: usize,
    /// Total bytes absorbed so far.
    length: u64,
}

impl Sha256 {
    fn new() -> Self {
        Sha256 {
            state: SHA256_INIT,
            buffer: [0; 64],
            buffered: 0,
            length: 0,
        }
    }

    fn update(&mut self, mut bytes: &[u8]) {
        self.length = self.length.wrapping_add(bytes.len() as u64);
        if self.buffered > 0 {
            let take = (64 - self.buffered).min(bytes.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&bytes[..take]);
            self.buffered += take;
            bytes = &bytes[take..];
            if self.buffered == 64 {
                let block = self.buffer;
                compress(&mut self.state, &block);
                self.buffered = 0;
            }
            if bytes.is_empty() {
                return;
            }
        }
        let mut blocks = bytes.chunks_exact(64);
        for block in &mut blocks {
            let block: &[u8; 64] = block.try_into().expect("chunks_exact yields 64 bytes");
            compress(&mut self.state, block);
        }
        let rest = blocks.remainder();
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_length = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buffered != 56 {
            self.update(&[0]);
        }
        self.update(&bit_length.to_be_bytes());
        debug_assert_eq!(self.buffered, 0);
        let mut out = [0u8; 32];
        for (chunk, word) in out.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

/// One SHA-256 compression round over a 64-byte block.
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (word, chunk) in w.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (k, wi) in SHA256_K.iter().zip(w) {
        let big_s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(big_s1)
            .wrapping_add(ch)
            .wrapping_add(*k)
            .wrapping_add(wi);
        let big_s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = big_s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}

/// SHA-256 of `bytes` in one shot. Hashing goes through [`HashSerializer`],
/// so this is only exercised by the test vectors at present.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(bytes);
    sha.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: [u8; 32]) -> String {
        ContentHash(bytes).to_hex()
    }

    #[test]
    fn sha256_empty() {
        assert_eq!(
            hex(sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_abc() {
        assert_eq!(
            hex(sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_two_blocks() {
        assert_eq!(
            hex(sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_million_a() {
        let input = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(sha256(&input)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn sha256_abc_one_byte_at_a_time() {
        let mut sha = Sha256::new();
        sha.update(b"a");
        sha.update(b"b");
        sha.update(b"c");
        assert_eq!(
            hex(sha.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_hundred_bytes_in_sevens() {
        let input: Vec<u8> = (0..100u8).collect();
        let mut sha = Sha256::new();
        for piece in input.chunks(7) {
            sha.update(piece);
        }
        assert_eq!(sha.finalize(), sha256(&input));
    }

    #[test]
    fn sha256_streaming_matches_one_shot() {
        let input: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let whole = sha256(&input);
        let mut sha = Sha256::new();
        for piece in input.chunks(13) {
            sha.update(piece);
        }
        assert_eq!(sha.finalize(), whole);
    }

    #[test]
    fn content_hash_hex_round_trip() {
        let h = ContentHash(sha256(b"abc"));
        let text = h.to_hex();
        assert_eq!(text.len(), 64);
        assert_eq!(h.to_string(), text);
        assert_eq!(ContentHash::from_hex(&text).unwrap(), h);
        assert_eq!(ContentHash::from_hex(&text.to_uppercase()).unwrap(), h);
        assert_eq!(h.as_bytes(), &sha256(b"abc"));
        assert_eq!(format!("{h:?}"), format!("ContentHash({text})"));
    }

    #[test]
    fn from_hex_rejects_bad_input() {
        let short = "0".repeat(63);
        assert!(matches!(ContentHash::from_hex(&short), Err(Error::Hex(_))));
        let bad = format!("{}g", "0".repeat(63));
        assert!(matches!(ContentHash::from_hex(&bad), Err(Error::Hex(_))));
    }

    #[test]
    fn content_hash_serde_as_hex_string() {
        let h = ContentHash(sha256(b"abc"));
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, format!("\"{}\"", h.to_hex()));
        let back: ContentHash = serde_json::from_str(&json).unwrap();
        assert_eq!(back, h);
        assert!(serde_json::from_str::<ContentHash>("\"abc\"").is_err());
        assert!(serde_json::from_str::<ContentHash>("12").is_err());
    }

    #[test]
    fn hash_value_is_deterministic_and_sensitive() {
        assert_eq!(hash_value(&1u32), hash_value(&1u32));
        assert_ne!(hash_value(&1u32), hash_value(&2u32));
        assert_ne!(hash_value(&1u32), hash_value(&1u64));
        assert_ne!(hash_value(&0.0f32), hash_value(&-0.0f32));
        assert_ne!(hash_value("ab"), hash_value(&("a", "b")));
        assert_ne!(hash_value(&Some(1u8)), hash_value(&1u8));
        assert_ne!(hash_value(&vec![1u8, 2]), hash_value(&[1u8, 2]));
    }
}
