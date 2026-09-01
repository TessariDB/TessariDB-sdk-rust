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

mod basic;
mod condition;
mod object;
mod reply;

use serde_json::Value as Json;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{Error, Result};

pub use crate::http::condition::Condition;
pub use crate::http::object::Bucket;
use crate::http::reply::Reply;

/// The node's operational surface.
///
/// Holds an address rather than a connection: these calls are occasional and
/// independent, so a connection per call costs nothing worth keeping a pool for
/// — and a pooled connection to a node that restarted is a failure at the next
/// call rather than at the one that should have had it.
#[derive(Clone)]
pub struct Operations {
    address: String,
    credential: Option<String>,
}

/// Written rather than derived, because a derived one prints the credential.
///
/// The header is base64, which is a **spelling** of the password and not a
/// disguise for it — anything that reads the line reads the password. A derived
/// `Debug` therefore puts a working credential into any log, panic message or
/// test failure that formats one of these, and into every [`Bucket`] too, since
/// a bucket holds one of these and derives its own.
///
/// So the presence of a credential is reported and its value never is. Presence
/// is worth reporting: *sent no credential* is the single most likely cause of a
/// `401` a caller is looking at, and hiding the field entirely would remove the
/// one thing the output is being read for.
impl std::fmt::Debug for Operations {
    fn fmt(&self, form: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        form.debug_struct("Operations")
            .field("address", &self.address)
            .field(
                "credential",
                match self.credential {
                    Some(_) => &"<present>",
                    None => &"<none>",
                },
            )
            .finish()
    }
}

impl Operations {
    /// The operational surface of the node at this address.
    ///
    /// This is the node's **HTTP** address, which is not its wire address: a
    /// node serves them on separate ports and may serve only one.
    ///
    /// No credential is sent. That is not a placeholder to be filled in later —
    /// a store with no user runs anything, and against one of those an absent
    /// header is the correct request. Add credentials with
    /// [`as_user`](Self::as_user) when the store has a user.
    pub fn at(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            credential: None,
        }
    }

    /// Present these credentials on every request this handle makes.
    ///
    /// Taken once and held, rather than passed at each call. A credential per
    /// call is a secret at every call site and one of them eventually goes
    /// without — and a request that silently omits the header does not fail
    /// loudly, it comes back `401` on a closed store and `400` on an open one,
    /// which are two different-looking bugs with one cause.
    ///
    /// The header is built here, so the plaintext password stops existing inside
    /// this crate the moment the handle is made.
    ///
    /// # A closed store and an open one refuse differently
    ///
    /// Measured against the shipped node: on a **closed** store an
    /// uncredentialed call is `401` with `WWW-Authenticate`, a wrong password is
    /// also `401`, and a right credential reaching outside the user's grants is
    /// `403`. On an **open** store the identical call is `400`. The status
    /// follows the store's posture rather than the call, which is why `401` and
    /// `403` are kept apart: a `401` is worth presenting a credential for and a
    /// `403` never is, however many times it is retried.
    #[must_use]
    pub fn as_user(mut self, name: &str, password: &str) -> Self {
        self.credential = Some(basic::header(name, password));
        self
    }

    /// The bucket of this name, in this database, in this namespace.
    ///
    /// Nothing is checked here and nothing is reached: the three names are
    /// carried to the node, which is the only party that knows whether they
    /// exist. A bucket must have been declared with `DEFINE BUCKET` — a table of
    /// the same name is refused, and refused by the node rather than guessed at
    /// here (LR-SDK-004).
    pub fn bucket(
        &self,
        namespace: impl Into<String>,
        database: impl Into<String>,
        name: impl Into<String>,
    ) -> Bucket {
        Bucket::new(self.clone(), namespace, database, name)
    }

    /// What the node reports about itself, in the Prometheus text format.
    ///
    /// Returned as the node wrote it. Parsing it here would mean this crate
    /// deciding which metrics matter and re-shaping them, and every metric the
    /// node adds would be one this client hides until it is taught about it.
    ///
    /// # Errors
    ///
    /// [`Error::HttpRefused`] carrying the status when the node answers with one
    /// that is not a success, and whatever the transport reports otherwise.
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
        let reply = self.send("GET", path, None).await?;
        if reply.status != 200 && reply.status != 503 {
            return Err(refusal(&reply));
        }
        Condition::read(&reply.body)
    }

    /// Issue a GET and insist on a successful status.
    async fn get(&self, path: &str) -> Result<Reply> {
        let reply = self.send("GET", path, None).await?;
        if !(200..300).contains(&reply.status) {
            return Err(refusal(&reply));
        }
        Ok(reply)
    }

    /// Connect, write one request, read one response, drop the connection.
    ///
    /// `body` is `None` for a request that carries none, which is not the same
    /// as `Some(&[])`: an empty file is written by declaring a length of zero,
    /// and a request with no `Content-Length` at all is a different request.
    async fn send(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<Reply> {
        let mut stream = TcpStream::connect(&self.address).await?;
        stream.set_nodelay(true)?;

        // `Host` is required of an HTTP/1.1 request, and `Connection: close`
        // says this connection carries one exchange — which is true, and saying
        // so lets the node release it rather than hold it open for a reuse that
        // is not coming.
        let length = match body {
            Some(bytes) => format!("Content-Length: {}\r\n", bytes.len()),
            None => String::new(),
        };
        // Absent rather than empty when there is no credential. An
        // `Authorization:` with nothing after it is a header the node must then
        // decide what to make of, and the answer it gives to that is not one
        // this client has measured.
        let authorization = match &self.credential {
            Some(value) => format!("Authorization: {value}\r\n"),
            None => String::new(),
        };
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\
             {authorization}{length}\r\n",
            self.address
        );
        stream.write_all(request.as_bytes()).await?;
        if let Some(bytes) = body {
            stream.write_all(bytes).await?;
        }
        stream.flush().await?;

        let mut reader = BufReader::new(stream);
        // `expects_body` is the method's property, not the response's: a HEAD
        // answers with the `Content-Length` a GET would carry and sends nothing
        // after the headers, so a reader that trusts the header waits forever.
        reply::read(&mut reader, method != "HEAD").await
    }
}

