//! Turning what a node answered into the types a caller declared.
//!
//! # The direction that was missing
//!
//! This crate has always converted **outward**: [`Value::from(21_i64)`] and its
//! siblings put a caller's data on the wire. Coming back, a caller received a
//! [`Value`] and matched their own way out of it — an [`Value::Object`], then a
//! lookup per field, then a [`Number`](crate::Number) inside a
//! [`Value::Number`]. That is three matches to read one integer, written again
//! at every call site, and each one is a place to get the null handling wrong.
//!
//! This module is the inward direction.
//!
//! # What it looks like
//!
//! ```no_run
//! # use tessaridb_client::{Answer, FromRecord, MappingFault, Row};
//! struct User {
//!     id: String,
//!     name: String,
//!     age: i64,
//!     nickname: Option<String>,
//! }
//!
//! impl FromRecord for User {
//!     fn from_row(mut row: Row) -> Result<Self, MappingFault> {
//!         Ok(Self {
//!             id: row.id().to_owned(),
//!             name: row.take("name")?,
//!             age: row.take("age")?,
//!             nickname: row.take("nickname")?,
//!         })
//!     }
//! }
//!
//! # fn use_it(answer: Answer) -> Result<(), MappingFault> {
//! let users: Vec<User> = answer.records_into()?;
//! # Ok(())
//! # }
//! ```
//!
//! # There is no derive macro, and that is a decision
//!
//! A derive would be a macro crate of this project's own to write, version and
//! keep working, bought to save a caller the ten lines above. The `impl` is
//! written by hand instead, and the compiler checks it against the struct rather
//! than against a string in an attribute — a misspelled field name is a
//! compile error at the line that misspelled it.
//!
//! `serde_json` is a dependency, for reading the node's HTTP answers, whose
//! strings carry arbitrary user text. It is used as a value tree: no
//! `Deserialize` implementations, no `serde_derive`, and so no `#[derive]` on
//! anything here. Having a JSON reader does not bring a derive, and it is the
//! derive this is a decision about.
//!
//! # There is no blanket implementation either
//!
//! [`FromRecord`] is deliberately **not** implemented as
//! `impl<T: FromValue> FromRecord for T`. Such an implementation covers every
//! type in every crate, including types this one has never heard of, and the
//! orphan rule then forbids a caller from writing `impl FromRecord for MyType`
//! at all — the blanket one already claims it. The convenience would be bought
//! by locking every user out of the trait, which is the opposite of the point.
//!
//! [`Value::from(21_i64)`]: crate::Value

mod fault;
mod impls;

use std::collections::BTreeMap;

use crate::value::Value;
use crate::wire::message::Answer;

pub use crate::mapping::fault::MappingFault;

/// A type a [`Value`] can become.
///
/// # Why by value
///
/// [`from_value`](FromValue::from_value) consumes the value rather than
/// borrowing it. A record is a map that is read once and taken apart, so the
/// owned form costs nothing and saves a clone of every string in every row —
/// which on a large result is the entire cost of mapping it.
pub trait FromValue: Sized {
    /// Convert a value into this type.
    ///
    /// # Errors
    ///
    /// [`MappingFault::WrongType`] when the value holds something else. The
    /// fault does **not** name the field, because a value does not know what it
    /// was called; [`Row::take`] attaches that.
    fn from_value(value: Value) -> Result<Self, MappingFault>;

    /// What this type makes of a field that is not in the record at all.
    ///
    /// The default is `None` — meaning *there is no answer*, so
    /// [`Row::take`] reports [`MappingFault::NoSuchField`] and the field is
    /// required. [`Option<T>`] overrides this to say that absence is an answer
    /// it can give.
    ///
    /// This exists because [`Row::take`] is generic and cannot look at `T` to
    /// decide whether a missing field is a failure. Asking `T` is the only way
    /// the question gets answered by the type that knows.
    #[must_use]
    fn absent() -> Option<Self> {
        None
    }
}

