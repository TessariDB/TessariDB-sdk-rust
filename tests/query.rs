//! The query builder: the exact text it emits, and the values it refuses to put there.
//!
//! # What these tests are, and what they are not
//!
//! They **pin the rendering**. A change to how a statement is spelled shows up as
//! a failing string comparison rather than as a difference nobody looks at.
//!
//! They do **not** prove the text parses. That needs the node's own parser, which
//! this client deliberately does not link, and no assertion here can stand in for
//! it — a builder can emit perfectly stable text that the grammar rejects. An
//! acceptance run against a running node is owed and is the only thing that
//! settles it.
//!
//! The one property that *is* proved here is the one the whole surface exists
//! for: a caller's value never becomes part of the statement text.

// Test assertions are exactly where a panic is the correct outcome; the lints
// these turn off target production paths, where a panic is a defect.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use tessaridb_client::Value;
use tessaridb_client::query::{BuildError, Create, Delete, Order, Select, Update, field};

#[test]
fn a_bare_select_projects_everything() {
    let query = Select::from("memories").build().expect("well formed");
    assert_eq!(query.script, "SELECT * FROM memories;");
    assert!(query.parameters.is_empty());
}

#[test]
fn a_select_renders_every_clause_in_order() {
    let query = Select::from("memories")
        .field("body")
        .field("weight")
        .filter(field("session").eq("abc").and(field("weight").gt(3_i64)))
        .order_by("created", Order::Descending)
        .order_by("body", Order::Ascending)
        .start(10)
        .limit(50)
        .build()
        .expect("well formed");

    assert_eq!(
        query.script,
        "SELECT body, weight FROM memories \
         WHERE (session = $p0 AND weight > $p1) \
         ORDER BY created DESC, body ASC START 10 LIMIT 50;"
    );
    assert_eq!(query.parameters.len(), 2);
    assert_eq!(query.parameters["p0"], Value::String("abc".to_owned()));
    assert_eq!(query.parameters["p1"], Value::from(3_i64));
}

#[test]
fn a_condition_tree_is_fully_parenthesised() {
    // The builder does not know the grammar's precedence — it deliberately does
    // not depend on the parser — so it may not assume how AND and OR associate.
    // Without the parentheses this statement would mean something else, and
    // would still run.
    let query = Select::from("t")
        .filter(
            field("a")
                .eq(1_i64)
                .or(field("b").eq(2_i64).and(field("c").eq(3_i64))),
        )
        .build()
        .expect("well formed");

    assert_eq!(
        query.script,
        "SELECT * FROM t WHERE (a = $p0 OR (b = $p1 AND c = $p2));"
    );
}

#[test]
fn every_comparison_has_its_own_spelling() {
    let cases = [
        (field("x").eq(1_i64), "="),
        (field("x").ne(1_i64), "!="),
        (field("x").lt(1_i64), "<"),
        (field("x").le(1_i64), "<="),
        (field("x").gt(1_i64), ">"),
        (field("x").ge(1_i64), ">="),
    ];
    assert_eq!(cases.len(), 6, "the case list lost entries");
    for (filter, spelled) in cases {
        let query = Select::from("t")
            .filter(filter)
            .build()
            .expect("well formed");
        assert_eq!(
            query.script,
            format!("SELECT * FROM t WHERE x {spelled} $p0;")
        );
    }
}

#[test]
fn a_create_binds_its_identity_and_all_its_fields() {
    let query = Create::record("memories", "note-1")
        .set("body", "the user prefers metric units")
        .set("weight", 3_i64)
        .build()
        .expect("well formed");

    // Fields render in name order, because the builder holds them in a map. That
    // makes two equal creates render equal, which is what makes this assertion
    // stable at all.
    assert_eq!(
        query.script,
        "CREATE memories:$p0 = { body: $p1, weight: $p2 };"
    );
    assert_eq!(query.parameters.len(), 3);
}

