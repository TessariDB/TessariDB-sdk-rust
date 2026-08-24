//! The shared conformance corpus, run against this client.
//!
//! # Why this is worth more than the other two test files
//!
//! `codec.rs` and `wire.rs` check this client against this author's reading of
//! the specification. They cannot catch a *consistent* misreading — one where the
//! encoder and the decoder are wrong in the same way, and every round trip
//! therefore passes.
//!
//! The corpus is produced by a **second implementation**, in another language,
//! written from the specification alone. When the two agree byte for byte, the
//! agreement is evidence that the document says enough for an independent reader
//! to arrive at the same bytes — which is the property the specification exists
//! to have, and which no amount of self-testing can demonstrate.
//!
//! It is still not the strongest tier. Two readers of one document can both be
//! wrong about what the node does; only the node settles that.

// Test assertions are exactly where a panic is the correct outcome; the lints
// these turn off target production paths, where a panic is a defect.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::ops::Bound;
use std::path::PathBuf;

use bgv_db_sdk::codec::{decode, encode};
use bgv_db_sdk::{
    Geometry, Number, Polygon, Position, RecordId, RecordRef, Ring, Value, ValueRange,
};
use serde_json::Value as Json;

/// Where the corpus lives.
///
/// The protocol is its own repository, so the path is configurable and the
/// default assumes it sits beside this one. A missing corpus **fails** rather
/// than skipping: a conformance suite that quietly passes when it found nothing
/// to check is worse than no suite at all, because it reports coverage it does
/// not have.
fn corpus_path() -> PathBuf {
    std::env::var("BGV_PROTOCOL_CORPUS").map_or_else(
        |_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../bgv-db-protocol/conformance/values-v1.json")
        },
        PathBuf::from,
    )
}

fn hex_to_bytes(text: &str) -> Vec<u8> {
    assert!(
        text.len().is_multiple_of(2),
        "hex must have an even length: {text}"
    );
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("hex is ascii");
            u8::from_str_radix(pair, 16).expect("valid hex")
        })
        .collect()
}

/// Build a value from the corpus's tagged-key notation.
fn value_of(json: &Json) -> Value {
    let object = json.as_object().expect("a value is an object");
    assert_eq!(object.len(), 1, "a value is exactly one tagged key");
    let (kind, held) = object.iter().next().expect("one entry");
    match kind.as_str() {
        "none" => Value::None,
        "null" => Value::Null,
        "bool" => Value::Bool(held.as_bool().expect("a bool")),
        "integer" => Value::Number(Number::Integer(
            held.as_str()
                .expect("an integer as text")
                .parse()
                .expect("an i64"),
        )),
        "float_bits" => {
            let bits = u64::from_str_radix(held.as_str().expect("hex bits"), 16).expect("u64 hex");
            Value::Number(Number::Float(f64::from_bits(bits)))
        }
        "decimal" => Value::Number(Number::Decimal {
            mantissa: held["mantissa"]
                .as_str()
                .expect("text")
                .parse()
                .expect("i128"),
            scale: u32::try_from(held["scale"].as_u64().expect("a scale")).expect("u32"),
        }),
        "string" => Value::String(held.as_str().expect("text").to_owned()),
        "bytes" => Value::Bytes(hex_to_bytes(held.as_str().expect("hex"))),
        "duration" => Value::Duration {
            seconds: held["seconds"]
                .as_str()
                .expect("text")
                .parse()
                .expect("i64"),
            nanos: u32::try_from(held["nanos"].as_u64().expect("nanos")).expect("u32"),
        },
        "datetime" => Value::Datetime {
            seconds: held["seconds"]
                .as_str()
                .expect("text")
                .parse()
                .expect("i64"),
            nanos: u32::try_from(held["nanos"].as_u64().expect("nanos")).expect("u32"),
        },
        "uuid" => Value::Uuid(
            <[u8; 16]>::try_from(hex_to_bytes(held.as_str().expect("hex")).as_slice())
                .expect("sixteen bytes"),
        ),
        "table" => Value::Table(u32::try_from(held.as_u64().expect("an id")).expect("u32")),
        "record" => Value::Record(RecordRef::new(
            u32::try_from(held["table"].as_u64().expect("a table")).expect("u32"),
            record_id_of(&held["id"]),
        )),
        "array" => Value::Array(
            held.as_array()
                .expect("an array")
                .iter()
                .map(value_of)
                .collect(),
        ),
        "object" => {
            let mut fields = BTreeMap::new();
            for (name, field) in held.as_object().expect("an object") {
                fields.insert(name.clone(), value_of(field));
            }
            Value::Object(fields)
        }
        "range" => Value::Range(Box::new(ValueRange::new(
            bound_of(&held["start"]),
            bound_of(&held["end"]),
        ))),
        "set" => Value::Set(
            held.as_array()
                .expect("an array")
                .iter()
                .map(value_of)
                .collect(),
        ),
        "geometry" => Value::Geometry(geometry_of(held)),
        "regex" => Value::Regex(held.as_str().expect("a pattern").to_owned()),
        other => panic!("the corpus names a value kind this client has no type for: {other}"),
    }
}

/// A coordinate, from the corpus's **bits**.
///
/// The corpus writes each coordinate as eight bytes of hex rather than as a
/// decimal literal, so nothing in this path depends on two languages agreeing
/// about how to parse `2.3522` — which they do, but which would be an
/// unnecessary thing for a byte-level corpus to rest on.
fn position_of(json: &Json) -> Position {
    let longitude = u64::from_str_radix(json["lon"].as_str().expect("hex bits"), 16).expect("u64");
    let latitude = u64::from_str_radix(json["lat"].as_str().expect("hex bits"), 16).expect("u64");
    Position::new(f64::from_bits(longitude), f64::from_bits(latitude))
}

