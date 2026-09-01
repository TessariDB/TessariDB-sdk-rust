//! One bucket, and the three things worth doing to a file in it.
//!
//! # A file name is a value, and the three names above it are not
//!
//! `/files/{ns}/{db}/{bucket}/{path…}` looks like four path segments and is
//! really two different kinds of thing. The namespace, database and bucket are
//! **names**: the node interpolates them into a statement, and refuses anything
//! outside `[A-Za-z0-9_]+` for exactly that reason. The path is a **value**,
//! carried as a parameter, and a file may be called anything at all — spaces,
//! punctuation, slashes that are part of the name rather than directories.
//!
//! That asymmetry is why this module percent-encodes the path and does nothing
//! to the three names. Encoding a name would hide a caller's mistake behind a
//! request the node then refuses for a reason that no longer matches what was
//! typed; leaving the path raw would put a space in an HTTP request line, which
//! does not merely fail — it makes the request unparseable.

use std::fmt::Write as _;

use crate::error::Result;
use crate::http::Operations;

/// A bucket, and the files in it.
///
/// Holds the three names so that a caller writes them once rather than at every
/// call. Obtained from [`Operations::bucket`].
#[derive(Debug, Clone)]
pub struct Bucket {
    node: Operations,
    namespace: String,
    database: String,
    name: String,
}

impl Bucket {
    /// The bucket of this name, in this database, in this namespace.
    pub(crate) fn new(
        node: Operations,
        namespace: impl Into<String>,
        database: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            node,
            namespace: namespace.into(),
            database: database.into(),
            name: name.into(),
        }
    }

    /// Write these bytes to this path, replacing whatever was there.
    ///
    /// The path is taken **literally**. A leading slash is part of the name, so
    /// `"notes.txt"` and `"/notes.txt"` are two files — the node stores a file
    /// name as an opaque value, and a client that quietly tidied one would make
    /// a file unreachable by the name its caller used.
    ///
    /// # Errors
    ///
    /// [`Error::HttpRefused`](crate::Error::HttpRefused) carrying the node's
    /// status — `400` when one of the three names is not an ordinary name or the
    /// bucket is not a bucket, `401` or `403` on a closed store — and whatever
    /// the transport reports otherwise.
    pub async fn put(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let reply = self
            .node
            .send("PUT", &self.route(path), Some(bytes))
            .await?;
        if reply.status != 201 {
            return Err(super::refusal(&reply));
        }
        Ok(())
    }

    /// Read this file, or learn that there is none.
    ///
    /// `None` means the node answered `404`, which on this route is an
    /// **answer**: the file is not there. It is not the same as a file that is
    /// there and empty — that reads back as `Some` of no bytes, because the node
    /// answers it `200` with a declared length of zero. Collapsing the two would
    /// throw away a distinction the node draws.
    ///
    /// # Errors
    ///
    /// As [`put`](Self::put). A `404` is not among them.
    pub async fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let reply = self.node.send("GET", &self.route(path), None).await?;
        match reply.status {
            200 => Ok(Some(reply.body)),
            404 => Ok(None),
            _ => Err(super::refusal(&reply)),
        }
    }

    /// How many bytes this file holds, or that there is none — without
    /// transferring it.
    ///
    /// `None` means the node answered `404`, exactly as in [`get`](Self::get),
    /// and it is not the same as a file that is there and empty: that answers
    /// `Some(0)`. Collapsing the two would throw the distinction away at the one
    /// call where a caller is most likely to be asking about it.
    ///
    /// # It saves the bytes on the wire, and nothing else
    ///
    /// This is a `HEAD`, and the node routes `HEAD` and `GET` to the **same**
    /// handler: it runs the same read, materialises the whole file, and drops the
    /// body when it answers. So a `HEAD` on a large file costs the node what a
    /// `GET` costs it, and only the caller's network is spared.
    ///
    /// Worth knowing before this goes in a loop. If a size is wanted for every
    /// file in a bucket, this is the wrong shape and the right one is a query.
    ///
    /// # Errors
    ///
    /// As [`put`](Self::put). A `404` is not among them. A `200` carrying no
    /// declared length is [`Error::Malformed`](crate::Error::Malformed) rather
    /// than `None`: the node has always declared one, and answering `None` there
    /// would make a file that exists indistinguishable from one that does not,
    /// with nothing at the call site able to tell.
    pub async fn size(&self, path: &str) -> Result<Option<u64>> {
        let reply = self.node.send("HEAD", &self.route(path), None).await?;
        match reply.status {
            200 => reply.length.map(Some).ok_or(crate::Error::Malformed),
            404 => Ok(None),
            _ => Err(super::refusal(&reply)),
        }
    }

    /// Remove this file.
    ///
    /// **Idempotent.** Deleting a file that is not there succeeds, because the
    /// node answers `204` either way and reports no difference between the two.
    /// A client cannot honestly promise more than the node tells it.
    ///
    /// # Errors
    ///
    /// As [`put`](Self::put).
    pub async fn delete(&self, path: &str) -> Result<()> {
        let reply = self.node.send("DELETE", &self.route(path), None).await?;
        if reply.status != 204 {
            return Err(super::refusal(&reply));
        }
        Ok(())
    }

    /// The request target for a file in this bucket.
    fn route(&self, path: &str) -> String {
        format!(
            "/files/{}/{}/{}/{}",
            self.namespace,
            self.database,
            self.name,
            encode(path)
        )
    }
}

/// Percent-encode a file path for a request line.
///
/// Everything outside the unreserved set is encoded, and `/` is kept because a
/// slash inside a file name is carried to the node as a slash — the node
/// percent-decodes what arrives, so an encoded slash would name the same file by
/// a longer route.
///
/// Encoding more than the minimum is safe and deliberate: the node decodes, so
/// an over-encoded byte arrives as itself, whereas an under-encoded one is
/// either lost or changes what the request means. `%` is the character that
/// makes this non-optional — left alone, a file called `100%.txt` asks the node
/// to decode `%.t` as an escape.
fn encode(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(char::from(byte));
            }
            // Writing into a `String` cannot fail — the only error `write!`
            // reports comes from the writer, and this one has none — so the
            // result is discarded rather than dressed up as something handled.
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}
