//! Turning values into declared types.
//!
//! # These are the weaker tier, and they know it
//!
//! Everything here builds a [`Value`] in this process and converts it in the
//! same process. That proves the conversions are self-consistent, which is worth
//! having and is not evidence that a node ever sends the shapes being converted.
//! The stronger tier is in `node.rs`, where the values come off a real socket
//! (LR-SDK-006).
//!
//! What these tests are actually for is the part the node cannot show cheaply:
//! the **failure** surface. A wrong type, a missing field, a null where one is
//! not allowed, a bad element deep in an array — each has to be provoked
//! deliberately, and doing it against a live store would mean writing bad data
//! on purpose to read it back.

// Test assertions are exactly where a panic is the correct outcome; the lints
// these turn off target production paths, where a panic is a defect.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use tessaridb_client::{Answer, FromRecord, FromValue, MappingFault, Number, Row, Value};

/// The struct the mapping is aimed at, kept deliberately ordinary: a required
/// string, a required integer, an optional string and a list.
#[derive(Debug, PartialEq)]
struct User {
    id: String,
    name: String,
    age: i64,
    nickname: Option<String>,
    tags: Vec<String>,
}

impl FromRecord for User {
    fn from_row(mut row: Row) -> Result<Self, MappingFault> {
        Ok(Self {
            id: row.id().to_owned(),
            name: row.take("name")?,
            age: row.take("age")?,
            nickname: row.take("nickname")?,
            tags: row.take("tags")?,
        })
    }
}

fn object(fields: &[(&str, Value)]) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn row(fields: &[(&str, Value)]) -> Row {
    Row::new("1".to_owned(), object(fields)).expect("an object is a row")
}

#[test]
fn a_record_becomes_the_struct_it_was_declared_as() {
    // Through `FromRecord` directly rather than through an `Answer`:
    // `Answer::Records` is a `#[non_exhaustive]` variant and cannot be built
    // from outside the crate, which is deliberate — the frame grows by
    // appending, so nobody outside gets to depend on its current field list.
    // The answer-level path is therefore proven in `node.rs`, against an answer
    // a real node sent, which is the better evidence for it in any case.
    let mapped = User::from_row(row(&[
        ("name", Value::from("ada")),
        ("age", Value::from(36_i64)),
        ("nickname", Value::Null),
        ("tags", Value::Array(vec![Value::from("founder")])),
    ]))
    .expect("the record maps");

    assert_eq!(
        mapped,
        User {
            id: "1".to_owned(),
            name: "ada".to_owned(),
            age: 36,
            nickname: None,
            tags: vec!["founder".to_owned()],
        }
    );
}

#[test]
fn absent_and_null_both_read_as_no_value_but_a_missing_key_does_not_stop_the_mapping() {
    // Three different ways for an optional field to hold nothing. All three are
    // the same answer to `Option<String>`, and the third — the key simply not
    // being there — is the one a naive implementation reports as a failure.
    let present_null: Option<String> = row(&[("nickname", Value::Null)])
        .take("nickname")
        .expect("null is a value");
    let present_none: Option<String> = row(&[("nickname", Value::None)])
        .take("nickname")
        .expect("an absent marker is a value");
    let no_key_at_all: Option<String> = row(&[("name", Value::from("ada"))])
        .take("nickname")
        .expect("a missing key is what Option is for");

    assert_eq!(present_null, None);
    assert_eq!(present_none, None);
    assert_eq!(no_key_at_all, None);
}

#[test]
fn a_required_field_that_is_missing_is_a_fault_that_names_it() {
    let fault = row(&[("name", Value::from("ada"))])
        .take::<i64>("age")
        .expect_err("age is required");

    assert_eq!(
        fault,
        MappingFault::NoSuchField {
            name: "age".to_owned()
        }
    );
    assert_eq!(fault.to_string(), "no field named `age`");
}

#[test]
fn a_required_field_that_is_null_is_a_fault_and_not_a_zero() {
    // The failure this pins is the one that does not look like a failure: a
    // mapping that answered `0` here would produce a report that is wrong by a
    // number nobody can distinguish from a real one.
    let fault = row(&[("age", Value::Null)])
        .take::<i64>("age")
        .expect_err("null is not a whole number");

    assert_eq!(
        fault.to_string(),
        "in field `age`: expected a whole number, found null"
    );
}

#[test]
fn a_wrong_type_names_the_field_the_expectation_and_what_was_there() {
    let fault = row(&[("age", Value::from("thirty-six"))])
        .take::<i64>("age")
        .expect_err("text is not a whole number");

    assert_eq!(
        fault.to_string(),
        "in field `age`: expected a whole number, found text"
    );
}

