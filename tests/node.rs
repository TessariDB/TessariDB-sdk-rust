//! The client against a node that is actually running.
//!
//! Every other test in this crate drives a mock: a duplex, or a `tokio::spawn`
//! that writes the bytes this client expects. Those tests are worth having — they
//! pin the framing exactly — but they share one blind spot, and it is the
//! important one. A mock is written from the same understanding of the protocol
//! as the client, by the same author, at the same time. When that understanding
//! is wrong, the mock is wrong in precisely the same direction, and the pair
//! agree with each other all the way to production.
//!
//! So this file exists to be the one tier that can disagree. The node here is the
//! shipped binary, started as a child process, speaking whatever it actually
//! speaks.
//!
//! # Why these are `#[ignore]`
//!
//! `cargo test` must stay hermetic: no binary, no network peer, no environment.
//! These run when asked for, which is also when the binary is known to exist:
//!
//! ```text
//! TESSARIDB_BIN=/path/to/tessaridb cargo test --test node -- --ignored
//! ```
//!
//! # Why the binary comes from the environment
//!
//! This crate depends on the database's repository by no mechanism — no path
//! dependency, no git revision, no vendored source. Naming a binary at run time
//! is a dependency of *this test*, not of the crate, and nothing about it reaches
//! `Cargo.toml`. A client for this protocol has to be writable in a language that
//! cannot link the server at all, and that stays true here.
//!
//! # Why a missing binary fails instead of skipping
//!
//! A test that quietly skips when its environment is absent reports success for a
//! node nobody connected to. That is the same shape as a search that returns zero
//! because it was pointed at nothing, and this project has already been caught by
//! it twice. Asking for these tests and getting a pass must mean a node answered.

// Assertions are exactly where a panic is the correct outcome; these lints target
// production paths, where a panic is a defect.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tessaridb_client::{
    Answer, Became, Change, Client, Feed, Follow, FromRecord, MappingFault, Number, Row, Value,
};

/// How long a node gets to bind its port before the test gives up.
///
/// Bounded rather than open-ended: a wait with no deadline turns "the node did
/// not start" into a suite that hangs, which is the failure that costs the most
/// to diagnose.
const STARTUP_BUDGET: Duration = Duration::from_secs(10);

/// A node started for one test, and stopped when that test ends.
struct Node {
    child: Child,
    address: String,
}

impl Node {
    /// Start the shipped binary on a free port and wait until it answers.
    async fn start() -> Self {
        let binary = std::env::var("TESSARIDB_BIN").unwrap_or_else(|_| {
            panic!(
                "TESSARIDB_BIN is not set.\n\
                 These tests exercise a real node and have nothing to fall back \
                 on. A version of this that skipped here would report success \
                 for a client that never connected to anything, which is worse \
                 than a failure because it looks like evidence.\n\
                 Set it to the shipped binary and run with `--ignored`."
            )
        });

        // Port 0 asks the OS for one that is free, which is what lets these run
        // beside the database's own suites — those bind fixed ports, and a fixed
        // port chosen here would collide with them exactly when both are running.
        let port = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap_or_else(|e| panic!("could not find a free port: {e}"));
            probe.local_addr().unwrap().port()
        };
        let address = format!("127.0.0.1:{port}");

