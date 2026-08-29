//! Why a value would not become the type a caller asked for.

use crate::value::{Number, Value};

/// A value did not become the type a caller asked for.
///
/// # Why this is a third enum and not a variant on an existing one
///
/// This crate already keeps [`Error`](crate::Error) and
/// [`EncodingFault`](crate::EncodingFault) apart, so that a byte-level fault
/// names its cause without the transport enum growing a variant per tag. A
/// mapping fault is a third thing again: nothing failed on the network and
/// nothing failed in the codec — the bytes arrived, decoded, and mean exactly
/// what they say. What failed is the caller's expectation about a field, which
/// is a fault in the program rather than in the conversation.
///
/// Folding it into either of the others would tell an operator to look at a
/// network or a codec, where there is nothing to find.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MappingFault {
    /// The value is not of the type the target needs.
    ///
    /// Both names are what a person calls the type, not a Rust path: the caller
    /// reading this is being told about their data, and `alloc::string::String`
    /// is a worse answer than `text`.
    #[error("expected {expected}, found {found}")]
    WrongType {
        /// What the conversion needed.
        expected: &'static str,
        /// What was actually there.
        found: &'static str,
    },

    /// The record has no field by that name.
    ///
    /// Distinct from a field that is present and holds nothing: a missing key is
    /// a disagreement about the shape of the record, and a null is a fact about
    /// one. Collapsing them would let a misspelled field name read as an absent
    /// value and map cleanly into `None`.
    #[error("no field named `{name}`")]
    NoSuchField {
        /// The name that was asked for.
        name: String,
    },

    /// A fault inside a named field.
    ///
    /// [`FromValue`](crate::FromValue) converts a value and never learns what it
    /// was called, which is what keeps it usable on a bare value. The name is
    /// attached here instead, by whoever knew it.
    #[error("in field `{name}`: {cause}")]
    InField {
        /// Which field.
        name: String,
        /// What went wrong inside it.
        #[source]
        cause: Box<MappingFault>,
    },

    /// A fault inside one element of a sequence.
    ///
    /// Separate from [`MappingFault::InField`] so that a bad value deep in a
    /// long array names its position. "expected a whole number, found text"
    /// against a thousand-element field is a search; against element 617 it is
    /// a fix.
    #[error("at element {index}: {cause}")]
    AtElement {
        /// Which element.
        index: usize,
        /// What went wrong inside it.
        #[source]
        cause: Box<MappingFault>,
    },

    /// The answer carried something other than records.
    ///
    /// A `CREATE` answers with records and a `DEFINE` answers [`Answer::Done`];
    /// asking the second for rows is a mistake about which statement ran, and it
    /// is reported rather than answered with an empty list, which would look
    /// like a query that found nothing.
    ///
    /// [`Answer::Done`]: crate::Answer::Done
    #[error("that answer holds no records")]
    NotRecords,

    /// A record's value is not an object, so it has no fields to address.
    #[error("a record holds {found}, which has no fields")]
    NotAnObject {
        /// What the record held instead.
        found: &'static str,
    },
}

impl MappingFault {
    /// Attach a field name to this fault.
    pub(crate) fn in_field(self, name: &str) -> Self {
        Self::InField {
            name: name.to_owned(),
            cause: Box::new(self),
        }
    }

    /// Attach a sequence position to this fault.
    pub(crate) fn in_element(self, index: usize) -> Self {
        Self::AtElement {
            index,
            cause: Box::new(self),
        }
    }
}

/// What to call a value in a message meant for a person.
///
/// # These are the store's names, not Rust's
///
/// A caller who sees `expected a whole number, found text` is being told
/// something about their record. `expected i64, found alloc::string::String`
/// tells them about this crate's implementation, which is not what they were
/// asking about.
///
/// [`Value::None`] and [`Value::Null`] get **different** names here, because the
/// distinction is one the store keeps deliberately and a caller debugging a
/// mapping is exactly the person who needs to see it.
///
/// # Every variant is listed, and there is no catch-all
///
/// [`Value`] is `#[non_exhaustive]` for the crates that consume it, but inside
/// the crate that defines it the compiler still demands every arm. That is left
/// as it is rather than shortened with a `_`: adding a value type then fails to
/// compile *here*, and whoever adds it has to say what it is called. A catch-all
/// would instead ship the new type under a stale name, which is the kind of
/// wrongness that reads as correct.
pub(crate) fn name_of(value: &Value) -> &'static str {
    match value {
        Value::None => "an absent field",
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(Number::Integer(_)) => "a whole number",
        Value::Number(Number::Float(_)) => "a floating-point number",
        Value::Number(Number::Decimal { .. }) => "an exact decimal",
        Value::String(_) => "text",
        Value::Bytes(_) => "bytes",
        Value::Duration { .. } => "a duration",
        Value::Datetime { .. } => "a datetime",
        Value::Uuid(_) => "a uuid",
        Value::Table(_) => "a table reference",
        Value::Record(_) => "a record reference",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
        Value::Range(_) => "a range",
        Value::Set(_) => "a set",
        Value::Geometry(_) => "a geometry",
        Value::Regex(_) => "a pattern",
    }
}