/// A type built from one whole record.
///
/// Implemented by hand for each of a caller's own types — see the module
/// documentation for why there is no derive macro and no blanket
/// implementation.
pub trait FromRecord: Sized {
    /// Build this type from a row.
    ///
    /// # Errors
    ///
    /// Whatever [`Row::take`] reports for the fields this type needs.
    fn from_row(row: Row) -> Result<Self, MappingFault>;
}

/// One record, opened up so its fields can be taken out by name.
///
/// # Taking rather than reading
///
/// [`take`](Row::take) removes the field it returns. That is what lets a row be
/// mapped without cloning: each field is moved into the caller's struct exactly
/// once. It also means a field cannot be read twice, which is a restriction
/// nobody has yet wanted and which keeps the type free of interior mutability
/// or a second borrowing method.
/// Only [`Debug`] is derived, and that is a starting position rather than an
/// oversight: adding a derive later is a compatible change and removing one is
/// not, so a published type begins with what is needed and grows on request.
#[derive(Debug)]
pub struct Row {
    id: String,
    fields: BTreeMap<String, Value>,
}

impl Row {
    /// Open a record — its identity as the node spells it, and its value.
    ///
    /// # Errors
    ///
    /// [`MappingFault::NotAnObject`] when the record does not hold an object,
    /// and therefore has no fields to address.
    pub fn new(id: String, value: Value) -> Result<Self, MappingFault> {
        match value {
            Value::Object(fields) => Ok(Self { id, fields }),
            other => Err(MappingFault::NotAnObject {
                found: fault::name_of(&other),
            }),
        }
    }

    /// The record's identity, as the node spelled it.
    ///
    /// Borrowed rather than taken: an identity is usually copied into a struct
    /// field *and* used to build a message, and a method that consumed it would
    /// make the second use a clone the caller had to write.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Take one field out of the record, converted.
    ///
    /// The target type is inferred from where the result is going, so a struct
    /// field's own declaration decides what the value must be.
    ///
    /// # Errors
    ///
    /// - [`MappingFault::NoSuchField`] when the record has no such field and
    ///   `T` does not accept absence. An [`Option<T>`] does accept it.
    /// - [`MappingFault::InField`] wrapping whatever the conversion objected
    ///   to, so the message names the field as well as the disagreement.
    pub fn take<T: FromValue>(&mut self, field: &str) -> Result<T, MappingFault> {
        match self.fields.remove(field) {
            Some(value) => T::from_value(value).map_err(|cause| cause.in_field(field)),
            None => T::absent().ok_or_else(|| MappingFault::NoSuchField {
                name: field.to_owned(),
            }),
        }
    }
}

impl Answer {
    /// Map every record in this answer into `T`.
    ///
    /// # Why this lives in the mapping module
    ///
    /// [`Answer`] belongs to the wire, and the wire's job is finished when the
    /// bytes have become values. Mapping is a layer a caller may never use, and
    /// keeping its only entry point here means `wire` carries no knowledge of
    /// it.
    ///
    /// # Errors
    ///
    /// - [`MappingFault::NotRecords`] when the answer is not
    ///   [`Answer::Records`] — a `DEFINE` answers [`Answer::Done`], and asking
    ///   it for rows is a mistake about which statement ran. Reported rather
    ///   than answered with an empty list, which would be indistinguishable
    ///   from a query that matched nothing.
    /// - Whatever the first record fails on, with its field named. The mapping
    ///   stops there rather than collecting every fault: a mismatch is nearly
    ///   always the same mismatch in every row, and a hundred copies of one
    ///   message is not a better report.
    pub fn records_into<T: FromRecord>(self) -> Result<Vec<T>, MappingFault> {
        match self {
            Self::Records { records, .. } => records
                .into_iter()
                .map(|(id, value)| T::from_row(Row::new(id, value)?))
                .collect(),
            _ => Err(MappingFault::NotRecords),
        }
    }
}
