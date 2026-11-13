// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: an S3 server in memory, for tests of the bucket client over HTTP (TASK-205), whose
//! upstream may be a link of so many bytes a second: what it reads of its connections, a host's
//! uploads, it reads no faster, all its connections sharing it.
//!
//! It serves what branches ask of S3 ([`crate::bucket::conformance`] checks it does), path-style
//! and unsigned: objects' PUTs, conditional on `If-None-Match: *` or `If-Match`, their GETs, whole
//! or of a range, HEADs, DELETEs and DeleteObjects, and ListObjectsV2 by prefix, by delimiter `/`
//! too (TASK-395.5), in one page. An update of no object is a failed precondition, as RustFS and
//! Azure have it. It counts the requests it serves by kind ([`S3Server::requests`]).

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::{Method, Request, Response, StatusCode};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::{Instant, Sleep};

/// The bucket it serves.
pub(crate) const BUCKET: &str = "fxdax";

/// An S3 server, on 127.0.0.1, for as long as the runtime it started on runs.
pub(crate) struct S3Server {
    address: SocketAddr,
    /// Its objects, if it serves them.
    objects: Option<Arc<Mutex<Objects>>>,
}

#[derive(Clone)]
struct Stored {
    bytes: Bytes,
    e_tag: String,
    modified: SystemTime,
}

#[derive(Default)]
struct Objects {
    objects: BTreeMap<String, Stored>,
    /// The number of the last version stored, whose ETag it is.
    versions: u64,
    /// The requests served, by kind.
    requests: BTreeMap<&'static str, u64>,
}

impl Objects {
    /// Counts a request of kind `kind`.
    fn count(&mut self, kind: &'static str) {
        *self.requests.entry(kind).or_default() += 1;
    }
}

