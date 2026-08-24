//! Encoding a value into the bytes the protocol carries.

use std::ops::Bound;

use crate::codec::{
    ESCAPE, ESCAPED_ZERO, TERMINATOR, bound_kind, number_kind, record_id_kind, shape_kind, tag,
};
use crate::geometry::{Geometry, Polygon, Position};
use crate::value::{Number, RecordId, RecordRef, Value};

/// Encode one value.
#[must_use]
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    put_value(&mut out, value);
    out
}

/// An `i64`, big-endian, **with the sign bit inverted**.
///
/// Not two's-complement big-endian: `1` becomes `80 00 00 00 00 00 00 01`. See
/// this module's parent for why, and note that a client which writes plain
/// big-endian here gets every integer, duration, datetime and integer record id
/// wrong — without failing to parse.
fn put_i64(out: &mut Vec<u8>, value: i64) {
    let mut bytes = value.to_be_bytes();
    // Index 0 is in bounds for a fixed eight-byte array; the mask is what maps
    // `i64::MIN` to all-zero and `i64::MAX` to all-ones.
    if let Some(first) = bytes.first_mut() {
        *first ^= 0x80;
    }
    out.extend_from_slice(&bytes);
}

/// A `u32`, big-endian and plain — no inversion at this width.
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

/// A length or item count, as it is written.
///
/// A collection with more members than a `u32` counts is not something a node
/// can hold — it would have exhausted memory long before the codec saw it — so
/// saturating keeps this total instead of making every caller handle a case that
/// cannot arise. A decoder rejects the result as truncated, so the failure stays
/// loud either way.
fn count_of(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// A length-prefixed block.
fn put_lenbytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, count_of(bytes.len()));
    out.extend_from_slice(bytes);
}

/// An escaped, terminated block.
///
/// A zero becomes `00 FF`; the block ends with `00 01`. The escape is
/// byte-local, which is what makes the encoding of a prefix a byte prefix of the
/// encoding of the whole.
fn put_varbytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.reserve(bytes.len().saturating_add(2));
    for &byte in bytes {
        if byte == ESCAPE {
            out.push(ESCAPE);
            out.push(ESCAPED_ZERO);
        } else {
            out.push(byte);
        }
    }
    out.push(ESCAPE);
    out.push(TERMINATOR);
}

fn put_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::None => out.push(tag::NONE),
        Value::Null => out.push(tag::NULL),
        Value::Bool(flag) => {
            out.push(tag::BOOL);
            out.push(u8::from(*flag));
        }
        Value::Number(number) => {
            out.push(tag::NUMBER);
            put_number(out, number);
        }
        Value::String(text) => {
            out.push(tag::STRING);
            put_lenbytes(out, text.as_bytes());
        }
        Value::Bytes(bytes) => {
            out.push(tag::BYTES);
            put_lenbytes(out, bytes);
        }
        Value::Duration { seconds, nanos } => {
            out.push(tag::DURATION);
            put_i64(out, *seconds);
            put_u32(out, *nanos);
        }
        Value::Datetime { seconds, nanos } => {
            out.push(tag::DATETIME);
            put_i64(out, *seconds);
            put_u32(out, *nanos);
        }
        Value::Uuid(bytes) => {
            out.push(tag::UUID);
            out.extend_from_slice(bytes);
        }
        Value::Table(table) => {
            out.push(tag::TABLE);
            put_u32(out, *table);
        }
        Value::Record(reference) => {
            out.push(tag::RECORD);
            put_record_ref(out, reference);
        }
        Value::Array(items) => {
            out.push(tag::ARRAY);
            put_u32(out, count_of(items.len()));
            for item in items {
                put_value(out, item);
            }
        }
        Value::Object(fields) => {
            out.push(tag::OBJECT);
            put_u32(out, count_of(fields.len()));
            // A `BTreeMap` iterates in name order, so equal objects encode to
            // equal bytes without a sorting step. The node re-normalises on
            // decode regardless.
            for (name, field) in fields {
                put_lenbytes(out, name.as_bytes());
                put_value(out, field);
            }
        }
        Value::Range(range) => {
            out.push(tag::RANGE);
            put_bound(out, &range.start);
            put_bound(out, &range.end);
        }
        Value::Set(items) => {
            out.push(tag::SET);
            put_u32(out, count_of(items.len()));
            for item in items {
                put_value(out, item);
            }
        }
        Value::Geometry(shape) => {
            out.push(tag::GEOMETRY);
            put_geometry(out, shape);
        }
        Value::Regex(pattern) => {
            out.push(tag::REGEX);
            // The pattern as written. Not compiled, not validated: dialects
            // disagree about what is valid, so a client that checks rejects
            // patterns the node would have accepted.
            put_lenbytes(out, pattern.as_bytes());
        }
    }
}