        // The store is in memory: no path argument, so it exists for this child
        // and is gone when it dies. Nothing on disk to clean up, and no way for
        // one test to see another's writes.
        let child = Command::new(&binary)
            .arg("--serve")
            .arg(&address)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("could not start {binary}: {e}"));

        let node = Self { child, address };
        node.wait_until_listening().await;
        node
    }

    /// Poll until the port answers, or fail naming how long was spent.
    async fn wait_until_listening(&self) {
        // Measured as elapsed rather than as a deadline instant: adding a
        // duration to an instant can overflow, and this crate denies bare
        // arithmetic for exactly that class of thing. Elapsed time only compares.
        let started = std::time::Instant::now();
        loop {
            if tokio::net::TcpStream::connect(&self.address).await.is_ok() {
                return;
            }
            assert!(
                started.elapsed() < STARTUP_BUDGET,
                "the node did not accept a connection on {} within {:?}",
                self.address,
                STARTUP_BUDGET
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A client already connected to this node.
    async fn client(&self) -> Client {
        Client::connect(&self.address)
            .await
            .unwrap_or_else(|e| panic!("could not connect to the node at {}: {e}", self.address))
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        // By handle, never by name: the database's own suites run this same
        // binary, and killing by pattern would take theirs down too.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A namespace, a database and a table, so a record has somewhere to live.
///
/// One connection is one session, so the `USE` statements below are still in
/// force for every later statement on the same client — which is what makes this
/// a setup step rather than a prefix every test has to repeat.
const PREAMBLE: &str = "DEFINE NAMESPACE app; USE NAMESPACE app; \
                        DEFINE DATABASE main; USE DATABASE main; \
                        DEFINE TABLE users;";

/// Pull the records out of an answer, or say what arrived instead.
fn records(answer: &Answer) -> &Vec<(String, Value)> {
    match answer {
        Answer::Records { records, .. } => records,
        other => panic!("expected records, and the node answered {other:?}"),
    }
}

/// Read one field of a record's value.
fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    match value {
        Value::Object(fields) => fields
            .get(name)
            .unwrap_or_else(|| panic!("no field {name} in {value:?}")),
        other => panic!("a record should be an object, and this is {other:?}"),
    }
}

#[tokio::test]
#[ignore = "needs the shipped binary; run with --ignored and TESSARIDB_BIN set"]
async fn a_node_that_is_really_running_completes_the_greeting() {
    let node = Node::start().await;

    // Connecting is the assertion. The greeting is where a wrong protocol or a
    // wrong version is refused, so reaching this line at all means the magic, the
    // major and the minor were what this client believes them to be — against the
    // node itself rather than against a fixture repeating the belief back.
    let _client = node.client().await;
}

#[tokio::test]
#[ignore = "needs the shipped binary; run with --ignored and TESSARIDB_BIN set"]
async fn a_script_runs_against_a_real_node_and_its_records_come_back() {
    let node = Node::start().await;
    let mut client = node.client().await;

    let answers = client
        .run(
            &format!("{PREAMBLE} CREATE users:1 = {{ name: 'ada', age: 36 }};"),
            None,
        )
        .await
        .expect("the preamble and the write should be accepted");
    assert_eq!(
        answers.len(),
        6,
        "one answer per statement, in order; got {answers:?}"
    );

    let answers = client
        .run("SELECT * FROM users;", None)
        .await
        .expect("the read should be accepted");
    let found = records(&answers[0]);
    assert_eq!(found.len(), 1, "one record was written; got {found:?}");

    let (id, value) = &found[0];
    assert_eq!(
        id, "1",
        "the identity comes back as the node spells it — bare, not qualified by \
         its table, because the answer already says which table it read"
    );
    assert_eq!(field(value, "name"), &Value::String("ada".to_owned()));
    assert_eq!(field(value, "age"), &Value::Number(Number::Integer(36)));
}

#[tokio::test]
#[ignore = "needs the shipped binary; run with --ignored and TESSARIDB_BIN set"]
async fn a_bound_value_reaches_a_real_node_as_data_and_never_as_script() {
    let node = Node::start().await;
    let mut client = node.client().await;

    client
        .run(PREAMBLE, None)
        .await
        .expect("the preamble should be accepted");

    // Punctuation that would end the statement and begin a destructive one, if
    // this ever became syntax. The mock tests already assert it stays in the
    // parameter map; what they cannot show is what the *node* does with it, and
    // that is the half that matters.
    let hostile = "'; DROP TABLE users; --";
    client
        .run_with(
            "CREATE users:2 = { name: $name };",
            None,
            [("name", Value::from(hostile))],
        )
        .await
        .expect("a bound value is data, so this is an ordinary write");

    let answers = client
        .run("SELECT * FROM users;", None)
        .await
        .expect("the table should still be there — which is the point");
    let found = records(&answers[0]);

    assert_eq!(
        found.len(),
        1,
        "the write landed and the table survives; got {found:?}"
    );
    assert_eq!(
        field(&found[0].1, "name"),
        &Value::String(hostile.to_owned()),
        "the value came back exactly as sent, having never been read as grammar"
    );
}

/// The struct the mapping is aimed at.
///
/// Declared here rather than shared with `mapping.rs`: an integration test
/// binary is its own crate, and a shared fixture between the two would be a
/// third thing to keep honest for no gain.
#[derive(Debug, PartialEq)]
struct User {
    id: String,
    name: String,
    age: i64,
    nickname: Option<String>,
}

impl FromRecord for User {
    fn from_row(mut row: Row) -> Result<Self, MappingFault> {
        Ok(Self {
            id: row.id().to_owned(),
            name: row.take("name")?,
            age: row.take("age")?,
            nickname: row.take("nickname")?,
        })
    }
}

#[tokio::test]
#[ignore = "needs the shipped binary; run with --ignored and TESSARIDB_BIN set"]
async fn a_real_answer_maps_into_a_declared_struct() {
    // The mapping's hermetic tests convert values this process built, which
    // proves the conversions agree with their author. This is the tier that can
    // disagree: the values here were encoded by the node, crossed a socket, and
    // were decoded by the codec (LR-SDK-006).
    let node = Node::start().await;
    let mut client = node.client().await;

    client
        .run(
            &format!(
                "{PREAMBLE} CREATE users:1 = {{ name: 'ada', age: 36, nickname: 'countess' }}; \
                 CREATE users:2 = {{ name: 'grace', age: 45 }};"
            ),
            None,
        )
        .await
        .expect("the preamble and both writes should be accepted");

    let answers = client
        .run("SELECT * FROM users;", None)
        .await
        .expect("the read should be accepted");
    let mut users: Vec<User> = answers
        .into_iter()
        .next()
        .expect("one statement, one answer")
        .records_into()
        .expect("the records map into the declared struct");
    users.sort_by(|left, right| left.id.cmp(&right.id));

    assert_eq!(
        users,
        vec![
            User {
                id: "1".to_owned(),
                name: "ada".to_owned(),
                age: 36,
                nickname: Some("countess".to_owned()),
            },
            User {
                id: "2".to_owned(),
                name: "grace".to_owned(),
                age: 45,
                // Written without the field at all. Whether the node answers
                // with the key missing or with an explicit absent marker, the
                // caller's `Option` gives the same answer — which is the whole
                // reason `FromValue::absent` exists.
                nickname: None,
            },
        ]
    );
}

#[tokio::test]
#[ignore = "needs the shipped binary; run with --ignored and TESSARIDB_BIN set"]
async fn a_field_the_node_did_not_write_comes_back_as_no_key_at_all() {
    // MEASURED 2026-08-30 against the shipped binary: a field that was never
    // written is **absent from the object**. The node does not send the key
    // carrying `Value::None`.
    //
    // That is the fact this whole design turns on. `FromValue::absent` — the
    // default method that lets `Option<T>` answer for a key that is not there —
    // is not an ergonomic nicety here; without it EVERY optional field would
    // report `NoSuchField` against a real node, and `Option` would mean
    // "nullable" while being unable to express "optional".
    //
    // The mapping accepts the other shape too, because accepting it costs
    // nothing and a store may later write an explicit marker. But this
    // assertion pins what is true today, so a change on the node's side fails
    // here loudly instead of quietly altering what `Option` means.
    let node = Node::start().await;
    let mut client = node.client().await;

    client
        .run(
            &format!("{PREAMBLE} CREATE users:1 = {{ name: 'grace' }};"),
            None,
        )
        .await
        .expect("the preamble and the write should be accepted");

    let answers = client
        .run("SELECT * FROM users;", None)
        .await
        .expect("the read should be accepted");
    let found = records(&answers[0]);
    let Value::Object(fields) = &found[0].1 else {
        panic!("a record holds an object; got {:?}", found[0].1);
    };

    assert_eq!(
        fields.get("nickname"),
        None,
        "an unwritten field is absent from the object; the node does not send the \
         key carrying an explicit marker. `FromValue::absent` is what makes \
         `Option` work against that, so this is the assertion that justifies it"
    );

    // The neighbouring field is present, which is what stops the assertion above
    // from passing against a record that came back empty for some other reason.
    assert_eq!(
        fields.get("name"),
        Some(&Value::String("grace".to_owned())),
        "the record did arrive and does carry the field that was written"
    );
}

/// The session context a second connection needs.
///
/// The namespace, database and table are already defined by whoever ran
/// [`PREAMBLE`]; what a new connection lacks is not the schema but the `USE`
/// statements, because one connection is one session.
const USE_CONTEXT: &str = "USE NAMESPACE app; USE DATABASE main;";

/// How long a change gets to arrive before the test says it did not.
///
/// Bounded on purpose: `Feed::next` waits for the node to push, so an
/// unbounded wait would hang the whole suite rather than fail it, and a hung
/// suite reports nothing at all.
const CHANGE_BUDGET: Duration = Duration::from_secs(10);

/// The next change, or a failure naming the budget it outlived.
///
/// The stream type is spelled out because `Feed<S>` carries no default, unlike
/// `Client<S = TcpStream>` beside it. Noted as Q-SDK-12 rather than fixed here:
/// the asymmetry is real and the fix is one word, but it is production API and
/// this wave's request was evidence for S6 (BGV-SURGICAL-001).
async fn next_change(feed: &mut Feed<tokio::net::TcpStream>) -> Change {
    match tokio::time::timeout(CHANGE_BUDGET, feed.next()).await {
        Ok(Ok(Some(change))) => change,
        Ok(Ok(None)) => panic!("the feed ended before the change arrived"),
        Ok(Err(error)) => panic!("the feed failed: {error}"),
        Err(elapsed) => panic!("no change arrived within {CHANGE_BUDGET:?} ({elapsed})"),
    }
}

#[tokio::test]
#[ignore = "needs the shipped binary; run with --ignored and TESSARIDB_BIN set"]
async fn a_change_written_on_another_connection_arrives_on_the_subscription() {
    // The two connections are the criterion, not an implementation detail. A
    // test that subscribes and asserts the subscription was *accepted* passes
    // with no push ever occurring, and one that writes and reads on a single
    // connection may be reading its own echo. Only a change crossing between
    // two sockets shows that the node pushed anything.
    let node = Node::start().await;

    let mut writer = node.client().await;
    writer
        .run(PREAMBLE, None)
        .await
        .expect("the preamble should be accepted");

    // `follow` takes the client by value: a socket delivering changes is not
    // also answering scripts. So the watcher is a second connection, and needs
    // its own session context before it subscribes.
    let mut watcher = node.client().await;
    watcher
        .run(USE_CONTEXT, None)
        .await
        .expect("the watcher's session context should be accepted");
    let mut feed = watcher
        .follow(&Follow::everything().to_table("users"))
        .await
        .expect("the subscription should be accepted");

    writer
        .run("CREATE users:1 = { name: 'ada' };", None)
        .await
        .expect("the write should be accepted");

    let change = next_change(&mut feed).await;

    assert_eq!(change.table, "users");
    assert_eq!(change.id, "1");
    let Became::Written(value) = &change.became else {
        panic!("a CREATE writes; got {:?}", change.became);
    };
    assert_eq!(
        field(value, "name"),
        &Value::String("ada".to_owned()),
        "the pushed value is the one that was written, not merely a notification \
         that something happened"
    );
}

#[tokio::test]
#[ignore = "needs the shipped binary; run with --ignored and TESSARIDB_BIN set"]
async fn the_table_filter_excludes_a_table_it_was_not_given() {
    // Proven by ORDERING rather than by a timeout. The obvious test writes to
    // the excluded table and waits for nothing to arrive — which passes when the
    // filter works and equally when the node is merely slow, and costs its whole
    // budget every run.
    //
    // Here the excluded write goes FIRST and the watched write SECOND. If the
    // filter leaks, the excluded change is already in the feed ahead of the one
    // being waited for, and the assertion fails on it. The positive control is
    // built in: the `users` change must arrive, or the budget fails the test.
    let node = Node::start().await;

    let mut writer = node.client().await;
    writer
        .run(&format!("{PREAMBLE} DEFINE TABLE logs;"), None)
        .await
        .expect("the preamble and the second table should be accepted");

    let mut watcher = node.client().await;
    watcher
        .run(USE_CONTEXT, None)
        .await
        .expect("the watcher's session context should be accepted");
    let mut feed = watcher
        .follow(&Follow::everything().to_table("users"))
        .await
        .expect("the subscription should be accepted");

    writer
        .run("CREATE logs:1 = { line: 'ignored' };", None)
        .await
        .expect("the excluded write should be accepted");
    writer
        .run("CREATE users:1 = { name: 'ada' };", None)
        .await
        .expect("the watched write should be accepted");

    // Read until the watched change appears. `Follow::everything` starts at
    // position 0, so the log is replayed from the beginning and unrelated
    // entries may precede it; what must never appear is the excluded table.
    loop {
        let change = next_change(&mut feed).await;
        assert_ne!(
            change.table, "logs",
            "a subscription narrowed to `users` delivered a change for `logs`; \
             the filter is not filtering"
        );
        if change.table == "users" && change.id == "1" {
            break;
        }
    }
}
