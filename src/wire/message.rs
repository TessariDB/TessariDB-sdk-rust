//! What a client asks, and what it gets back.

use std::collections::BTreeMap;

use crate::codec::{decode, encode};
use crate::error::{Error, Result};
use crate::value::Value;
use crate::wire::frame::{Body, put_bytes, put_text, put_u32};

/// The values a script's parameters are bound to.
pub type Parameters = BTreeMap<String, Value>;

/// What the tables an answer references are called.
///
/// A reference holds a table id, and an id is meaningless outside the node that
/// minted it — the catalog lives on the node. Without these names a client can
/// only render an opaque reference, and the point of this protocol is that a
/// client decides nothing.
pub type Names = BTreeMap<u32, String>;

/// A script to run, and who is running it.
#[derive(Clone, PartialEq)]
pub struct Request {
    /// The script.
    pub script: String,
    /// The credentials, when the caller has any.
    ///
    /// Optional because a store with no users declared is open and runs
    /// anything, which is what keeps an empty one usable. A closed store's
    /// refusal comes from the node, not from a second rule here.
    pub credentials: Option<(String, String)>,
    /// The values the script's parameters bind to.
    ///
    /// Carried in the store's own codec rather than as text the node parses. A
    /// value the node has to *read* is a value that can be read as something
    /// else, and binding after parsing exists to make that impossible — undoing
    /// it at the wire would be a strange place to give it back.
    pub parameters: Parameters,
}

/// Written by hand rather than derived, because the derived one prints the
/// password.
///
/// Nothing in this crate prints a request. But a struct holding a credential and
/// answering `{:?}` with it is how a password reaches a log line, and the line
/// that does it is always somewhere else and written later. The name is shown,
/// because knowing *who* a refused request claimed to be is the whole value of
/// printing one; the secret is not.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("script", &self.script)
            .field(
                "as",
                &self
                    .credentials
                    .as_ref()
                    .map_or("nobody", |(name, _)| name.as_str()),
            )
            .field("parameters", &self.parameters.keys())
            .finish_non_exhaustive()
    }
}

impl Request {
    /// A request to run `script` as nobody in particular.
    #[must_use]
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            credentials: None,
            parameters: Parameters::new(),
        }
    }

    /// The same request, signed in as `name`.
    #[must_use]
    pub fn as_user(mut self, name: impl Into<String>, password: impl Into<String>) -> Self {
        self.credentials = Some((name.into(), password.into()));
        self
    }

    /// The same request with `name` bound to `value`.
    ///
    /// The value travels in the codec. It never becomes part of the script text,
    /// so a value cannot turn into syntax however it is spelled.
    #[must_use]
    pub fn bind(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.parameters.insert(name.into(), value.into());
        self
    }

    /// The body of a request frame.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        put_text(&mut body, &self.script);
        match &self.credentials {
            Some((name, password)) => {
                body.push(1);
                put_text(&mut body, name);
                put_text(&mut body, password);
            }
            None => body.push(0),
        }
        put_u32(
            &mut body,
            u32::try_from(self.parameters.len()).unwrap_or(u32::MAX),
        );
        for (name, value) in &self.parameters {
            put_text(&mut body, name);
            put_bytes(&mut body, &encode(value));
        }
        body
    }
}

/// The tag that opens an outcome.
mod tag {
    pub(super) const DONE: u8 = 0;
    pub(super) const RECORDS: u8 = 1;
    pub(super) const VALUE: u8 = 2;
    pub(super) const KEYS: u8 = 3;
    pub(super) const REMOVED: u8 = 4;
}

/// Something a read volunteered about how it answered.
///
/// A **kind and a message**, not a structure. A client's two uses are to group
/// by the first and show the second, and a typed note would put the node's whole
/// vocabulary into this build for no gain.
///
/// The kinds a node sends today are `fell-back`, `approximate`,
/// `compared-across-kinds`, `cursor-walked` and `subquery-ceiling`. That list is
/// **not** closed and is not matched on here: an unfamiliar kind is a newer node
/// saying something this build has no name for, which is not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Which kind of thing the read is saying.
    pub kind: String,
    /// What it says, in words meant for a person.
    pub message: String,
}

