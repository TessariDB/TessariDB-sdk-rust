//! What a [`Value`] converts into.
//!
//! # The set is deliberately the mirror of the outward one
//!
//! [`value`](crate::value) offers `From<i64>`, `From<f64>`, `From<bool>`,
//! `From<String>`, `From<&str>` and `From<Vec<u8>>` for [`Value`]. The
//! conversions here are those same types coming back, plus the three that only
//! make sense inward: [`Value`] itself, [`Option<T>`] and [`Vec<T>`].
//!
//! That boundary is a principle rather than a stopping point chosen at random.
//! A store type with no natural Rust counterpart — a duration, a datetime, a
//! geometry — is one where the *caller's* choice of crate decides the target
//! type, and this crate has no business picking `chrono` over `time` on their
//! behalf. Those values are taken as [`Value`] and converted by whoever knows.

use crate::mapping::FromValue;
use crate::mapping::fault::{MappingFault, name_of};
use crate::value::{Number, Value};

impl FromValue for Value {
    /// Take a field without converting it.
    ///
    /// The escape hatch: a store type this module has no Rust counterpart for
    /// is still reachable, and a caller who wants to match it themselves is not
    /// forced around the mapping to do so.
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        Ok(value)
    }
}

impl FromValue for bool {
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        match value {
            Value::Bool(held) => Ok(held),
            other => Err(MappingFault::WrongType {
                expected: "a boolean",
                found: name_of(&other),
            }),
        }
    }
}

impl FromValue for i64 {
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        match value {
            Value::Number(Number::Integer(held)) => Ok(held),
            other => Err(MappingFault::WrongType {
                expected: "a whole number",
                found: name_of(&other),
            }),
        }
    }
}

impl FromValue for f64 {
    /// # A whole number is not accepted here, and that is on purpose
    ///
    /// A store integer is 64 bits and an `f64` carries 53 of them exactly, so
    /// widening one into the other is lossy above roughly nine quadrillion —
    /// silently, and in the direction of a plausible wrong answer rather than an
    /// error. This crate denies `cast_possible_truncation` and
    /// `arithmetic_side_effects` for the same class of reason, and a conversion
    /// that quietly did it here would be the one place the rule bent.
    ///
    /// A field that is sometimes whole and sometimes not is taken as
    /// [`Number`] or as [`Value`] and decided by the caller, who knows the range
    /// their data actually occupies.
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        match value {
            Value::Number(Number::Float(held)) => Ok(held),
            other => Err(MappingFault::WrongType {
                expected: "a floating-point number",
                found: name_of(&other),
            }),
        }
    }
}

impl FromValue for Number {
    /// Any of the three shapes a number comes in, undecided.
    ///
    /// The target for a numeric field whose shape the caller would rather
    /// inspect than assert — which is the honest answer for money, for anything
    /// summed from a mixed column, and for a schema still moving.
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        match value {
            Value::Number(held) => Ok(held),
            other => Err(MappingFault::WrongType {
                expected: "a number",
                found: name_of(&other),
            }),
        }
    }
}

impl FromValue for String {
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        match value {
            Value::String(held) => Ok(held),
            other => Err(MappingFault::WrongType {
                expected: "text",
                found: name_of(&other),
            }),
        }
    }
}

impl FromValue for Vec<u8> {
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        match value {
            Value::Bytes(held) => Ok(held),
            other => Err(MappingFault::WrongType {
                expected: "bytes",
                found: name_of(&other),
            }),
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    /// # Three ways a field can hold nothing, and they are not all the same
    ///
    /// [`Value::None`] says the field is absent and [`Value::Null`] says it is
    /// present and holds nothing — a distinction the store keeps deliberately.
    /// Both become `None` here, because a caller writing `Option<T>` has asked
    /// one question, *is there a value*, and both answer it the same way.
    ///
    /// The third way is a field that is not in the record at all, which
    /// [`from_value`](FromValue::from_value) never sees. That is handled by
    /// [`absent`](FromValue::absent) below.
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        match value {
            Value::None | Value::Null => Ok(None),
            held => T::from_value(held).map(Some),
        }
    }

    /// A field the record does not carry is `None`, not a failure.
    ///
    /// This is what makes `Option<T>` mean *optional* rather than merely
    /// *nullable*. Without it a caller could express "this field may be null"
    /// but not "this field may not be there", and a store that omits empty
    /// fields would make every optional field a fault.
    fn absent() -> Option<Self> {
        Some(None)
    }
}

impl<T: FromValue> FromValue for Vec<T> {
    /// # A set converts too
    ///
    /// [`Value::Array`] is ordered and [`Value::Set`] is not, and both arrive as
    /// a sequence. Refusing the set would mean a caller could not read one at
    /// all without dropping to [`Value`], to preserve an ordering guarantee that
    /// a `Vec` does not make in the first place.
    ///
    /// # Errors
    ///
    /// A fault inside an element is reported with the element's index, so a bad
    /// value in a long array names its position rather than the whole field.
    fn from_value(value: Value) -> Result<Self, MappingFault> {
        let elements = match value {
            Value::Array(held) | Value::Set(held) => held,
            other => {
                return Err(MappingFault::WrongType {
                    expected: "an array",
                    found: name_of(&other),
                });
            }
        };
        elements
            .into_iter()
            .enumerate()
            .map(|(index, element)| T::from_value(element).map_err(|cause| cause.in_element(index)))
            .collect()
    }
}
