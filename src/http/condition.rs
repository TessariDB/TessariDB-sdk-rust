//! What a node says about itself when asked whether it is well.
//!
//! Two routes ask nearly the same question and a caller acts on the difference,
//! so both answer with the same type and neither is the other's alias. See
//! [`Operations::health`] and [`Operations::ready`] for which question is which.
//!
//! [`Operations::health`]: crate::Operations::health
//! [`Operations::ready`]: crate::Operations::ready

use serde_json::Value as Json;

use crate::error::{Error, Result};

/// A node's own report on its condition.
///
/// # Why the shapes are variants and not one struct of options
///
/// The node answers with three different **field sets**, not with one shape
/// carrying absent fields: a well node reports its commit position, an unwell
/// one adds what it is complaining about, and a node that is leaving reports
/// **neither**. A struct of `Option`s would let a caller ask a departing node
/// for its commit position and receive `None` with nothing to say why. Here the
/// case a supervisor most needs to read correctly is the one that cannot be read
/// wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Condition {
    /// The store is well, and this is how far it has committed.
    Ok {
        /// The sequence number of the last commit.
        committed: u64,
    },

    /// The store is answering, and is complaining.
    ///
    /// The node still serves in this state; what it is saying is that it should
    /// not be relied on, and the complaint is the sentence to put in the page
    /// rather than a code to look up.
    Unwell {
        /// The sequence number of the last commit.
        committed: u64,
        /// How many background operations have failed.
        background_errors: u64,
        /// What the store is complaining about, in its own words.
        complaint: String,
    },

    /// The node is shutting down and no longer wants new work.
    ///
    /// It is still serving — that is the point of the state — so this is an
    /// instruction to route elsewhere, not a report that anything has failed.
    /// Only [`Operations::ready`] can return it.
    ///
    /// [`Operations::ready`]: crate::Operations::ready
    Leaving,
}

impl Condition {
    /// Read one of the three shapes, or refuse.
    ///
    /// A `status` this build does not know is [`Error::Malformed`] rather than a
    /// guess. The same discipline as the wire half, where an unknown frame kind
    /// ends the connection instead of being skipped: a client that treats an
    /// unfamiliar state as one it recognises reports a healthy node during
    /// whatever the new state was invented to describe.
    pub(crate) fn read(body: &[u8]) -> Result<Self> {
        let json: Json = serde_json::from_slice(body).map_err(|_| Error::Malformed)?;
        let status = json.get("status").and_then(Json::as_str);

        match status {
            Some("ok") => Ok(Self::Ok {
                committed: number(&json, "committed")?,
            }),
            Some("unwell") => Ok(Self::Unwell {
                committed: number(&json, "committed")?,
                background_errors: number(&json, "background_errors")?,
                complaint: json
                    .get("complaint")
                    .and_then(Json::as_str)
                    .ok_or(Error::Malformed)?
                    .to_owned(),
            }),
            Some("leaving") => Ok(Self::Leaving),
            _ => Err(Error::Malformed),
        }
    }
}

/// One unsigned field, or [`Error::Malformed`].
///
/// Absent and present-but-not-a-number are the same failure on purpose: both
/// mean this build cannot read what arrived, and distinguishing them would offer
/// a caller a choice it has no way to act on.
fn number(json: &Json, field: &str) -> Result<u64> {
    json.get(field)
        .and_then(Json::as_u64)
        .ok_or(Error::Malformed)
}