impl S3Server {
    /// Starts one on the current runtime, reading at most `rate` bytes a second of its
    /// connections, if any rate. A connection's socket buffers little of what is sent it, so that
    /// a sender feels the link's rate.
    pub(crate) fn start(rate: Option<u64>) -> Self {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(64 << 10).unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = socket.listen(64).unwrap();
        let address = listener.local_addr().unwrap();
        let link = rate.map(|rate| Arc::new(Link { rate, free: Mutex::new(Instant::now()) }));
        let objects = Arc::new(Mutex::new(Objects::default()));
        let served = objects.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let stream = Throttled { stream, link: link.clone(), waiting: None };
                let objects = objects.clone();
                let serve = hyper::service::service_fn(move |request| {
                    let objects = objects.clone();
                    async move { Ok::<_, std::convert::Infallible>(serve(&objects, request).await) }
                });
                tokio::spawn(async move {
                    // A client that gave up on a request hangs up on it: no error of the server's.
                    let connection = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), serve);
                    let _ = connection.await;
                });
            }
        });
        Self { address, objects: Some(served) }
    }

    /// One that something else serves at `address`.
    pub(crate) fn at(address: SocketAddr) -> Self {
        Self { address, objects: None }
    }

    /// The requests it served so far, by kind: `get`, `head`, `put`, `delete`, `delete-objects`
    /// (DeleteObjects), `list` and `list-delimited` (ListObjectsV2, by delimiter or not).
    pub(crate) fn requests(&self) -> BTreeMap<&'static str, u64> {
        let objects = self.objects.as_ref().expect("a server of its own");
        objects.lock().unwrap().requests.clone()
    }

    /// The options of an S3 store of its bucket, `s3://fxdax/...`.
    pub(crate) fn options(&self) -> Vec<(String, String)> {
        [
            ("aws_endpoint", format!("http://{}", self.address)),
            ("aws_allow_http", "true".to_owned()),
            ("aws_skip_signature", "true".to_owned()),
            ("aws_region", "us-east-1".to_owned()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
    }
}

/// A link that carries `rate` bytes a second, whoever sends them.
struct Link {
    rate: u64,
    /// When what it was given so far has gone through.
    free: Mutex<Instant>,
}

impl Link {
    /// Takes `len` bytes: returns when they have gone through.
    fn take(&self, len: usize) -> Instant {
        let mut free = self.free.lock().unwrap();
        *free =
            (*free).max(Instant::now()) + Duration::from_secs_f64(len as f64 / self.rate as f64);
        *free
    }
}

/// The most read of a connection at once over a link.
const READ: usize = 16 << 10;

/// A connection whose reads go through a link, if any: each waits until the one before it went
/// through.
struct Throttled {
    stream: TcpStream,
    link: Option<Arc<Link>>,
    waiting: Option<Pin<Box<Sleep>>>,
}

impl AsyncRead for Throttled {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        let Some(link) = &this.link else {
            return Pin::new(&mut this.stream).poll_read(cx, buf);
        };
        if let Some(waiting) = &mut this.waiting {
            ready!(waiting.as_mut().poll(cx));
            this.waiting = None;
        }
        let mut chunk = [0; READ];
        let len = buf.remaining().min(READ);
        let mut read = ReadBuf::new(&mut chunk[..len]);
        ready!(Pin::new(&mut this.stream).poll_read(cx, &mut read))?;
        buf.put_slice(read.filled());
        if !read.filled().is_empty() {
            this.waiting = Some(Box::pin(tokio::time::sleep_until(link.take(read.filled().len()))));
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Throttled {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

type Answer = Response<Full<Bytes>>;

fn status(status: StatusCode) -> Answer {
    let mut answer = Response::new(Full::default());
    *answer.status_mut() = status;
    answer
}

/// An HTTP date, as Last-Modified has it.
fn http_date(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

async fn serve(objects: &Mutex<Objects>, request: Request<hyper::body::Incoming>) -> Answer {
    let (parts, body) = request.into_parts();
    let header =
        |name: &str| parts.headers.get(name).map(|value| value.to_str().unwrap().to_owned());
    // Keys of no characters an S3 path escapes, as fxdax's are.
    let path = parts.uri.path();
    assert!(!path.contains('%'), "an escaped key: {path}");
    let Some(path) = path.strip_prefix(&format!("/{BUCKET}")) else {
        return status(StatusCode::NOT_FOUND);
    };
    let key = path.trim_start_matches('/');
    let Ok(body) = body.collect().await.map(|body| body.to_bytes()) else {
        return status(StatusCode::BAD_REQUEST);
    };
    let mut objects = objects.lock().unwrap();
    match (&parts.method, key) {
        (&Method::GET, "") => {
            let query = parts.uri.query().unwrap_or("");
            let query: BTreeMap<_, _> = url::form_urlencoded::parse(query.as_bytes()).collect();
            assert_eq!(query.get("list-type").map(|kind| &**kind), Some("2"), "{query:?}");
            let delimited = match query.get("delimiter").map(|delimiter| &**delimiter) {
                None => false,
                Some("/") => true,
                Some(other) => panic!("a listing by delimiter {other:?}"),
            };
            objects.count(if delimited { "list-delimited" } else { "list" });
            let prefix = query.get("prefix").map_or("", |prefix| prefix);
            let mut xml =
                String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>");
            xml += &format!("<Name>{BUCKET}</Name><IsTruncated>false</IsTruncated>");
            // The prefixes of the keys holding the delimiter past the prefix, each once, as their
            // keys are in order: after the objects, as S3 lists them apart.
            let mut prefixes: Vec<&str> = Vec::new();
            for (key, stored) in objects.objects.range(prefix.to_owned()..) {
                if !key.starts_with(prefix) {
                    break;
                }
                if let Some(slash) = key[prefix.len()..].find('/').filter(|_| delimited) {
                    let common = &key[..prefix.len() + slash + 1];
                    if prefixes.last() != Some(&common) {
                        prefixes.push(common);
                    }
                    continue;
                }
                let modified = chrono::DateTime::<chrono::Utc>::from(stored.modified)
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                xml += &format!(
                    "<Contents><Key>{}</Key><LastModified>{modified}</LastModified>\
                     <ETag>{}</ETag><Size>{}</Size></Contents>",
                    escaped(key),
                    escaped(&stored.e_tag),
                    stored.bytes.len()
                );
            }
            for common in prefixes {
                xml += &format!(
                    "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
                    escaped(common)
                );
            }
            xml += "</ListBucketResult>";
            Response::new(Full::new(Bytes::from(xml)))
        }
        (&Method::PUT, key) => {
            objects.count("put");
            let there = objects.objects.get(key);
            let refused = match (header("if-none-match"), header("if-match")) {
                (Some(any), _) => {
                    assert_eq!(any, "*");
                    there.is_some()
                }
                (None, Some(e_tag)) => there.is_none_or(|stored| stored.e_tag != e_tag),
                (None, None) => false,
            };
            if refused {
                return status(StatusCode::PRECONDITION_FAILED);
            }
            objects.versions += 1;
            let e_tag = format!("\"{}\"", objects.versions);
            let stored = Stored { bytes: body, e_tag: e_tag.clone(), modified: SystemTime::now() };
            objects.objects.insert(key.to_owned(), stored);
            Response::builder().header("etag", e_tag).body(Full::default()).unwrap()
        }
        (&Method::GET | &Method::HEAD, key) => {
            objects.count(if parts.method == Method::HEAD { "head" } else { "get" });
            let Some(stored) = objects.objects.get(key) else {
                return status(StatusCode::NOT_FOUND);
            };
            let size = stored.bytes.len();
            let range = header("range").map(|range| {
                let range = range.strip_prefix("bytes=").expect("a range of bytes");
                let (start, end) = range.split_once('-').expect("a range");
                match (start, end) {
                    ("", suffix) => size.saturating_sub(suffix.parse().unwrap())..size,
                    (start, "") => start.parse().unwrap()..size,
                    (start, last) => {
                        start.parse().unwrap()..(last.parse::<usize>().unwrap() + 1).min(size)
                    }
                }
            });
            let answer = Response::builder()
                .header("etag", &stored.e_tag)
                .header("last-modified", http_date(stored.modified));
            let (answer, bytes) = match range {
                None => (answer.status(StatusCode::OK), stored.bytes.clone()),
                Some(range) if range.start >= size => {
                    let answer = answer.status(StatusCode::RANGE_NOT_SATISFIABLE);
                    let answer = answer.header("content-range", format!("bytes */{size}"));
                    return answer.body(Full::default()).unwrap();
                }
                Some(range) => {
                    let answer = answer.status(StatusCode::PARTIAL_CONTENT).header(
                        "content-range",
                        format!("bytes {}-{}/{size}", range.start, range.end - 1),
                    );
                    (answer, stored.bytes.slice(range))
                }
            };
            let answer = answer.header("content-length", bytes.len());
            let bytes = if parts.method == Method::HEAD { Bytes::new() } else { bytes };
            answer.body(Full::new(bytes)).unwrap()
        }
        // DeleteObjects, which `object_store` deletes with.
        (&Method::POST, "") => {
            assert_eq!(parts.uri.query(), Some("delete"));
            objects.count("delete-objects");
            let request = std::str::from_utf8(&body).unwrap();
            let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><DeleteResult>");
            for key in request.split("<Key>").skip(1) {
                let (key, _) = key.split_once("</Key>").expect("a key");
                assert!(!key.contains('&'), "an escaped key: {key}");
                objects.objects.remove(key);
                xml += &format!("<Deleted><Key>{key}</Key></Deleted>");
            }
            xml += "</DeleteResult>";
            Response::new(Full::new(Bytes::from(xml)))
        }
        (&Method::DELETE, key) => {
            objects.count("delete");
            objects.objects.remove(key);
            status(StatusCode::NO_CONTENT)
        }
        _ => status(StatusCode::NOT_IMPLEMENTED),
    }
}