/// What a client got back for one statement.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Answer {
    /// The statement did its work.
    Done,
    /// Records, each with its identity as written and its value.
    ///
    /// Marked `#[non_exhaustive]` as well as the enum. The enum's attribute
    /// covers a new *variant*; this one covers a new *field*, and this variant
    /// has now gained one twice — the notes and the `ONLY` flag below — because
    /// the frame grows by appending. Without it every such addition is a
    /// breaking change for anyone matching the fields out.
    #[non_exhaustive]
    Records {
        /// What was found — identity as the node spells it, and the value.
        records: Vec<(String, Value)>,
        /// How the node found them: `record`, `index`, `ordered`, or `scan`.
        ///
        /// Reported, not requested. An unrecognised path reads as `scan`, which
        /// is the honest name for a path this build does not know: it is the one
        /// that promises nothing.
        path: String,
        /// What the tables these records reference are called.
        names: Names,
        /// What the read volunteered about how it answered.
        ///
        /// Empty when the node had nothing to say, and also when the node
        /// predates notes entirely — the two are the same claim, so they are not
        /// distinguished.
        notes: Vec<Note>,
        /// Whether the statement wrote `ONLY`.
        ///
        /// An assertion by the statement's author that at most one record
        /// answers. A caller that offers the clause should render such an answer
        /// as the record itself rather than as a list holding it; ignoring the
        /// flag is conforming, and produces a one-element array with nothing to
        /// say it was asked for differently.
        only: bool,
    },
    /// One value, and the names of the tables it references.
    Value {
        /// What was answered.
        value: Value,
        /// What the tables it references are called.
        names: Names,
    },
    /// Keys, as the node spells them.
    Keys(Vec<String>),
    /// How many records a conditional delete removed.
    Removed(u64),
    /// An outcome this build has no type for.
    ///
    /// Not an error. A newer node may answer with something this client has
    /// never seen, and saying so is honest where guessing at its content is not.
    /// A caller sees it rather than losing the outcome silently.
    Unknown,
}

/// Read every outcome out of an answer frame's body.
///
/// # Each outcome carries its own length, and that is what makes this safe
///
/// An outcome is a `u32` length and then its tagged body. A tag this build does
/// not know is therefore *skippable*: the reader yields [`Answer::Unknown`],
/// steps to the end of that outcome, and carries on with the next one. So a
/// newer node may introduce an outcome kind **anywhere** in an answer without
/// breaking this client, which is the whole content of the promise that a minor
/// version bump is compatible.
///
/// Each outcome is decoded inside a cursor bounded by its own length, so a
/// recognised outcome cannot read into the next one either — a body that claims
/// more than its length allows is malformed rather than a mis-parse of
/// everything after it.
///
/// Bytes left over **inside** one outcome are skipped rather than refused. That
/// is deliberate: it lets a later minor add a trailing field to an outcome kind
/// this build already knows, and an older client keeps reading the part it
/// understands.
pub fn decode_answers(body: &[u8]) -> Result<Vec<Answer>> {
    let mut reader = Body::new(body);
    let count = reader.take_u32()?;
    let mut answers = Vec::new();
    // The count is not used to pre-allocate: it is a stranger's number, and the
    // frame ceiling bounds the bytes rather than the claim.
    for _ in 0..count {
        let length = reader.take_u32()?;
        let length = usize::try_from(length).map_err(|_| Error::Malformed)?;
        let mut outcome = Body::new(reader.take(length)?);
        answers.push(decode_one(&mut outcome)?);
    }
    Ok(answers)
}

fn decode_one(reader: &mut Body<'_>) -> Result<Answer> {
    let found = reader.take_u8()?;
    match found {
        tag::DONE => Ok(Answer::Done),
        tag::RECORDS => {
            let path = path_name(reader.take_u8()?).to_owned();
            let names = take_names(reader)?;
            let count = reader.take_u32()?;
            let mut records = Vec::new();
            for _ in 0..count {
                let id = reader.take_text()?;
                let bytes = reader.take_bytes()?;
                records.push((id, decode(&bytes).map_err(Error::from)?));
            }
            // Everything past the records is a later addition, and the frame
            // grows by appending — so each field is read only if it is there,
            // and absent means the node had nothing to add rather than that the
            // body was truncated. The outcome's own length is what makes that
            // distinction safe to draw.
            let notes = if reader.remaining() > 0 {
                let count = reader.take_u32()?;
                let mut notes = Vec::new();
                for _ in 0..count {
                    let kind = reader.take_text()?;
                    let message = reader.take_text()?;
                    notes.push(Note { kind, message });
                }
                notes
            } else {
                Vec::new()
            };
            let only = reader.remaining() > 0 && reader.take_u8()? != 0;
            Ok(Answer::Records {
                records,
                path,
                names,
                notes,
                only,
            })
        }
        tag::VALUE => {
            let names = take_names(reader)?;
            let bytes = reader.take_bytes()?;
            Ok(Answer::Value {
                value: decode(&bytes).map_err(Error::from)?,
                names,
            })
        }
        tag::KEYS => {
            let count = reader.take_u32()?;
            let mut keys = Vec::new();
            for _ in 0..count {
                keys.push(reader.take_text()?);
            }
            Ok(Answer::Keys(keys))
        }
        tag::REMOVED => Ok(Answer::Removed(reader.take_u64()?)),
        // Consumes nothing further, and does not need to: the caller reads this
        // outcome inside a cursor bounded by its own length and steps to the end
        // of it either way.
        _ => Ok(Answer::Unknown),
    }
}

fn take_names(reader: &mut Body<'_>) -> Result<Names> {
    let count = reader.take_u32()?;
    let mut names = Names::new();
    for _ in 0..count {
        let table = reader.take_u32()?;
        let name = reader.take_text()?;
        names.insert(table, name);
    }
    Ok(names)
}

/// The access path a byte names.
const fn path_name(tag: u8) -> &'static str {
    match tag {
        0 => "record",
        1 => "index",
        3 => "ordered",
        _ => "scan",
    }
}
