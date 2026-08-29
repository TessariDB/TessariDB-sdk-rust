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
//! The deciding argument, though, is coherence. This crate has twice refused a
//! dependency at the cost of caller convenience, and said so in prose that ships:
//! no `serde`, and no derive macro for row mapping, both argued on the tree being
//! `tokio` + `thiserror`. Taking ten crates for HTTP after refusing one for a
//! derive macro would make that posture decoration rather than a commitment.
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

mod reply;

use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{Error, Result};

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

    /// Issue a GET and insist on a successful status.
    async fn get(&self, path: &str) -> Result<Reply> {
        let reply = self.send("GET", path).await?;
        if !(200..300).contains(&reply.status) {
            // The status is carried into the message rather than mapped onto a
            // variant per code. A caller acts on the distinction between "the
            // node said no" and "the network said no", and this crate already
            // draws that line at `Error::Refused`.
            return Err(Error::Refused {
                message: format!(
                    "the node answered {} for {path}: {}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body).trim()
                ),
            });
        }
        Ok(reply)
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
