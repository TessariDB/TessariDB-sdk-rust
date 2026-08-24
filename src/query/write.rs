//! Writing.

use std::collections::BTreeMap;

use crate::query::{Binder, BuildError, Query, check_name, record_target};
use crate::value::Value;

/// Write a record, replacing whatever was there.
///
/// ```
/// use bgv_db_sdk::query::Create;
/// use bgv_db_sdk::Value;
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
    id: Value,
    fields: BTreeMap<String, Value>,
}

impl Create {
    /// A record in a table, by identity.
    #[must_use]
    pub fn record(table: impl Into<String>, id: impl Into<Value>) -> Self {
        Self {
            table: table.into(),
            id: id.into(),
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
        let target = record_target(&self.table, self.id.clone(), &mut binder)?;
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
