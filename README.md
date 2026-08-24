# bgv-db-sdk-rust

The Rust client for **bgv-db**.

This is the library you use to talk to a bgv-db node: run statements, subscribe
to changes, store and fetch files, and check whether a node is healthy — without
writing HTTP by hand.

Licence: **Apache-2.0**. See [`LICENSE`](LICENSE). Permissive, with an explicit
patent grant — the licence a client library you embed in your own product should
carry.

The server is licensed separately. The boundary runs between the two, and this
client depends on the server's *protocol*, never on its code.

---

## Status

**Early.** The wire half works: connect, run statements with bound parameters,
decode every value type, subscribe to changes, and build the four common
statements. The HTTP half — objects, files, backup, health — is next.

It implements **protocol 1.0**: a two-number version where only a differing
major is a refusal, and an outcome kind this build has never seen is stepped over
by its length rather than ending the read.

The boundary was written before any code, on purpose. An SDK's README is where it
is decided what the library owns and what it refuses, and that decision is
expensive to reverse once callers depend on it — so it is made deliberately, in
one place, rather than emerging from whichever module someone writes first.

### Written against the protocol, not against the server

This client implements the published protocol specification and takes **no
dependency on the database's repository** — not by path, not by git, not by a
published crate. Its whole dependency tree is `tokio` and `thiserror`.

That is what lets the same protocol have a client in any language, and it is why
a rename inside the server cannot break a client that never used the renamed
thing.

### It is asynchronous

`async fn` throughout, on Tokio. A change subscription holds a connection open
for as long as you care to listen, which is the workload a thread is the wrong
unit for — and it is the workload this client exists for.

```rust
use bgv_db_sdk::{Client, Follow, Value};

let mut client = Client::connect("127.0.0.1:9080").await?;

let answers = client
    .run_with(
        "SELECT * FROM users WHERE age > $min;",
        None,
        [("min", Value::from(21_i64))],
    )
    .await?;

// A subscription consumes the connection: a socket delivering changes is not
// also answering scripts. Two jobs, two connections.
let mut feed = Client::connect("127.0.0.1:9080")
    .await?
    .follow(&Follow::everything().to_table("users"))
    .await?;

while let Some(change) = feed.next().await? {
    println!("{} {} at {}", change.table, change.id, change.sequence);
}
```

## What this SDK is for

One client, everything the node serves. If a bgv-db node answers it, this SDK
reaches it, and no caller should ever hand-roll an HTTP request to use a surface
the node already exposes.

```rust
let db = bgv_db_sdk::Client::connect("bgv://localhost:9080")?;

let adults = db
    .query("SELECT * FROM users WHERE age > $min;")
    .bind("min", 21)
    .fetch::<User>()?;
```

## It speaks two transports, and that is not a design preference

A bgv-db node serves two surfaces, and **neither one carries everything**:

| | wire protocol | HTTP |
|---|---|---|
| statements and parameters | yes | yes |
| change subscription | yes | transport only, for now |
| objects and files | — | yes |
| backup | — | yes |
| health, readiness, metrics | — | yes |

That table makes an HTTP-only client look like the obvious choice: it reaches
every route. It is the trap. HTTP answers are JSON, which carries **six** types,
while the wire protocol carries the store's full model of **seventeen**. An
HTTP-only SDK would therefore work, reach everything, and quietly narrow every
statement result — invisibly, because a value that has been through JSON is
still a perfectly valid value, and nothing at the call site shows what was lost.

The mirror mistake is just as available: a wire-only SDK keeps every type and
cannot store a file.

So this SDK uses **both**, and which transport carries an operation is fixed:

- **statements and subscriptions → the wire protocol**, for type fidelity;
- **objects, files, backup, health, readiness, metrics → HTTP**, because nothing
  else serves them.

You never choose a transport per call. But the two connections stay separately
configurable and separately reportable, because they are two ports: a firewall
rule, a service-mesh policy, or a partial bind can leave one reachable and the
other not. "Files work but queries don't" should be a state you can diagnose,
not a mystery.

## What it owns

- **Connecting and the session** — addresses, timeouts, reconnection.
- **Authentication** — credentials per call, or held for the session.
- **Retry**, with its boundary stated: transport failures are retried; a
  statement that reached the store and failed *there* is not, because the SDK
  cannot know it was safe to repeat.
- **The change subscription** — what changed, delivered as it happens.
- **Typed rows** — answers mapped into your own structs.
- **Objects and files** — put, get, byte ranges, list, delete.
- **The operational routes** — health, readiness, metrics.
- **The query builder** — re-exported from here, not a second dependency you are
  told to also add.

## What it does not own

**The language.** Statements are bgvQL. This SDK does not invent a second way to
express them. The query builder is not a dialect: it produces the same grammar
the server parses, and that is checked by round-tripping built queries through
the server's own parser rather than by reading them.

**The catalog.** The SDK cannot tell you that a table exists, that a field is
indexed, or that your types match the schema. That knowledge lives on the
server. A client-side validator would be a promise that holds in every test and
fails in production.

## The query builder

Typed and composable, resting on one guarantee: **it never puts a value into the
query text**.

```rust
let q = Select::from("users")
    .filter(field("age").gt(21).and(field("active").eq(true)))
    .order_by("created", Desc)
    .limit(50);
```

Values bind as parameters *after* the statement is parsed, so a string
containing `'; DROP TABLE users; --` is text that reads alarmingly and does
nothing at all. A builder that formatted values into the query would destroy the
one property this store's whole surface is designed around — and would do it
invisibly, because the output still looks correct.

An incomplete query does not compile: a `Select` with no source has no `build`.

## Branches

- **`main`** — stable.
- **`dev`** — integration; work lands here first.

Matching the layout of the `bgv-db` repository.

## Building the surface

Each capability arrives with a test against a **running node**, not a mock. A
mock proves the SDK agrees with its author's belief about the protocol, which is
precisely the belief most likely to be wrong.