/// The node said no, over HTTP, with a status.
///
/// The status is carried as a field rather than folded into the sentence. The
/// protocol enumerates its refusals by code and says a client branches on the
/// code, so a client that offered only prose would be handing the caller the one
/// thing it was told not to parse.
fn refusal(reply: &Reply) -> Error {
    Error::HttpRefused {
        status: reply.status,
        message: sentence(&reply.body),
    }
}

/// The node's sentence, unwrapped from the JSON that carried it.
///
/// Every refusal the protocol answers as JSON is the same single-field object,
/// `{"error": "…"}`. Handing that object to a caller as the error message leaves
/// them reading punctuation, or parsing it a second time — so it is unwrapped
/// here, once.
///
/// The fallback is the body verbatim, for a refusal that is not that shape at
/// all. Something between the caller and the node — a proxy, a gateway — answers
/// in its own words, and those words are worth showing rather than replacing
/// with a sentence this crate invented about a node it never reached.
fn sentence(body: &[u8]) -> String {
    serde_json::from_slice::<Json>(body)
        .ok()
        .and_then(|json| json.get("error").and_then(Json::as_str).map(str::to_owned))
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::Operations;

    /// The password, and the base64 that is only a spelling of it.
    ///
    /// Both are asserted absent: checking for the plaintext alone would pass on
    /// a `Debug` that printed the header, which is the exact thing being
    /// prevented — the header is a working credential, decodable by anything.
    #[test]
    fn debugging_a_handle_does_not_print_the_credential() {
        let handle = Operations::at("127.0.0.1:1").as_user("admin", "s3cr3t pw");
        let shown = format!("{handle:?}");

        assert!(
            !shown.contains("s3cr3t"),
            "the plaintext password must not be formatted; got {shown}"
        );
        assert!(
            !shown.contains("YWRtaW46czNjcjN0IHB3"),
            "nor the header, which decodes back to it; got {shown}"
        );
        assert!(
            shown.contains("<present>"),
            "that a credential is set is worth saying — it is the first thing \
             looked for when a 401 arrives; got {shown}"
        );
        assert!(
            shown.contains("127.0.0.1:1"),
            "the address is not a secret and is the other half of the question; \
             got {shown}"
        );
    }

    /// A bucket derives its `Debug` and holds one of these, so it is the second
    /// way the credential reaches a log and is worth pinning separately.
    #[test]
    fn debugging_a_bucket_does_not_print_the_credential_either() {
        let bucket = Operations::at("127.0.0.1:1")
            .as_user("admin", "s3cr3t pw")
            .bucket("app", "main", "files");
        let shown = format!("{bucket:?}");

        assert!(
            !shown.contains("s3cr3t") && !shown.contains("YWRtaW46czNjcjN0IHB3"),
            "a bucket carries the handle, and formatting it must not carry the \
             credential out with it; got {shown}"
        );
    }

    /// Absence is reported as absence rather than as nothing at all.
    #[test]
    fn a_handle_with_no_credential_says_which_it_is() {
        let shown = format!("{:?}", Operations::at("127.0.0.1:1"));
        assert!(
            shown.contains("<none>"),
            "sending no credential is the most likely cause of a 401, so the \
             output has to distinguish it from a credential it declined to \
             print; got {shown}"
        );
    }
}