fn positions_of(json: &Json) -> Vec<Position> {
    json.as_array()
        .expect("positions are an array")
        .iter()
        .map(position_of)
        .collect()
}

fn polygon_of(json: &Json) -> Polygon {
    Polygon {
        exterior: Ring(positions_of(&json["exterior"])),
        interiors: json["interiors"]
            .as_array()
            .expect("interiors are an array")
            .iter()
            .map(|ring| Ring(positions_of(ring)))
            .collect(),
    }
}

fn geometry_of(json: &Json) -> Geometry {
    let object = json.as_object().expect("a geometry is an object");
    assert_eq!(object.len(), 1, "a geometry is exactly one tagged key");
    let (kind, held) = object.iter().next().expect("one entry");
    match kind.as_str() {
        "point" => Geometry::Point(position_of(held)),
        "line" => Geometry::Line(positions_of(held)),
        "polygon" => Geometry::Polygon(polygon_of(held)),
        "multipoint" => Geometry::MultiPoint(positions_of(held)),
        "multiline" => Geometry::MultiLine(
            held.as_array()
                .expect("lines are an array")
                .iter()
                .map(positions_of)
                .collect(),
        ),
        "multipolygon" => Geometry::MultiPolygon(
            held.as_array()
                .expect("polygons are an array")
                .iter()
                .map(polygon_of)
                .collect(),
        ),
        "collection" => Geometry::Collection(
            held.as_array()
                .expect("a collection is an array")
                .iter()
                .map(|one| Box::new(geometry_of(one)))
                .collect(),
        ),
        other => panic!("the corpus names a shape this client has no type for: {other}"),
    }
}

fn record_id_of(json: &Json) -> RecordId {
    let object = json.as_object().expect("a record id is an object");
    let (kind, held) = object.iter().next().expect("one entry");
    match kind.as_str() {
        "int" => RecordId::Int(held.as_str().expect("text").parse().expect("i64")),
        "text" => RecordId::Text(held.as_str().expect("text").to_owned()),
        "uuid" => RecordId::Uuid(
            <[u8; 16]>::try_from(hex_to_bytes(held.as_str().expect("hex")).as_slice())
                .expect("sixteen bytes"),
        ),
        "bytes" => RecordId::Bytes(hex_to_bytes(held.as_str().expect("hex"))),
        other => panic!("no such record id kind: {other}"),
    }
}

fn bound_of(json: &Json) -> Bound<Value> {
    if json.as_str() == Some("unbounded") {
        return Bound::Unbounded;
    }
    let object = json
        .as_object()
        .expect("a bound is an object or \"unbounded\"");
    let (kind, held) = object.iter().next().expect("one entry");
    match kind.as_str() {
        "included" => Bound::Included(value_of(held)),
        "excluded" => Bound::Excluded(value_of(held)),
        other => panic!("no such bound: {other}"),
    }
}

/// Compare, treating a NaN's *bits* as the thing that must survive.
///
/// `f64::NAN != f64::NAN`, so a corpus case carrying a NaN would fail an
/// ordinary equality check even when the bytes are exactly right. What the
/// protocol promises is that the bits cross unchanged, and that is what is
/// asserted.
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(Number::Float(a)), Value::Number(Number::Float(b))) => {
            a.to_bits() == b.to_bits()
        }
        _ => left == right,
    }
}

#[test]
fn every_corpus_vector_encodes_and_decodes_exactly() {
    let path = corpus_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|why| {
        panic!(
            "the conformance corpus is required, not optional — a suite that \
             passes having found nothing reports coverage it does not have.\n\
             tried: {}\n{why}\n\
             set BGV_PROTOCOL_CORPUS to point at values-v1.json",
            path.display()
        )
    });
    let document: Json = serde_json::from_str(&raw).expect("the corpus is JSON");

    assert_eq!(
        document["protocol_major"].as_u64(),
        Some(u64::from(bgv_db_sdk::protocol::MAJOR)),
        "this client implements a different major from the corpus"
    );

    let cases = document["cases"].as_array().expect("the corpus has cases");
    assert!(
        cases.len() >= 50,
        "the corpus shrank to {} cases — a suite cannot get stronger by losing vectors",
        cases.len()
    );

    let mut checked = 0_usize;
    for case in cases {
        let name = case["name"].as_str().expect("every case is named");
        let expected = hex_to_bytes(case["bytes"].as_str().expect("every case has bytes"));
        let value = value_of(&case["value"]);

        // Forwards: this client must produce exactly the corpus's bytes.
        let produced = encode(&value);
        assert_eq!(
            produced,
            expected,
            "case {name}: this client encoded {} but the corpus says {}",
            hex(&produced),
            hex(&expected)
        );

        // Backwards: the corpus's bytes must decode to exactly the value. Not
        // implied by the forward check — an encoder and a decoder can agree with
        // each other and both disagree with the document.
        let decoded = decode(&expected)
            .unwrap_or_else(|why| panic!("case {name}: the corpus bytes did not decode: {why}"));
        assert!(
            same(&decoded, &value),
            "case {name}: decoded {decoded:?}, corpus says {value:?}"
        );

        checked = checked.saturating_add(1);
    }

    assert_eq!(checked, cases.len(), "every case must have been checked");
    println!("conformance: {checked} vectors, both directions");
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}
