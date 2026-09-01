//! The `Authorization` header, and the base64 underneath it.
//!
//! # Why this is written rather than taken
//!
//! The same question ADR-SDK-0004 asks of every candidate dependency: is the
//! grammar **closed**? RFC 4648 is sixty-four symbols, one padding rule, and no
//! content the encoder has to interpret — the whole alphabet fits on one line
//! and the published test vectors are exhaustive. That is the same shape as the
//! HTTP framing, and it gets the same answer.
//!
//! The contrast with `serde_json` is the rule rather than an exception to it:
//! JSON is **open**, because what it carries is a stranger's text. Base64 has no
//! content of its own at all — it is a spelling of bytes.
//!
//! Only the encoder is here. Nothing in this client ever reads a base64 value,
//! so a decoder would be code with no caller, and the first bug in it would be
//! found by whoever eventually wrote one (BGV-MINIMAL-001).

/// RFC 4648 §4, the standard alphabet. Not the URL-safe one: this value goes in
/// a header, not in a path.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The whole header value for these credentials, ready to send.
///
/// Built once and held, rather than at each call: a secret assembled per request
/// is a secret at every call site, and this way the plaintext password stops
/// existing inside this crate the moment the handle is made.
///
/// The name and password are joined with a colon and encoded as **bytes**, so a
/// password outside ASCII travels as its UTF-8, which is what the node decodes.
/// A name containing a colon is not rejected here — RFC 7617 says the first
/// colon separates, so such a name is unrepresentable in the scheme itself, and
/// the node is the party that decides what it makes of one.
pub fn header(name: &str, password: &str) -> String {
    let mut joined =
        String::with_capacity(name.len().saturating_add(password.len()).saturating_add(1));
    joined.push_str(name);
    joined.push(':');
    joined.push_str(password);
    format!("Basic {}", encode(joined.as_bytes()))
}

/// Base64 as RFC 4648 §4 spells it, padded.
fn encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));

    for group in bytes.chunks(3) {
        // A short final group is encoded as though the missing bytes were zero,
        // and the symbols those zeroes would have produced are written as `=`
        // instead. Taking the missing bytes as zero here is what makes the two
        // cases one loop rather than three.
        let (first, second, third) = match group {
            [a] => (*a, 0, 0),
            [a, b] => (*a, *b, 0),
            [a, b, c] => (*a, *b, *c),
            // `chunks(3)` yields nothing else, and saying so as a pattern rather
            // than as a comment keeps the compiler holding the claim.
            _ => continue,
        };

        // Four six-bit groups out of three bytes. Every shift is by a constant
        // smaller than the width and every mask keeps the result inside a `u8`,
        // so none of this can overflow — which is why it is written in `u8`
        // rather than packed into a `u32` and cast back down.
        encoded.push(symbol(first >> 2));
        encoded.push(symbol((first & 0b0000_0011) << 4 | (second >> 4)));
        encoded.push(if group.len() > 1 {
            symbol((second & 0b0000_1111) << 2 | (third >> 6))
        } else {
            '='
        });
        encoded.push(if group.len() > 2 { symbol(third) } else { '=' });
    }

    encoded
}

/// One symbol, for one six-bit group.
///
/// The mask is what makes the lookup total: it leaves at most 63 and the
/// alphabet is 64 long. `indexing_slicing` is denied across this crate and
/// rightly so, and it is lifted for exactly this line — where the proof of the
/// bound is the expression itself rather than a promise about every caller.
#[expect(
    clippy::indexing_slicing,
    reason = "the mask bounds the index to 0..=63 and the alphabet is 64 long"
)]
fn symbol(six: u8) -> char {
    char::from(ALPHABET[usize::from(six & 0b0011_1111)])
}

#[cfg(test)]
mod tests {
    use super::{encode, header};

    /// The vectors of RFC 4648 §10, verbatim and complete.
    ///
    /// They are worth taking whole rather than sampling: between them they reach
    /// every remainder — the empty input, one leftover byte, two leftover bytes,
    /// and a length that divides evenly — which is exactly where an encoder
    /// written from the description goes wrong.
    #[test]
    fn the_published_vectors_hold() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    /// The vectors above are all lowercase ASCII, which never reaches the top of
    /// the alphabet or the `+` and `/` at its end. A byte string that walks the
    /// whole range does.
    #[test]
    fn the_far_end_of_the_alphabet_is_reached() {
        assert_eq!(encode(&[0x00, 0x00, 0x00]), "AAAA");
        assert_eq!(encode(&[0xFF, 0xFF, 0xFF]), "////");
        assert_eq!(encode(&[0xFB, 0xFF]), "+/8=");
        assert_eq!(encode(&[0x00, 0x10, 0x83]), "ABCD");
    }

    /// The one measurement this wave took from the node, asserted here so the
    /// encoder is pinned to the value a real node was seen to accept rather than
    /// to my reading of the RFC alone.
    #[test]
    fn the_header_is_the_value_the_node_was_seen_to_accept() {
        assert_eq!(header("admin", "s3cr3t pw"), "Basic YWRtaW46czNjcjN0IHB3");
    }

    /// A password outside ASCII is its UTF-8, and a password containing a colon
    /// is left alone — only the first colon separates, so the second is data.
    #[test]
    fn a_password_is_bytes_and_not_a_grammar() {
        assert_eq!(header("u", "pä"), "Basic dTpww6Q=");
        assert_eq!(header("u", "a:b"), "Basic dTphOmI=");
    }
}
