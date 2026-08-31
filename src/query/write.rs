//! Writing.

use std::collections::BTreeMap;

use crate::query::{Binder, BuildError, Query, check_name, record_target};
use crate::value::Value;

/// Write a record, replacing whatever was there.
///
/// ```
/// use tessaridb_client::query::Create;
/// use tessaridb_client::Value;
///
/// let query = Create::record("memories", "note-1")
///     .set("body", "the user prefers metric units")
///     .set("weight", 3_i64)
///     .build()
///     .expect("a well-formed query");
///
/// // Neither the identity nor the text reaches the script.
/// assert!(!query.script.contains("note-1"));
/// assert!(!query.script.contains("metric"));
/// assert_eq!(query.parameters.len(), 3);
/// ```
#[derive(Debug, Clone)]
pub struct Create {
    table: String,
    id: Option<Value>,
    fields: BTreeMap<String, Value>,
}

impl Create {
    /// A record in a table, by identity.
    #[must_use]
    pub fn record(table: impl Into<String>, id: impl Into<Value>) -> Self {
        Self {
            table: table.into(),
            id: Some(id.into()),
            fields: BTreeMap::new(),
        }
    }

    /// A record in a table, letting the store name it.
    ///
    /// The store allocates the identity and answers with it, so the caller does
    /// not have to invent one that is unique. Prefer this: an identity a caller
    /// invents is a uniqueness problem the caller now owns, and the usual
    /// answers to it — a counter kept somewhere else, a timestamp, a random
    /// number long enough to feel safe — are all worse than the one the store
    /// keeps per table.
    ///
    /// [`record`](Self::record) stays for the case that actually needs it: an
    /// identity that came from outside and means something there, like an order
    /// number or an account id.
    ///
    /// ```
    /// use tessaridb_client::query::Create;
    ///
    /// let query = Create::in_table("memories")
    ///     .set("body", "the user prefers metric units")
    ///     .build()
    ///     .expect("a well-formed query");
    ///
    /// // No identity in the script, because the caller never had one.
    /// assert!(query.script.starts_with("CREATE memories = {"));
    /// assert!(!query.script.contains("memories:"));
    /// ```
    #[must_use]
    pub fn in_table(table: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            id: None,
            fields: BTreeMap::new(),
        }
    }

    /// Give a field a value.
    #[must_use]
    pub fn set(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(name.into(), value.into());
        self
    }

    /// Render the statement and its parameters.
    pub fn build(&self) -> Result<Query, BuildError> {
        if self.fields.is_empty() {
            return Err(BuildError::Incomplete {
                what: "a create needs at least one field",
            });
        }
        let mut binder = Binder::new();
        // The bare table is the whole difference between the two forms, and it
        // is checked as a name here because `record_target` — the only other
        // place that writes a table into a script — is not on this path.
        let target = if let Some(id) = self.id.clone() {
            record_target(&self.table, id, &mut binder)?
        } else {
            check_name("a table", &self.table)?;
            self.table.clone()
        };
        let body = render_object(&self.fields, &mut binder)?;
        Ok(Query {
            script: format!("CREATE {target} = {body};"),
            parameters: binder.finish(),
        })
    }
}

/// Change fields on a record, leaving the rest alone.
#[derive(Debug, Clone)]
pub struct Update {
    table: String,
    id: Value,
    fields: BTreeMap<String, Value>,
}

impl Update {
    /// A record in a table, by identity.
    #[must_use]
    pub fn record(table: impl Into<String>, id: impl Into<Value>) -> Self {
        Self {
            table: table.into(),
            id: id.into(),
            fields: BTreeMap::new(),
        }
    }

    /// Give a field a new value.
    #[must_use]
    pub fn set(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(name.into(), value.into());
        self
    }

    /// Render the statement and its parameters.
    ///
    /// Emits `SET`, which changes the named fields only. The whole-record form
    /// (`UPDATE t:id = { … }`) replaces everything and is not what a builder
    /// with a `set` method should quietly do — a caller who wants a replacement
    /// asks for [`Create`], where the word says so.
    pub fn build(&self) -> Result<Query, BuildError> {
        if self.fields.is_empty() {
            return Err(BuildError::Incomplete {
                what: "an update needs at least one field to set",
            });
        }
        let mut binder = Binder::new();
        let target = record_target(&self.table, self.id.clone(), &mut binder)?;
        let mut assignments = Vec::with_capacity(self.fields.len());
        for (name, value) in &self.fields {
            check_name("a field", name)?;
            let reference = binder.bind(value.clone());
            assignments.push(format!("{name} = {reference}"));
        }
        Ok(Query {
            script: format!("UPDATE {target} SET {};", assignments.join(", ")),
            parameters: binder.finish(),
        })
    }
}

/// Remove a record.
#[derive(Debug, Clone)]
pub struct Delete {
    table: String,
    id: Value,
}

impl Delete {
    /// A record in a table, by identity.
    #[must_use]
    pub fn record(table: impl Into<String>, id: impl Into<Value>) -> Self {
        Self {
            table: table.into(),
            id: id.into(),
        }
    }

    /// Render the statement and its parameters.
    pub fn build(&self) -> Result<Query, BuildError> {
        let mut binder = Binder::new();
        let target = record_target(&self.table, self.id.clone(), &mut binder)?;
        Ok(Query {
            script: format!("DELETE {target};"),
            parameters: binder.finish(),
        })
    }
}

fn render_object(
    fields: &BTreeMap<String, Value>,
    binder: &mut Binder,
) -> Result<String, BuildError> {
    let mut parts = Vec::with_capacity(fields.len());
    for (name, value) in fields {
        check_name("a field", name)?;
        let reference = binder.bind(value.clone());
        parts.push(format!("{name}: {reference}"));
    }
    Ok(format!("{{ {} }}", parts.join(", ")))
}
