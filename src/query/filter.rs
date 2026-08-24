//! Conditions, kept deliberately small.
//!
//! Comparison against a bound value, and `AND`/`OR` between them. That is what a
//! filter needs to be useful and it is where a builder stops being simpler than
//! writing the statement out. Anything richer is a script.

use crate::query::{Binder, BuildError, check_name};
use crate::value::Value;

/// Start a condition on a field.
///
/// ```
/// use tessaridb::query::field;
/// let condition = field("age").gt(21_i64).and(field("active").eq(true));
/// ```
#[must_use]
pub fn field(name: impl Into<String>) -> Field {
    Field { name: name.into() }
}

/// A field, waiting for a comparison.
#[derive(Debug, Clone)]
pub struct Field {
    name: String,
}

/// How two things are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compare {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl Compare {
    const fn spelled(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "!=",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
        }
    }
}

impl Field {
    /// Equal to a value.
    #[must_use]
    pub fn eq(self, value: impl Into<Value>) -> Filter {
        self.compare(Compare::Equal, value)
    }

    /// Not equal to a value.
    #[must_use]
    pub fn ne(self, value: impl Into<Value>) -> Filter {
        self.compare(Compare::NotEqual, value)
    }

    /// Less than a value.
    #[must_use]
    pub fn lt(self, value: impl Into<Value>) -> Filter {
        self.compare(Compare::Less, value)
    }

    /// At most a value.
    #[must_use]
    pub fn le(self, value: impl Into<Value>) -> Filter {
        self.compare(Compare::LessOrEqual, value)
    }

    /// Greater than a value.
    #[must_use]
    pub fn gt(self, value: impl Into<Value>) -> Filter {
        self.compare(Compare::Greater, value)
    }

    /// At least a value.
    #[must_use]
    pub fn ge(self, value: impl Into<Value>) -> Filter {
        self.compare(Compare::GreaterOrEqual, value)
    }

    fn compare(self, how: Compare, value: impl Into<Value>) -> Filter {
        Filter(Node::Compare {
            field: self.name,
            how,
            value: value.into(),
        })
    }
}

/// One node of the condition tree.
///
/// Private, and the wrapper below is what keeps it so. Exposing the tree would
/// publish a shape callers never construct or read — every condition is built
/// through [`field`] and consumed by [`Filter::render`] — and would then have to
/// be kept stable for them.
#[derive(Debug, Clone)]
enum Node {
    Compare {
        field: String,
        how: Compare,
        value: Value,
    },
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
}

/// A condition on the records a statement touches.
///
/// Opaque: built with [`field`], combined with [`Filter::and`] and
/// [`Filter::or`], and rendered by the builder that holds it.
#[derive(Debug, Clone)]
pub struct Filter(Node);

impl Filter {
    /// This condition and another.
    #[must_use]
    pub fn and(self, other: Self) -> Self {
        Self(Node::And(Box::new(self.0), Box::new(other.0)))
    }

    /// This condition or another.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        Self(Node::Or(Box::new(self.0), Box::new(other.0)))
    }

    /// Render into statement text, binding every value on the way.
    ///
    /// Fully parenthesised rather than relying on the grammar's precedence. This
    /// builder does not know that grammar — it deliberately does not depend on
    /// the parser — so it does not get to assume how `AND` and `OR` associate.
    /// Parentheses make the tree the caller built the tree that runs.
    pub(crate) fn render(&self, binder: &mut Binder) -> Result<String, BuildError> {
        self.0.render(binder)
    }
}

impl Node {
    fn render(&self, binder: &mut Binder) -> Result<String, BuildError> {
        match self {
            Self::Compare { field, how, value } => {
                check_name("a field", field)?;
                let reference = binder.bind(value.clone());
                Ok(format!("{field} {} {reference}", how.spelled()))
            }
            Self::And(left, right) => {
                let left = left.render(binder)?;
                let right = right.render(binder)?;
                Ok(format!("({left} AND {right})"))
            }
            Self::Or(left, right) => {
                let left = left.render(binder)?;
                let right = right.render(binder)?;
                Ok(format!("({left} OR {right})"))
            }
        }
    }
}
