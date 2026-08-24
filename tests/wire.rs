//! The framing, the greeting, and one conversation end to end.
//!
//! Driven over an in-memory duplex rather than a socket: this tier is about the
//! bytes this client writes and the bytes it accepts, and a loopback TCP
//! connection would add scheduling without adding evidence. Exercising a running
//! node is the stronger tier and is owed separately.

// Test assertions are exactly where a panic is the correct outcome; the lints
// these turn off target production paths, where a panic is a defect.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]
use bgv_db_sdk::protocol::{CEILING, GREETING, VERSION};
use bgv_db_sdk::wire::frame::{self, Kind};
use bgv_db_sdk::wire::message::decode_answers;
use bgv_db_sdk::{Answer, Client, Error, Number, Request, Value};
use tokio::io::AsyncWriteExt;

/// An answer frame body carrying `count` outcomes.
fn answer_body(outcomes: &[u8], count: u32) -> Vec<u8> {
    let mut body = count.to_be_bytes().to_vec();
    body.extend_from_slice(outcomes);
    body
}

#[tokio::test]
async fn a_frame_round_trips_through_the_wire() {
    let (mut a, mut b) = tokio::io::duplex(4096);
    frame::write(&mut a, Kind::Request, b"hello")
        .await
        .expect("a small frame should write");
    let (kind, body) = frame::read(&mut b)
        .await
        .expect("a written frame should read")
        .expect("and should not be a clean goodbye");
    assert_eq!(kind, Kind::Request);
    assert_eq!(body, b"hello");
}

#[tokio::test]
async fn a_clean_hangup_between_frames_is_not_an_error() {
    let (a, mut b) = tokio::io::duplex(64);
    drop(a);
    let read = frame::read(&mut b)
        .await
        .expect("a clean close is not an error");
    assert!(read.is_none(), "nothing at all is a goodbye, not a failure");
}

#[tokio::test]
async fn a_partial_header_is_truncation_and_not_a_goodbye() {
    let (mut a, mut b) = tokio::io::duplex(64);
    a.write_all(&[Kind::Request.tag(), 0, 0])
        .await
        .expect("write");
    drop(a);
    let error = frame::read(&mut b)
        .await
        .expect_err("a partial header must fail");
    assert!(matches!(error, Error::Truncated), "got {error:?}");
}