/// A coordinate pair, **longitude first**, each as the double's bits.
///
/// Bits rather than any decimal form: a coordinate routed through text is a
/// different coordinate, and the difference survives every round trip this
/// client could perform on itself.
fn put_position(out: &mut Vec<u8>, position: &Position) {
    out.extend_from_slice(&position.longitude.to_bits().to_be_bytes());
    out.extend_from_slice(&position.latitude.to_bits().to_be_bytes());
}

fn put_positions(out: &mut Vec<u8>, positions: &[Position]) {
    put_u32(out, count_of(positions.len()));
    for position in positions {
        put_position(out, position);
    }
}

fn put_polygon(out: &mut Vec<u8>, polygon: &Polygon) {
    put_positions(out, &polygon.exterior.0);
    put_u32(out, count_of(polygon.interiors.len()));
    for interior in &polygon.interiors {
        put_positions(out, &interior.0);
    }
}

fn put_geometry(out: &mut Vec<u8>, shape: &Geometry) {
    match shape {
        Geometry::Point(position) => {
            out.push(shape_kind::POINT);
            put_position(out, position);
        }
        Geometry::Line(positions) => {
            out.push(shape_kind::LINE);
            put_positions(out, positions);
        }
        Geometry::Polygon(polygon) => {
            out.push(shape_kind::POLYGON);
            put_polygon(out, polygon);
        }
        Geometry::MultiPoint(positions) => {
            out.push(shape_kind::MULTI_POINT);
            put_positions(out, positions);
        }
        Geometry::MultiLine(lines) => {
            out.push(shape_kind::MULTI_LINE);
            put_u32(out, count_of(lines.len()));
            for line in lines {
                put_positions(out, line);
            }
        }
        Geometry::MultiPolygon(polygons) => {
            out.push(shape_kind::MULTI_POLYGON);
            put_u32(out, count_of(polygons.len()));
            for polygon in polygons {
                put_polygon(out, polygon);
            }
        }
        Geometry::Collection(shapes) => {
            out.push(shape_kind::COLLECTION);
            put_u32(out, count_of(shapes.len()));
            for held in shapes {
                // A whole geometry, kind byte included — which is what lets a
                // collection hold a collection.
                put_geometry(out, held);
            }
        }
    }
}

fn put_number(out: &mut Vec<u8>, number: &Number) {
    match number {
        Number::Integer(value) => {
            out.push(number_kind::INTEGER);
            put_i64(out, *value);
        }
        Number::Float(value) => {
            out.push(number_kind::FLOAT);
            // The *bits*, plain big-endian — not the inverted form. Three
            // numeric shapes, two conventions, and this is the asymmetry.
            out.extend_from_slice(&value.to_bits().to_be_bytes());
        }
        Number::Decimal { mantissa, scale } => {
            out.push(number_kind::DECIMAL);
            out.extend_from_slice(&mantissa.to_be_bytes());
            put_u32(out, *scale);
        }
    }
}

fn put_record_ref(out: &mut Vec<u8>, reference: &RecordRef) {
    put_u32(out, reference.table);
    put_record_id(out, &reference.id);
}

pub(crate) fn put_record_id(out: &mut Vec<u8>, id: &RecordId) {
    match id {
        RecordId::Int(value) => {
            out.push(record_id_kind::INT);
            put_i64(out, *value);
        }
        RecordId::Text(text) => {
            out.push(record_id_kind::TEXT);
            put_varbytes(out, text.as_bytes());
        }
        RecordId::Uuid(bytes) => {
            out.push(record_id_kind::UUID);
            out.extend_from_slice(bytes);
        }
        RecordId::Bytes(bytes) => {
            out.push(record_id_kind::BYTES);
            put_varbytes(out, bytes);
        }
    }
}

fn put_bound(out: &mut Vec<u8>, bound: &Bound<Value>) {
    match bound {
        Bound::Unbounded => out.push(bound_kind::UNBOUNDED),
        Bound::Included(value) => {
            out.push(bound_kind::INCLUDED);
            put_value(out, value);
        }
        Bound::Excluded(value) => {
            out.push(bound_kind::EXCLUDED);
            put_value(out, value);
        }
    }
}