#[test]
fn a_bad_element_reports_its_position_rather_than_the_whole_field() {
    let fault = row(&[(
        "tags",
        Value::Array(vec![
            Value::from("founder"),
            Value::from("engineer"),
            Value::from(7_i64),
        ]),
    )])
    .take::<Vec<String>>("tags")
    .expect_err("a whole number is not text");

    assert_eq!(
        fault.to_string(),
        "in field `tags`: at element 2: expected text, found a whole number"
    );
}

#[test]
fn a_whole_number_does_not_widen_itself_into_a_float() {
    // Deliberate strictness. An i64 carries more precision than an f64 can hold,
    // so the widening is lossy at the top of the range and silent everywhere.
    // A caller who wants it does it themselves, where the choice is visible.
    let fault = row(&[("price", Value::from(10_i64))])
        .take::<f64>("price")
        .expect_err("a whole number is not a float");

    assert_eq!(
        fault.to_string(),
        "in field `price`: expected a floating-point number, found a whole number"
    );

    // ...and the undecided target accepts it, which is the way through.
    let held: Number = row(&[("price", Value::from(10_i64))])
        .take("price")
        .expect("a number is a number");
    assert_eq!(held, Number::Integer(10));
}

#[test]
fn a_value_can_be_taken_unconverted() {
    // The escape hatch: a store type with no Rust counterpart here is still
    // reachable rather than walled off.
    let held: Value = row(&[(
        "when",
        Value::Datetime {
            seconds: 5,
            nanos: 6,
        },
    )])
    .take("when")
    .expect("a value is always a value");

    assert_eq!(
        held,
        Value::Datetime {
            seconds: 5,
            nanos: 6
        }
    );
}

#[test]
fn a_set_maps_as_a_sequence() {
    let held: Vec<String> = row(&[("tags", Value::Set(vec![Value::from("founder")]))])
        .take("tags")
        .expect("a set is a sequence");

    assert_eq!(held, vec!["founder".to_owned()]);
}

#[test]
fn an_answer_that_holds_no_records_says_so_instead_of_answering_nothing() {
    // An empty Vec here would be indistinguishable from a query that matched
    // nothing, which is the wrong answer to "you ran a DEFINE".
    let fault = Answer::Done
        .records_into::<User>()
        .expect_err("Done holds no records");

    assert_eq!(fault, MappingFault::NotRecords);
}

#[test]
fn a_record_that_is_not_an_object_has_no_fields_to_address() {
    let fault = Row::new("1".to_owned(), Value::from(7_i64)).expect_err("a number has no fields");

    assert_eq!(
        fault.to_string(),
        "a record holds a whole number, which has no fields"
    );
}

#[test]
fn a_record_carrying_more_than_the_struct_declares_still_maps() {
    // A schema ahead of its client is the ordinary case, not an error: the
    // mapping takes what it was asked for and leaves the rest.
    let mut subject = row(&[
        ("name", Value::from("ada")),
        (
            "joined",
            Value::Datetime {
                seconds: 1,
                nanos: 0,
            },
        ),
    ]);

    let name: String = subject.take("name").expect("name is text");

    assert_eq!(name, "ada");
}

#[test]
fn the_empty_object_is_a_row_with_nothing_in_it() {
    // Not an error: a record with no fields is a record, and a mapping whose
    // struct is all-optional should succeed against it.
    let mut subject = Row::new("1".to_owned(), object(&[])).expect("an object is a row");
    let nickname: Option<String> = subject.take("nickname").expect("all fields are absent");

    assert_eq!(nickname, None);
}

#[test]
fn taking_a_field_removes_it_so_a_second_take_finds_nothing() {
    // Documents the consequence of moving rather than cloning, so that the
    // behaviour is a decision on the record instead of a surprise.
    let mut subject = row(&[("name", Value::from("ada"))]);
    let first: String = subject.take("name").expect("the first take reads it");
    let second = subject
        .take::<String>("name")
        .expect_err("the second does not");

    assert_eq!(first, "ada");
    assert_eq!(
        second,
        MappingFault::NoSuchField {
            name: "name".to_owned()
        }
    );
}

#[test]
fn conversions_are_available_without_a_row() {
    // `FromValue` is usable on a bare value — a `Answer::Value` outcome carries
    // one with no record around it, and nothing should force a caller to invent
    // a row to convert it.
    assert_eq!(String::from_value(Value::from("ada")).unwrap(), "ada");
    assert_eq!(i64::from_value(Value::from(36_i64)).unwrap(), 36);
    assert!(bool::from_value(Value::Bool(true)).unwrap());
    assert_eq!(
        Vec::<u8>::from_value(Value::Bytes(vec![1, 2])).unwrap(),
        vec![1, 2]
    );
    // Compared as bits rather than as numbers. The conversion is a pass-through,
    // so the claim being made is the strong one — the same float came back, not
    // one that is near enough — and `==` on a float cannot express that.
    assert_eq!(
        f64::from_value(Value::Number(Number::Float(1.5)))
            .unwrap()
            .to_bits(),
        1.5_f64.to_bits()
    );
}
