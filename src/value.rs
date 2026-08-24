//! The fifteen types a node carries.
//!
//! # Why this is fifteen types and not JSON's six
//!
//! The wire protocol exists so that a decimal stays a decimal, a duration stays
//! a duration, and a record reference stays a reference. Every one of them
//! survives a round trip unchanged, and none of them has to be recovered from
//! text by guessing.
//!
//! # No arithmetic or time dependency
//!
//! A decimal is carried as the two numbers that *define* it — an unscaled
//! mantissa and a count of fractional digits — and a moment as seconds plus
//! nanoseconds. That is what the protocol puts on the wire, so this crate
//! reproduces it directly rather than pulling in an arithmetic library or a
//! calendar. A caller that wants `rust_decimal` or `chrono` converts at its own
//! edge, and a caller that wants neither pays for neither.

use std::collections::BTreeMap;
use std::ops::Bound;

/// A value, as a node carries it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// The field is not present.
    ///
    /// Distinct from [`Value::Null`], and the distinction is the point: one says
    /// the field is absent, the other that it is present and holds nothing.
    None,
    /// The field is present and holds nothing.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number — integer, float, or exact decimal.
    Number(Number),
    /// Text.
    String(String),
    /// Opaque bytes.
    Bytes(Vec<u8>),
    /// A span of time, which may be negative.
    Duration {
        /// Whole seconds; negative for a span running backwards.
        seconds: i64,
        /// The sub-second part, in nanoseconds.
        nanos: u32,
    },
    /// A point in time, as seconds and nanoseconds from the epoch.
    Datetime {
        /// Whole seconds from the epoch; negative before it.
        seconds: i64,
        /// The sub-second part, in nanoseconds.
        nanos: u32,
    },
    /// A universally unique identifier, as its sixteen bytes.
    Uuid([u8; 16]),
    /// A reference to a table, by the id the node minted.
    Table(u32),
    /// A reference to one record.
    Record(RecordRef),
    /// An ordered sequence.
    Array(Vec<Value>),
    /// A map from field name to value.
    ///
    /// Held name-ordered, which makes two equal objects encode to equal bytes.
    /// The node re-normalises on decode either way, so this is a convenience
    /// here rather than a protocol requirement.
    Object(BTreeMap<String, Value>),
    /// A span between two values.
    ///
    /// Boxed because a range holds values and a value may be a range; without
    /// the box the type would have no finite size.
    Range(Box<ValueRange>),
    /// A collection with no duplicates and no significant order.
    ///
    /// Carried as a sequence rather than an ordered set: the node normalises it
    /// on decode, so a client is not obliged to implement a total order across
    /// mixed types merely to send one.
    Set(Vec<Value>),
}

/// A number, in one of the three shapes the store keeps.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Number {
    /// A 64-bit signed integer.
    Integer(i64),
    /// A double.
    Float(f64),
    /// An exact decimal, as the two numbers that define it.
    ///
    /// The value is `mantissa / 10^scale`. Carried this way rather than as an
    /// arithmetic library's in-memory layout, so that upgrading such a library
    /// is not silently a data migration.
    Decimal {
        /// The unscaled value.
        mantissa: i128,
        /// How many of its digits are fractional.
        scale: u32,
    },
}

/// The identity of one record.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordId {
    /// An integer identity.
    Int(i64),
    /// A textual identity.
    Text(String),
    /// A UUID identity, as its sixteen bytes.
    Uuid([u8; 16]),
    /// An opaque byte identity.
    Bytes(Vec<u8>),
}

/// A reference to one record in one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRef {
    /// The table, by the id the node minted.
    ///
    /// An id is meaningless outside the node that minted it, which is why an
    /// answer that carries references also carries the names — see
    /// [`crate::wire::message::Answer`].
    pub table: u32,
    /// Which record.
    pub id: RecordId,
}

impl RecordRef {
    /// A reference to `id` in `table`.
    #[must_use]
    pub const fn new(table: u32, id: RecordId) -> Self {
        Self { table, id }
    }
}

/// A span between two values, either end of which may be open.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueRange {
    /// Where the span starts.
    pub start: Bound<Value>,
    /// Where it ends.
    pub end: Bound<Value>,
}

impl ValueRange {
    /// A span from `start` to `end`.
    #[must_use]
    pub const fn new(start: Bound<Value>, end: Bound<Value>) -> Self {
        Self { start, end }
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Number(Number::Integer(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Number(Number::Float(value))
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}
