//! Reading.

use crate::query::{Binder, BuildError, Filter, Query, check_name};

/// Which way an ordering runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Smallest first. The node's default, and written out anyway so the
    /// statement says what it does.
    Ascending,
    /// Largest first.
    Descending,
}

impl Order {
    const fn spelled(self) -> &'static str {
        match self {
            Self::Ascending => "ASC",
            Self::Descending => "DESC",
        }
    }
}

/// A read.
///
/// ```
/// use bgv_db_sdk::query::{Order, Select, field};
///
/// let query = Select::from("memories")
///     .filter(field("session").eq("abc"))
///     .order_by("created", Order::Descending)
///     .limit(50)
///     .build()
///     .expect("a well-formed query");
///
/// assert!(query.script.starts_with("SELECT * FROM memories WHERE"));
/// // The value is bound, not written into the text.
/// assert!(!query.script.contains("abc"));
/// ```
#[derive(Debug, Clone)]
pub struct Select {
    source: String,
    projection: Vec<String>,
    filter: Option<Filter>,
    order: Vec<(String, Order)>,
    start: Option<u64>,
    limit: Option<u64>,
}

impl Select {
    /// Read from a table.
    #[must_use]
    pub fn from(table: impl Into<String>) -> Self {
        Self {
            source: table.into(),
            projection: Vec::new(),
            filter: None,
            order: Vec::new(),
            start: None,
            limit: None,
        }
    }

    /// Name a field to return.
    ///
    /// With none named the statement projects everything.
    #[must_use]
    pub fn field(mut self, name: impl Into<String>) -> Self {
        self.projection.push(name.into());
        self
    }

    /// Keep only records the condition holds for.
    ///
    /// Calling this twice replaces the condition rather than combining — use
    /// [`Filter::and`] to say what you mean, because a builder that silently
    /// `AND`ed two filters would make a duplicated call look like it worked.
    #[must_use]
    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Order by a field. Called more than once, orders by each in turn.
    #[must_use]
    pub fn order_by(mut self, name: impl Into<String>, order: Order) -> Self {
        self.order.push((name.into(), order));
        self
    }

    /// Skip this many records.
    #[must_use]
    pub const fn start(mut self, at: u64) -> Self {
        self.start = Some(at);
        self
    }

    /// Return at most this many.
    #[must_use]
    pub const fn limit(mut self, count: u64) -> Self {
        self.limit = Some(count);
        self
    }

    /// Render the statement and its parameters.
    pub fn build(&self) -> Result<Query, BuildError> {
        check_name("a table", &self.source)?;
        let mut binder = Binder::new();

        let projection = if self.projection.is_empty() {
            "*".to_owned()
        } else {
            for name in &self.projection {
                check_name("a field", name)?;
            }
            self.projection.join(", ")
        };

        let mut script = format!("SELECT {projection} FROM {}", self.source);

        if let Some(filter) = &self.filter {
            let rendered = filter.render(&mut binder)?;
            script.push_str(" WHERE ");
            script.push_str(&rendered);
        }

        if !self.order.is_empty() {
            let mut parts = Vec::with_capacity(self.order.len());
            for (name, order) in &self.order {
                check_name("a field", name)?;
                parts.push(format!("{name} {}", order.spelled()));
            }
            script.push_str(" ORDER BY ");
            script.push_str(&parts.join(", "));
        }

        // A count is a literal, not a value: it is part of the statement's shape
        // and cannot be supplied by a parameter. It is a `u64` from the type
        // system, so there is nothing here a caller could smuggle syntax through.
        if let Some(at) = self.start {
            script.push_str(" START ");
            script.push_str(&at.to_string());
        }
        if let Some(count) = self.limit {
            script.push_str(" LIMIT ");
            script.push_str(&count.to_string());
        }

        script.push(';');
        Ok(Query {
            script,
            parameters: binder.finish(),
        })
    }
}
