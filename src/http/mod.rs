//! The node's other surface.
//!
//! # Why there are two transports and a caller never picks
//!
//! A node serves two surfaces and neither carries everything. Statements and
//! subscriptions go over the wire protocol, because it carries the store's full
//! model of seventeen value types. Objects, files, backup, health, readiness and
//! metrics go over **HTTP**, because nothing else serves them.
//!
//! Routing statements over HTTP would work, reach everything, and silently
//! narrow every result — JSON carries six types — with nothing at the call site
//! to show what was lost. So the choice is forced by the operation rather than
//! offered as a preference (LR-SDK-005).
//!
//! # Why this client is written rather than taken
//!
//! The framing was **measured** against the shipped node before this was
//! written: every route answers with `Content-Length`, and not one uses
//! `Transfer-Encoding: chunked` — including `/backup`, the only plausible
//! streaming candidate. That is what makes a written client tractable instead of
//! reckless.
//!
//! The deciding argument, though, is that the framing is a **closed** grammar:
//! five routes, one way of declaring length, and no case this client has not
//! been shown. Ten crates to read something that small would be a poor trade
//! against a crate that keeps its dependencies few on purpose.
//!
//! The JSON those routes answer with went the other way, and the contrast is the
//! rule rather than an exception to it: its strings carry arbitrary user text,
//! so it is **open**, and `serde_json` is a dependency (ADR-SDK-0004). What is
//! avoided here is an avoidable dependency, not every dependency.
//!
//! What is **not** claimed is that this is a general HTTP client. It speaks to
//! one server, whose framing is known, and it refuses what it has not been shown
//! rather than guessing — the same discipline as the wire half, where an unknown
//! frame kind ends the connection instead of being skipped.
//!
//! # There is no TLS here either
//!
//! As on the wire protocol. A node belongs on a network you protect or behind
//! something that terminates TLS.

mod condition;
mod reply;

use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{Error, Result};

pub use crate::http::condition::Condition;
use crate::http::reply::Reply;

/// The node's operational surface.
///
/// Holds an address rather than a connection: these calls are occasional and
/// independent, so a connection per call costs nothing worth keeping a pool for
/// — and a pooled connection to a node that restarted is a failure at the next
/// call rather than at the one that should have had it.
#[derive(Debug, Clone)]
pub struct Operations {
    address: String,
}

impl Operations {
    /// The operational surface of the node at this address.
    ///
    /// This is the node's **HTTP** address, which is not its wire address: a
    /// node serves them on separate ports and may serve only one.
    pub fn at(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }

    /// What the node reports about itself, in the Prometheus text format.
    ///
    /// Returned as the node wrote it. Parsing it here would mean this crate
    /// deciding which metrics matter and re-shaping them, and every metric the
    /// node adds would be one this client hides until it is taught about it.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] naming the status when the node answers with one that
    /// is not a success, and whatever the transport reports otherwise.
    pub async fn metrics(&self) -> Result<String> {
        let reply = self.get("/metrics").await?;
        String::from_utf8(reply.body).map_err(|_| Error::Malformed)
    }

    /// Whether the store behind this node is well.
    ///
    /// A failing liveness probe means *restart this node*. That is a different
    /// instruction from a failing readiness probe, which is why this is not
    /// [`ready`](Self::ready) under another name: on a healthy node the two
    /// agree, and they are worth separating precisely where they stop agreeing.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] if the node reports a condition this build does not
    /// know, and whatever the transport reports otherwise. A node that says it
    /// is **unwell** has answered the question, and arrives as
    /// [`Condition::Unwell`] rather than as an error.
    pub async fn health(&self) -> Result<Condition> {
        self.condition("/health").await
    }

    /// Whether this node will take new work now.
    ///
    /// A failing readiness probe means *stop sending traffic here* — the node is
    /// still running and may be perfectly well. During a staged shutdown it
    /// answers [`Condition::Leaving`] while [`health`](Self::health) still
    /// answers [`Condition::Ok`], which is the moment the two routes exist for.
    ///
    /// # Errors
    ///
    /// As [`health`](Self::health).
    pub async fn ready(&self) -> Result<Condition> {
        self.condition("/ready").await
    }

    /// Ask one of the condition routes, where 503 is an answer.
    ///
    /// These two routes report a refusal to serve *as their content*, so the
    /// usual "any non-2xx is a failure" rule is wrong here: a caller handed
    /// `Err` for a node that said "I am not ready" would have to read the
    /// message to tell it apart from a wrong address. Every other status is
    /// still a refusal.
    async fn condition(&self, path: &str) -> Result<Condition> {
        let reply = self.send("GET", path).await?;
        if reply.status != 200 && reply.status != 503 {
            return Err(Self::refusal(path, &reply));
        }
        Condition::read(&reply.body)
    }

    /// Issue a GET and insist on a successful status.
    async fn get(&self, path: &str) -> Result<Reply> {
        let reply = self.send("GET", path).await?;
        if !(200..300).contains(&reply.status) {
            return Err(Self::refusal(path, &reply));
        }
        Ok(reply)
    }

    /// The node said no.
    ///
    /// The status is carried into the message rather than mapped onto a variant
    /// per code. A caller acts on the distinction between "the node said no" and
    /// "the network said no", and this crate already draws that line at
    /// [`Error::Refused`].
    fn refusal(path: &str, reply: &Reply) -> Error {
        Error::Refused {
            message: format!(
                "the node answered {} for {path}: {}",
                reply.status,
                String::from_utf8_lossy(&reply.body).trim()
            ),
        }
    }

    /// Connect, write one request, read one response, drop the connection.
    async fn send(&self, method: &str, path: &str) -> Result<Reply> {
        let mut stream = TcpStream::connect(&self.address).await?;
        stream.set_nodelay(true)?;

        // `Host` is required of an HTTP/1.1 request, and `Connection: close`
        // says this connection carries one exchange — which is true, and saying
        // so lets the node release it rather than hold it open for a reuse that
        // is not coming.
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.address
        );
        stream.write_all(request.as_bytes()).await?;
        stream.flush().await?;

        let mut reader = BufReader::new(stream);
        // `expects_body` is the method's property, not the response's: a HEAD
        // answers with the `Content-Length` a GET would carry and sends nothing
        // after the headers, so a reader that trusts the header waits forever.
        reply::read(&mut reader, method != "HEAD").await
    }
}