#[tokio::test]
async fn a_declared_length_above_the_ceiling_is_refused_before_reading_a_body() {
    let (mut a, mut b) = tokio::io::duplex(64);
    let mut header = vec![Kind::Answer.tag()];
    header.extend_from_slice(&CEILING.saturating_add(1).to_be_bytes());
    a.write_all(&header).await.expect("write");
    // Deliberately no body follows. If the reader allocated on the claim and
    // then waited for bytes, this test would hang rather than fail.
    let error = frame::read(&mut b)
        .await
        .expect_err("an oversized claim must be refused");
    match error {
        Error::TooLarge { length } => assert_eq!(length, CEILING.saturating_add(1)),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_frame_kind_ends_the_connection_rather_than_being_skipped() {
    let (mut a, mut b) = tokio::io::duplex(64);
    a.write_all(&[99, 0, 0, 0, 0]).await.expect("write");
    let error = frame::read(&mut b)
        .await
        .expect_err("an unknown kind must not be skipped");
    match error {
        Error::UnknownFrame { tag } => assert_eq!(tag, 99),
        other => panic!("expected UnknownFrame, got {other:?}"),
    }
}

#[tokio::test]
async fn a_greeting_from_something_that_is_not_a_node_is_refused() {
    let (mut ours, mut theirs) = tokio::io::duplex(64);
    tokio::spawn(async move {
        theirs.write_all(b"HTTP/").await.ok();
    });
    let error = frame::greet(&mut ours)
        .await
        .expect_err("a foreign greeting must be refused");
    assert!(matches!(error, Error::NotThisProtocol), "got {error:?}");
}

#[tokio::test]
async fn a_node_of_another_version_is_refused_at_the_greeting() {
    let (mut ours, mut theirs) = tokio::io::duplex(64);
    tokio::spawn(async move {
        let mut said = GREETING.to_vec();
        said.push(VERSION.saturating_add(1));
        theirs.write_all(&said).await.ok();
    });
    let error = frame::greet(&mut ours)
        .await
        .expect_err("a version mismatch must be refused here, not later");
    match error {
        Error::WrongVersion { found, supported } => {
            assert_eq!(found, VERSION.saturating_add(1));
            assert_eq!(supported, VERSION);
        }
        other => panic!("expected WrongVersion, got {other:?}"),
    }
}

#[tokio::test]
async fn a_script_is_sent_and_its_answers_read_back() {
    let (ours, mut theirs) = tokio::io::duplex(4096);

    let node = tokio::spawn(async move {
        // Read what the client sent, so the test proves the request reached the
        // wire rather than only that a reply could be parsed.
        let (kind, body) = frame::read(&mut theirs)
            .await
            .expect("the client should send a frame")
            .expect("and it should not be a goodbye");
        assert_eq!(kind, Kind::Request);

        // Two outcomes: Done, then Removed(7).
        let mut outcomes = vec![0_u8];
        outcomes.push(4);
        outcomes.extend_from_slice(&7_u64.to_be_bytes());
        frame::write(&mut theirs, Kind::Answer, &answer_body(&outcomes, 2))
            .await
            .expect("the node should answer");
        body
    });

    let mut client = Client::with_stream(ours);
    let answers = client
        .run("DELETE user WHERE age < $limit;", None)
        .await
        .expect("the exchange should succeed");
    assert_eq!(answers, vec![Answer::Done, Answer::Removed(7)]);

    let sent = node.await.expect("the node task should finish");
    let text = String::from_utf8_lossy(&sent);
    assert!(
        text.contains("DELETE user WHERE age < $limit;"),
        "the script should reach the wire verbatim"
    );
}

#[tokio::test]
async fn a_bound_parameter_travels_as_bytes_and_never_as_script_text() {
    // The value is a string that spells a statement. If it were interpolated
    // into the script this test would find it there.
    let hostile = "'; DROP TABLE users; --";
    let request = Request::new("SELECT * FROM users WHERE name = $name;").bind("name", hostile);
    let body = request.encode();
    let text = String::from_utf8_lossy(&body);

    assert!(
        text.contains("SELECT * FROM users WHERE name = $name;"),
        "the script itself should be present"
    );
    // The parameter's bytes are in the frame — that is where a value belongs —
    // but the script portion must not have grown to contain it. The script is
    // length-prefixed and first, so its own bytes end before the parameters.
    let header = body.get(..4).expect("the body opens with a length");
    let script_len =
        u32::from_be_bytes(<[u8; 4]>::try_from(header).expect("four bytes are four bytes"));
    let script_len = usize::try_from(script_len).expect("a length fits a usize here");
    let script = body
        .get(4..4_usize.saturating_add(script_len))
        .map(String::from_utf8_lossy)
        .expect("the script occupies the length it declared");
    assert!(
        !script.contains("DROP TABLE"),
        "a bound value must not reach the script text; script was {script:?}"
    );
}

#[tokio::test]
async fn a_refusal_carries_the_nodes_own_words() {
    let (ours, mut theirs) = tokio::io::duplex(1024);
    tokio::spawn(async move {
        let _ = frame::read(&mut theirs).await;
        frame::write(&mut theirs, Kind::Refusal, b"no such table: widgets")
            .await
            .ok();
    });
    let mut client = Client::with_stream(ours);
    let error = client
        .run("SELECT * FROM widgets;", None)
        .await
        .expect_err("a refusal should surface");
    match error {
        Error::Refused { message } => assert_eq!(message, "no such table: widgets"),
        other => panic!("expected Refused, got {other:?}"),
    }
}

#[test]
fn an_unknown_outcome_stops_the_read_instead_of_producing_garbage() {
    // Three outcomes claimed: Done, then an unknown tag whose payload happens to
    // look like a valid Removed, then a real Done. A decoder that carried on
    // would read the payload as the next outcome and hand back invented values.
    let mut outcomes = vec![0_u8, 0xEE];
    outcomes.extend_from_slice(&99_u64.to_be_bytes());
    outcomes.push(0);
    let answers = decode_answers(&answer_body(&outcomes, 3)).expect("this should decode");
    assert_eq!(
        answers,
        vec![Answer::Done, Answer::Unknown],
        "the read must stop at the unknown outcome, with it last"
    );
}

#[test]
fn a_value_outcome_carries_the_names_its_references_need() {
    let mut outcomes = vec![2_u8];
    // one name: table 3 is "users"
    outcomes.extend_from_slice(&1_u32.to_be_bytes());
    outcomes.extend_from_slice(&3_u32.to_be_bytes());
    outcomes.extend_from_slice(&5_u32.to_be_bytes());
    outcomes.extend_from_slice(b"users");
    // the value: integer 1
    let value = bgv_db_sdk::codec::encode(&Value::Number(Number::Integer(1)));
    let value_len = u32::try_from(value.len()).expect("the test value is small");
    outcomes.extend_from_slice(&value_len.to_be_bytes());
    outcomes.extend_from_slice(&value);

    let answers = decode_answers(&answer_body(&outcomes, 1)).expect("this should decode");
    match answers.first() {
        Some(Answer::Value { value, names }) => {
            assert_eq!(value, &Value::Number(Number::Integer(1)));
            assert_eq!(names.get(&3).map(String::as_str), Some("users"));
        }
        other => panic!("expected a Value outcome, got {other:?}"),
    }
}
