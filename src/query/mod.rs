//! Building statements without ever building an injection.
//!
//! # The one guarantee
//!
//! **A value never reaches the script text.** Every value a caller supplies
//! becomes a bound parameter; the script carries `$p0`, and the value travels
//! beside it in the store's own codec. A builder that formatted values into the
//! text would destroy the property the whole surface is designed around, and
//! would do it invisibly, because the output still looks correct.
//!
//! # A name is grammar; a value is not
//!
//! Table names, field names and directions are **syntax** — a parameter may
//! never supply one, so they are interpolated. That is safe only because each is
//! checked to be an ordinary identifier first, and the check is *in front of* the
//! interpolation rather than trusted to be somewhere.
//!
//! The check is deliberately **narrower** than what the node's lexer accepts. A
//! guard that reasons about what a lexer would do is a guard that has to be
//! re-checked every time the lexer changes; one that accepts only
//! `[A-Za-z_][A-Za-z0-9_]*` does not.
//!
//! # What this builder does not promise
//!
//! That the text it emits **parses**. That guarantee needs the node's own parser,
//! which this client deliberately does not depend on.
//!
//! Nor does the protocol *specification* supply it: its §6 puts the query
//! language explicitly outside the protocol, so no section there says what a
//! rendering must be.
//!
//! A **shared** rendering corpus is still what this needs, because Python and Go
//! will each want a builder and none of them can link a Rust parser — where that
//! corpus lives is an open decision, not a settled one.
//!
//! Until it exists, two weaker things stand in its place, and naming them is the
//! point: `tests/query.rs` pins the exact text of every builder case, so a
//! change to the rendering is visible in a diff instead of silent; and an
//! acceptance run against a running node is **owed** and is the only thing that
//! settles whether the text parses. A grammar change on the node side will not
//! fail anything here.
//!
//! Coverage is a staging order, not a shape: `SELECT`, `CREATE`, `UPDATE` and
//! `DELETE` on one record or one table. Anything else is written by hand and sent
//! as a script, which is always available and always the fallback.

mod filter;
mod select;
mod write;

pub use crate::query::filter::{Filter, field};
pub use crate::query::select::{Order, Select};
pub use crate::query::write::{Create, Delete, Update};

use crate::value::Value;
use crate::wire::message::Parameters;

/// A statement and the values its parameters bind to.
///
/// Hand this to [`Client::run_with`](crate::Client::run_with).
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// The statement text. Contains parameter references, never values.
    pub script: String,
    /// What those references bind to.
    pub parameters: Parameters,
}

/// Why a statement could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// A name that is not a name.
    ///
    /// Refused rather than quoted. Quoting would turn a caller's mistake into a
    /// statement that runs and means something else.
    #[error(
        "{what} {name:?} is not a name: a name is letters, digits and underscores, and does not start with a digit"
    )]
    NotAName {
        /// Which position the name was in — a table, a field, a bucket.
        what: &'static str,
        /// What was supplied.
        name: String,
    },

    /// A statement with nothing to say.
    #[error("{what}")]
    Incomplete {
        /// What is missing.
        what: &'static str,
    },
}

/// Whether a string may be interpolated into a statement as a name.
///
/// Deliberately narrower than the node's lexer — see this module's documentation.
pub(crate) fn is_name(held: &str) -> bool {
    let mut characters = held.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

pub(crate) fn check_name(what: &'static str, name: &str) -> Result<(), BuildError> {
    if is_name(name) {
        Ok(())
    } else {
        Err(BuildError::NotAName {
            what,
            name: name.to_owned(),
        })
    }
}

/// Collects values into parameters and hands back the reference to write.
///
/// One counter per statement, so `$p0` in one query is unrelated to `$p0` in the
/// next — parameters are per-request and never global.
#[derive(Debug, Default)]
pub(crate) struct Binder {
    parameters: Parameters,
    next: usize,
}

impl Binder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Bind `value` and return the reference that names it.
    pub(crate) fn bind(&mut self, value: Value) -> String {
        let name = format!("p{}", self.next);
        self.next = self.next.saturating_add(1);
        let reference = format!("${name}");
        self.parameters.insert(name, value);
        reference
    }

    pub(crate) fn finish(self) -> Parameters {
        self.parameters
    }
}

/// A record identity as a statement spells it: `table:id`.
///
/// The identity travels as a **parameter**, not as text, so an identity that
/// happens to spell a statement is a record with an unusual name. Only the table
/// is interpolated, and only after being checked.
pub(crate) fn record_target(
    table: &str,
    id: Value,
    binder: &mut Binder,
) -> Result<String, BuildError> {
    check_name("a table", table)?;
    let reference = binder.bind(id);
    Ok(format!("{table}:{reference}"))
}
