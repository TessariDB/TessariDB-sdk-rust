//! Decoding a value out of the bytes the protocol carries.
//!
//! Every read is bounds-checked against the remaining input, and every unknown
//! discriminant is an error rather than a default. Both matter more here than
//! almost anywhere else in the crate: these bytes come from the network.

use std::collections::BTreeMap;
use std::ops::Bound;

use crate::codec::{
    ESCAPE, ESCAPED_ZERO, TERMINATOR, bound_kind, number_kind, record_id_kind, tag,
};
use crate::error::EncodingFault;
use crate::value::{Number, RecordId, RecordRef, Value, ValueRange};

/// How deep a nested value may go before this client refuses it.
///
/// A value may hold an array which holds a range which holds a set, and nothing
/// in the encoding bounds that. Without a limit, a few hundred bytes of crafted
/// input recurses until the stack ends — which is a crash, not an error, and one
/// no caller can catch. The limit is generous rather than tuned: real data is
/// nowhere near it, and it exists to make a hostile input an ordinary refusal.
const MAX_DEPTH: u32 = 128;

/// Decode one value, and require that it consumed everything.
///
/// Trailing bytes are an error rather than something to ignore: they mean this
/// build and the sender disagree about the value's shape, and continuing would
/// hand a caller a value that is right only by luck.
pub fn decode(bytes: &[u8]) -> Result<Value, EncodingFault> {
    let mut reader = Reader::new(bytes);
    let value = reader.take_value(0)?;
    let left = reader.remaining();
    if left == 0 {
        Ok(value)
    } else {
        Err(EncodingFault::TrailingBytes { count: left })
    }
}