#[test]
fn a_create_in_a_table_leaves_the_identity_to_the_store() {
    let query = Create::in_table("memories")
        .set("body", "the user prefers metric units")
        .set("weight", 3_i64)
        .build()
        .expect("well formed");

    // The target is the bare table, and the fields therefore number from $p0
    // rather than $p1 — there is no identity ahead of them in the binder.
    assert_eq!(
        query.script,
        "CREATE memories = { body: $p0, weight: $p1 };"
    );
    assert_eq!(query.parameters.len(), 2);
}

#[test]
fn the_generated_form_refuses_a_table_that_is_not_a_name() {
    // The named form gets its check from `record_target`, which this path does
    // not call. A generated form that skipped the check would write a caller's
    // text straight into the statement, where a table position accepts far more
    // than a table name.
    let error = Create::in_table("users; DROP")
        .set("body", 1_i64)
        .build()
        .expect_err("a table is a name");
    assert!(
        matches!(
            error,
            BuildError::NotAName {
                what: "a table",
                ..
            }
        ),
        "got {error:?}"
    );
}

#[test]
fn an_update_sets_named_fields_rather_than_replacing_the_record() {
    // The distinction is the whole reason `Update` emits SET: a builder with a
    // `set` method that quietly replaced the record would destroy every field
    // the caller did not mention, and the statement would look correct.
    let query = Update::record("memories", 7_i64)
        .set("weight", 9_i64)
        .build()
        .expect("well formed");

    assert_eq!(query.script, "UPDATE memories:$p0 SET weight = $p1;");
}

#[test]
fn a_delete_names_one_record() {
    let query = Delete::record("memories", "note-1")
        .build()
        .expect("well formed");
    assert_eq!(query.script, "DELETE memories:$p0;");
    assert_eq!(query.parameters.len(), 1);
}

#[test]
fn a_write_with_no_fields_is_refused_rather_than_rendered_empty() {
    let error = Create::record("t", 1_i64)
        .build()
        .expect_err("a create needs a field");
    assert!(
        matches!(error, BuildError::Incomplete { .. }),
        "got {error:?}"
    );

    let error = Update::record("t", 1_i64)
        .build()
        .expect_err("an update needs a field");
    assert!(
        matches!(error, BuildError::Incomplete { .. }),
        "got {error:?}"
    );
}

#[test]
fn a_name_that_is_not_a_name_is_refused_and_never_quoted() {
    // Quoting would turn a caller's mistake into a statement that runs and means
    // something else. Every position that is interpolated is checked.
    let cases = [
        ("a table", Select::from("users; DROP TABLE x").build()),
        (
            "a field in a projection",
            Select::from("t").field("a b").build(),
        ),
        (
            "a field in a condition",
            Select::from("t").filter(field("a-b").eq(1_i64)).build(),
        ),
        (
            "a field in an ordering",
            Select::from("t").order_by("1st", Order::Ascending).build(),
        ),
    ];
    assert_eq!(
        cases.len(),
        4,
        "every interpolated position must be covered"
    );
    for (what, outcome) in cases {
        match outcome {
            Err(BuildError::NotAName { .. }) => {}
            Err(other) => panic!("{what}: expected NotAName, got {other:?}"),
            Ok(query) => panic!("{what}: expected a refusal, built {:?}", query.script),
        }
    }
}

#[test]
fn a_hostile_value_stays_a_value() {
    // The string spells a statement. If any builder interpolated values, it
    // would appear in the text here.
    let hostile = "'; DROP TABLE users; --";
    let built = [
        Select::from("users")
            .filter(field("name").eq(hostile))
            .build()
            .expect("well formed"),
        Create::record("users", hostile)
            .set("name", hostile)
            .build()
            .expect("well formed"),
        Update::record("users", hostile)
            .set("name", hostile)
            .build()
            .expect("well formed"),
        Delete::record("users", hostile)
            .build()
            .expect("well formed"),
    ];
    assert_eq!(built.len(), 4, "every builder must be covered");
    for query in &built {
        assert!(
            !query.script.contains("DROP TABLE"),
            "a value reached the script text: {:?}",
            query.script
        );
        assert!(
            query
                .parameters
                .values()
                .any(|held| *held == Value::String(hostile.to_owned())),
            "the value should be bound instead: {:?}",
            query.parameters
        );
    }
}
