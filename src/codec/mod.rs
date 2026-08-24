//! The value codec — protocol version 1.0.
//!
//! # This module's integers are not the frame layer's
//!
//! The protocol has **two** primitive sets and they disagree about signed
//! integers. Here, an `i64` is written big-endian **with the top bit of the
//! first byte inverted**; in [`crate::wire::frame`] every integer is plain
//! big-endian.
//!
//! The inversion exists because the node writes these values with a primitive
//! shared with an order-preserving key encoder, where a set sign bit would sort
//! negatives above positives. A payload does not need that ordering, but it
//! carries it, so a client reproduces the bytes rather than the rationale.
//!
//! The two sets live in two modules on purpose. One shared helper serving both
//! layers is the defect this arrangement exists to prevent, and it is a defect
//! that does not fail to compile: it returns wrong numbers.
//!
//! # Every value starts with its type
//!
//! An unknown tag is refused, never guessed at. A codec that infers a type from
//! what follows reads a newer format as a plausible wrong value, and nothing
//! downstream can tell.

pub mod read;
pub mod write;

pub use crate::codec::read::decode;
pub use crate::codec::write::encode;

/// The type tag that opens every value.
///
/// **Permanent.** A tag is never reused for a different type and never
/// renumbered; data already written carries them.
pub(crate) mod tag {
    pub(crate) const NONE: u8 = 0x01;
    pub(crate) const NULL: u8 = 0x02;
    pub(crate) const BOOL: u8 = 0x03;
    pub(crate) const NUMBER: u8 = 0x04;
    pub(crate) const STRING: u8 = 0x05;
    pub(crate) const BYTES: u8 = 0x06;
    pub(crate) const DURATION: u8 = 0x07;
    pub(crate) const DATETIME: u8 = 0x08;
    pub(crate) const UUID: u8 = 0x09;
    pub(crate) const TABLE: u8 = 0x0a;
    pub(crate) const RECORD: u8 = 0x0b;
    pub(crate) const ARRAY: u8 = 0x0c;
    pub(crate) const OBJECT: u8 = 0x0d;
    pub(crate) const RANGE: u8 = 0x0e;
    pub(crate) const SET: u8 = 0x0f;
    pub(crate) const GEOMETRY: u8 = 0x10;
    pub(crate) const REGEX: u8 = 0x11;
}

/// Which shape a geometry is.
///
/// Permanent for the reason the value tags are: a shape already written carries
/// this byte.
pub(crate) mod shape_kind {
    pub(crate) const POINT: u8 = 0x01;
    pub(crate) const LINE: u8 = 0x02;
    pub(crate) const POLYGON: u8 = 0x03;
    pub(crate) const MULTI_POINT: u8 = 0x04;
    pub(crate) const MULTI_LINE: u8 = 0x05;
    pub(crate) const MULTI_POLYGON: u8 = 0x06;
    pub(crate) const COLLECTION: u8 = 0x07;
}

/// Which of the three shapes a number takes.
pub(crate) mod number_kind {
    pub(crate) const INTEGER: u8 = 0x01;
    pub(crate) const FLOAT: u8 = 0x02;
    pub(crate) const DECIMAL: u8 = 0x03;
}

/// Which end a range bound is.
pub(crate) mod bound_kind {
    pub(crate) const UNBOUNDED: u8 = 0x01;
    pub(crate) const INCLUDED: u8 = 0x02;
    pub(crate) const EXCLUDED: u8 = 0x03;
}

/// The discriminant that opens a record identity.
pub(crate) mod record_id_kind {
    pub(crate) const INT: u8 = 0x01;
    pub(crate) const TEXT: u8 = 0x02;
    pub(crate) const UUID: u8 = 0x03;
    pub(crate) const BYTES: u8 = 0x04;
}

/// A zero byte, which opens both an escape and a terminator.
pub(crate) const ESCAPE: u8 = 0x00;
/// The second byte of a terminator.
pub(crate) const TERMINATOR: u8 = 0x01;
/// The second byte of an escaped zero.
pub(crate) const ESCAPED_ZERO: u8 = 0xFF;