/// A bounds-checked cursor over a value's bytes.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], EncodingFault> {
        let end = self.at.checked_add(count).ok_or(EncodingFault::Truncated)?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(EncodingFault::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn take_u8(&mut self) -> Result<u8, EncodingFault> {
        self.take(1)?
            .first()
            .copied()
            .ok_or(EncodingFault::Truncated)
    }

    fn take_fixed<const N: usize>(&mut self) -> Result<[u8; N], EncodingFault> {
        let slice = self.take(N)?;
        <[u8; N]>::try_from(slice).map_err(|_| EncodingFault::Truncated)
    }

    fn take_u32(&mut self) -> Result<u32, EncodingFault> {
        Ok(u32::from_be_bytes(self.take_fixed::<4>()?))
    }

    /// An `i64` written with its sign bit inverted — see [`super`].
    fn take_i64(&mut self) -> Result<i64, EncodingFault> {
        let mut bytes = self.take_fixed::<8>()?;
        if let Some(first) = bytes.first_mut() {
            *first ^= 0x80;
        }
        Ok(i64::from_be_bytes(bytes))
    }

    /// A count, as a bound on how many items may follow.
    ///
    /// The count is **not** used to pre-allocate. A four-byte field can claim
    /// four billion items, and reserving that on a stranger's word is the
    /// allocation the frame ceiling exists to prevent — one layer down. The
    /// items are pushed as they decode, so a lie costs a truncation error rather
    /// than the process.
    fn take_count(&mut self) -> Result<u32, EncodingFault> {
        self.take_u32()
    }

    fn take_lenbytes(&mut self) -> Result<Vec<u8>, EncodingFault> {
        let len = self.take_u32()?;
        let len = usize::try_from(len).map_err(|_| EncodingFault::Truncated)?;
        Ok(self.take(len)?.to_vec())
    }

    fn take_text(&mut self) -> Result<String, EncodingFault> {
        String::from_utf8(self.take_lenbytes()?).map_err(|_| EncodingFault::InvalidUtf8)
    }

    /// An escaped, terminated block.
    fn take_varbytes(&mut self) -> Result<Vec<u8>, EncodingFault> {
        let mut out = Vec::new();
        loop {
            let byte = self.take_u8().map_err(|_| EncodingFault::Unterminated)?;
            if byte != ESCAPE {
                out.push(byte);
                continue;
            }
            let next = self.take_u8().map_err(|_| EncodingFault::Unterminated)?;
            match next {
                TERMINATOR => return Ok(out),
                ESCAPED_ZERO => out.push(ESCAPE),
                found => return Err(EncodingFault::InvalidEscape { found }),
            }
        }
    }

    pub(crate) fn take_value(&mut self, depth: u32) -> Result<Value, EncodingFault> {
        if depth > MAX_DEPTH {
            return Err(EncodingFault::Truncated);
        }
        let deeper = depth.saturating_add(1);
        let found = self.take_u8()?;
        match found {
            tag::NONE => Ok(Value::None),
            tag::NULL => Ok(Value::Null),
            tag::BOOL => Ok(Value::Bool(self.take_u8()? != 0)),
            tag::NUMBER => Ok(Value::Number(self.take_number()?)),
            tag::STRING => Ok(Value::String(self.take_text()?)),
            tag::BYTES => Ok(Value::Bytes(self.take_lenbytes()?)),
            tag::DURATION => {
                let (seconds, nanos) = self.take_time()?;
                Ok(Value::Duration { seconds, nanos })
            }
            tag::DATETIME => {
                let (seconds, nanos) = self.take_time()?;
                Ok(Value::Datetime { seconds, nanos })
            }
            tag::UUID => Ok(Value::Uuid(self.take_fixed::<16>()?)),
            tag::TABLE => Ok(Value::Table(self.take_u32()?)),
            tag::RECORD => {
                let table = self.take_u32()?;
                let id = self.take_record_id()?;
                Ok(Value::Record(RecordRef::new(table, id)))
            }
            tag::ARRAY => {
                let count = self.take_count()?;
                let mut items = Vec::new();
                for _ in 0..count {
                    items.push(self.take_value(deeper)?);
                }
                Ok(Value::Array(items))
            }
            tag::OBJECT => {
                let count = self.take_count()?;
                let mut fields = BTreeMap::new();
                for _ in 0..count {
                    let name = self.take_text()?;
                    fields.insert(name, self.take_value(deeper)?);
                }
                Ok(Value::Object(fields))
            }
            tag::RANGE => {
                let start = self.take_bound(deeper)?;
                let end = self.take_bound(deeper)?;
                Ok(Value::Range(Box::new(ValueRange::new(start, end))))
            }
            tag::SET => {
                let count = self.take_count()?;
                let mut items = Vec::new();
                for _ in 0..count {
                    items.push(self.take_value(deeper)?);
                }
                Ok(Value::Set(items))
            }
            unknown => Err(EncodingFault::UnknownTag { tag: unknown }),
        }
    }

    fn take_time(&mut self) -> Result<(i64, u32), EncodingFault> {
        let seconds = self.take_i64()?;
        let nanos = self.take_u32()?;
        // A second holds a billion nanoseconds; anything above is a value this
        // client will not hand to a caller as if it meant something.
        if nanos >= 1_000_000_000 {
            return Err(EncodingFault::InvalidSubSecond { nanos });
        }
        Ok((seconds, nanos))
    }

    fn take_number(&mut self) -> Result<Number, EncodingFault> {
        match self.take_u8()? {
            number_kind::INTEGER => Ok(Number::Integer(self.take_i64()?)),
            number_kind::FLOAT => {
                let bits = u64::from_be_bytes(self.take_fixed::<8>()?);
                Ok(Number::Float(f64::from_bits(bits)))
            }
            number_kind::DECIMAL => {
                let mantissa = i128::from_be_bytes(self.take_fixed::<16>()?);
                let scale = self.take_u32()?;
                Ok(Number::Decimal { mantissa, scale })
            }
            unknown => Err(EncodingFault::UnknownTag { tag: unknown }),
        }
    }

    pub(crate) fn take_record_id(&mut self) -> Result<RecordId, EncodingFault> {
        match self.take_u8()? {
            record_id_kind::INT => Ok(RecordId::Int(self.take_i64()?)),
            record_id_kind::TEXT => {
                let bytes = self.take_varbytes()?;
                String::from_utf8(bytes)
                    .map(RecordId::Text)
                    .map_err(|_| EncodingFault::InvalidUtf8)
            }
            record_id_kind::UUID => Ok(RecordId::Uuid(self.take_fixed::<16>()?)),
            record_id_kind::BYTES => Ok(RecordId::Bytes(self.take_varbytes()?)),
            unknown => Err(EncodingFault::UnknownTag { tag: unknown }),
        }
    }

    fn take_bound(&mut self, depth: u32) -> Result<Bound<Value>, EncodingFault> {
        match self.take_u8()? {
            bound_kind::UNBOUNDED => Ok(Bound::Unbounded),
            bound_kind::INCLUDED => Ok(Bound::Included(self.take_value(depth)?)),
            bound_kind::EXCLUDED => Ok(Bound::Excluded(self.take_value(depth)?)),
            unknown => Err(EncodingFault::UnknownTag { tag: unknown }),
        }
    }
}
