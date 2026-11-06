// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the HTTP client of buckets in the cloud (TASK-205), which [`crate::store`] gives
//! `object_store`'s S3, GCS and Azure stores. It is `object_store`'s own, reqwest, built from the
//! same options as `object_store` builds it, but for how a request times out.
//!
//! `object_store`'s gives a request 30 s, its body included. A host's uploads, up to 512 MiB of
//! single PUTs at once, share its upstream: below 18 MB/s they all ran out of time together, part
//! sent, and each went again from its first byte, for ever. Here a request runs as long as its
//! bytes move, and times out once they stall: none of its body taken by the connection, nor any
//! of its response read, for [`Client::stall`]. A body is handed to the connection in slices of
//! [`SLICE`] bytes, so that taking each is progress.
//!
//! What the connection's socket buffers it takes, though the link has not carried it yet: its
//! last bytes may wait there, moving nothing, for as long as the link takes to carry the buffer.
//! Over a link that limits the bandwidth the buffer grows with the congestion window, a couple of
//! round trips' and the link's queue's worth: seconds at worst, well within the stall time. A
//! peer that reads slowly lets it grow to the kernel's bound, 4 MiB on Linux, which over
//! loopback, whose segments are 64 KiB, it does at once: a request's share of the bandwidth must
//! carry that within the stall time (the tests' S3 server's reads, `store::tests`).

use bytes::Bytes;
use futures::future::Either;
use hyper::body::{Body, Frame, SizeHint};
use object_store::client::{
    HttpClient, HttpConnector, HttpError, HttpErrorKind, HttpRequest, HttpRequestBody,
    HttpResponse, HttpResponseBody,
};
use object_store::{ClientConfigKey, ClientOptions};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::time::{Instant, Sleep};

/// The slices a request's body goes to its connection in.
pub const SLICE: usize = 16 << 10;

/// The client of buckets: how long a connection may take to open, and a request to move no bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Client {
    pub connect: Duration,
    pub stall: Duration,
}

impl Client {
    /// `object_store`'s connect timeout; S3 closes a connection that moved nothing for 20 s.
    pub const DEFAULT: Self =
        Self { connect: Duration::from_secs(5), stall: Duration::from_secs(30) };

    /// The options it builds stores with, which the environment's may add to: its connect timeout,
    /// and no timeout of a whole request.
    pub(crate) fn options(&self) -> ClientOptions {
        ClientOptions::new().with_connect_timeout(self.connect).with_timeout_disabled()
    }
}

impl HttpConnector for Client {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        Ok(HttpClient::new(Watched { client: reqwest_client(options)?, stall: self.stall }))
    }
}

/// The user agent of a client whose options name none.
const USER_AGENT: &str = concat!("fxdax/", env!("CARGO_PKG_VERSION"));

/// A reqwest client built from `options` as `object_store` builds its own (0.14.2,
/// `ClientOptions::client`, which is its crate's alone), from what the options say of each key.
/// Root certificates, which only `object_store`'s API adds, fxdax adds none of.
fn reqwest_client(options: &ClientOptions) -> object_store::Result<reqwest::Client> {
    use ClientConfigKey as Key;
    let value = |key: Key| options.get_config_value(&key);
    let invalid = |key: Key, error: &dyn std::fmt::Display| object_store::Error::Generic {
        store: "fxdax",
        source: format!("{}: {error}", key.as_ref()).into(),
    };
    let duration = |key: Key| {
        (value(key).map(|value| humantime::parse_duration(&value)).transpose())
            .map_err(|error| invalid(key, &error))
    };
    let flag = |key: Key| match value(key).map(|value| value.to_ascii_lowercase()).as_deref() {
        None | Some("0" | "false" | "off" | "no" | "n") => Ok(false),
        Some("1" | "true" | "on" | "yes" | "y") => Ok(true),
        Some(other) => Err(invalid(key, &format!("{other} is no boolean"))),
    };
    let tls = |error: reqwest::Error| invalid(Key::ProxyCaCertificate, &error);

    let mut builder = reqwest::Client::builder()
        .user_agent(value(Key::UserAgent).unwrap_or_else(|| USER_AGENT.to_owned()))
        // Compression would change Content-Length, which says an object's size.
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
        .https_only(!flag(Key::AllowHttp)?);
    if let Some(headers) = options.get_default_headers() {
        builder = builder.default_headers(headers.clone());
    }
    if let Some(url) = value(Key::ProxyUrl) {
        let mut proxy =
            reqwest::Proxy::all(&url).map_err(|error| invalid(Key::ProxyUrl, &error))?;
        if let Some(pem) = value(Key::ProxyCaCertificate) {
            let certificate = reqwest::Certificate::from_pem(pem.as_bytes()).map_err(tls)?;
            builder = builder.tls_certs_merge([certificate]);
        }
        if let Some(excludes) = value(Key::ProxyExcludes) {
            proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&excludes));
        }
        builder = builder.proxy(proxy);
    }
    if flag(Key::NoSystemCertificates)? {
        builder = builder.tls_certs_only([]);
    }
    if let Some(timeout) = duration(Key::Timeout)? {
        builder = builder.timeout(timeout);
    }
    if let Some(timeout) = duration(Key::ConnectTimeout)? {
        builder = builder.connect_timeout(timeout);
    }
    if let Some(timeout) = duration(Key::ReadTimeout)? {
        builder = builder.read_timeout(timeout);
    }
    if let Some(timeout) = duration(Key::PoolIdleTimeout)? {
        builder = builder.pool_idle_timeout(timeout);
    }
    if let Some(max) = value(Key::PoolMaxIdlePerHost) {
        let max = max.parse().map_err(|error| invalid(Key::PoolMaxIdlePerHost, &error))?;
        builder = builder.pool_max_idle_per_host(max);
    }
    if let Some(interval) = duration(Key::Http2KeepAliveInterval)? {
        builder = builder.http2_keep_alive_interval(interval);
    }
    if let Some(timeout) = duration(Key::Http2KeepAliveTimeout)? {
        builder = builder.http2_keep_alive_timeout(timeout);
    }
    if flag(Key::Http2KeepAliveWhileIdle)? {
        builder = builder.http2_keep_alive_while_idle(true);
    }
    if let Some(size) = value(Key::Http2MaxFrameSize) {
        let size = size.parse().map_err(|error| invalid(Key::Http2MaxFrameSize, &error))?;
        builder = builder.http2_max_frame_size(Some(size));
    }
    if flag(Key::Http1Only)? {
        builder = builder.http1_only();
    }
    if flag(Key::Http2Only)? {
        builder = builder.http2_prior_knowledge();
    }
    if flag(Key::AllowInvalidCertificates)? {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if flag(Key::RandomizeAddresses)? {
        builder = builder.dns_resolver(Arc::new(Shuffled));
    }
    builder
        .build()
        .map_err(|error| object_store::Error::Generic { store: "fxdax", source: Box::new(error) })
}

/// Resolves names to their addresses in a random order, as `object_store` does unless told not
/// to: a host's connections spread over S3's many addresses.
struct Shuffled;

impl reqwest::dns::Resolve for Shuffled {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            use rand::seq::SliceRandom as _;
            let mut addresses: Vec<std::net::SocketAddr> =
                tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            addresses.shuffle(&mut rand::rng());
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A request that moved no bytes for so long.
#[derive(Debug, thiserror::Error)]
#[error("no bytes moved for {0:?}")]
pub struct Stalled(pub Duration);

fn stalled(stall: Duration) -> HttpError {
    HttpError::new(HttpErrorKind::Timeout, Stalled(stall))
}

/// reqwest's client, whose requests time out once they stall.
#[derive(Debug)]
struct Watched {
    client: reqwest::Client,
    stall: Duration,
}

#[async_trait::async_trait]
impl object_store::client::HttpService for Watched {
    async fn call(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let (parts, body) = request.into_parts();
        let url = reqwest::Url::parse(&parts.uri.to_string())
            .map_err(|error| HttpError::new(HttpErrorKind::Unknown, error))?;
        let mut sent = reqwest::Request::new(parts.method, url);
        *sent.headers_mut() = parts.headers;
        let progress = Arc::new(Progress::new());
        let body = Sending { body, slice: Bytes::new(), progress: progress.clone() };
        *sent.body_mut() = Some(reqwest::Body::wrap(body));
        let response = std::pin::pin!(self.client.execute(sent));
        let stalls = std::pin::pin!(progress.stalls(self.stall));
        let response = match futures::future::select(response, stalls).await {
            Either::Left((response, _)) => response.map_err(http_error)?,
            Either::Right(((), _)) => return Err(stalled(self.stall)),
        };
        let (parts, body) = hyper::Response::<reqwest::Body>::from(response).into_parts();
        let body = Receiving { body, stall: self.stall, waiting: None };
        Ok(HttpResponse::from_parts(parts, HttpResponseBody::new(body)))
    }
}

/// What kind of failure a reqwest error is, which says whether `object_store` tries the request
/// again: one whose connection closed under it, as a pooled one the server had closed, goes again
/// whatever it is; one that timed out or was cut short, only if it is idempotent.
fn http_error(error: reqwest::Error) -> HttpError {
    let kind = if error.is_timeout() {
        HttpErrorKind::Timeout
    } else if error.is_connect() {
        HttpErrorKind::Connect
    } else if error.is_decode() {
        HttpErrorKind::Decode
    } else {
        let mut source = std::error::Error::source(&error);
        let mut kind = HttpErrorKind::Unknown;
        while let (HttpErrorKind::Unknown, Some(cause)) = (kind, source) {
            if let Some(hyper) = cause.downcast_ref::<hyper::Error>() {
                if hyper.is_closed()
                    || hyper.is_incomplete_message()
                    || hyper.is_body_write_aborted()
                {
                    kind = HttpErrorKind::Request;
                } else if hyper.is_timeout() {
                    kind = HttpErrorKind::Timeout;
                }
            }
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                use std::io::ErrorKind as Io;
                match io.kind() {
                    Io::TimedOut => kind = HttpErrorKind::Timeout,
                    Io::ConnectionAborted
                    | Io::ConnectionReset
                    | Io::BrokenPipe
                    | Io::UnexpectedEof => kind = HttpErrorKind::Interrupted,
                    _ => {}
                }
            }
            source = cause.source();
        }
        kind
    };
    // Its URL, which may hold a presigned request's credentials, `object_store` says if it may.
    HttpError::new(kind, error.without_url())
}

/// When a request last moved bytes.
struct Progress {
    start: Instant,
    /// Since `start`, in nanoseconds.
    last: AtomicU64,
}

impl Progress {
    fn new() -> Self {
        Self { start: Instant::now(), last: AtomicU64::new(0) }
    }

    fn made(&self) {
        self.last.store(self.start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    /// Returns once no bytes moved for `stall`.
    async fn stalls(&self, stall: Duration) {
        loop {
            let last = Duration::from_nanos(self.last.load(Ordering::Relaxed));
            let idle = self.start.elapsed().saturating_sub(last);
            if idle >= stall {
                return;
            }
            tokio::time::sleep(stall - idle).await;
        }
    }
}

/// A request's body, handed to its connection a slice at a time, each slice taken progress.
struct Sending {
    body: HttpRequestBody,
    /// What is left of the body's frame being sliced.
    slice: Bytes,
    progress: Arc<Progress>,
}

impl Body for Sending {
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let this = &mut *self;
        while this.slice.is_empty() {
            match ready!(Pin::new(&mut this.body).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => this.slice = data,
                    Err(frame) => return Poll::Ready(Some(Ok(frame))),
                },
                end => return Poll::Ready(end),
            }
        }
        this.progress.made();
        let len = this.slice.len().min(SLICE);
        Poll::Ready(Some(Ok(Frame::data(this.slice.split_to(len)))))
    }

    fn is_end_stream(&self) -> bool {
        self.slice.is_empty() && self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        let (body, slice) = (self.body.size_hint(), self.slice.len() as u64);
        let mut hint = SizeHint::new();
        hint.set_lower(body.lower() + slice);
        if let Some(upper) = body.upper() {
            hint.set_upper(upper + slice);
        }
        hint
    }
}

/// A response's body, which fails once none of it came for the stall time while it was waited
/// for: a reader slow to read it stalls nothing.
struct Receiving {
    body: reqwest::Body,
    stall: Duration,
    /// Since the read waiting began, if one is waiting.
    waiting: Option<Pin<Box<Sleep>>>,
}

impl Body for Receiving {
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let this = &mut *self;
        if let Poll::Ready(frame) = Pin::new(&mut this.body).poll_frame(cx) {
            this.waiting = None;
            return Poll::Ready(frame.map(|frame| frame.map_err(http_error)));
        }
        let stall = this.stall;
        let waiting = this.waiting.get_or_insert_with(|| Box::pin(tokio::time::sleep(stall)));
        ready!(waiting.as_mut().poll(cx));
        this.waiting = None;
        Poll::Ready(Some(Err(stalled(stall))))
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt as _;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap()
    }

    /// A body goes in slices of at most [`SLICE`] bytes, its frames' bytes in order, its size
    /// known throughout; each slice taken is progress.
    #[test]
    fn bodies_go_in_slices() {
        runtime().block_on(async {
            let payload: object_store::PutPayload =
                [Bytes::from(vec![1; SLICE * 2 + 5]), Bytes::new(), Bytes::from(vec![2; 3])]
                    .into_iter()
                    .collect();
            let progress = Arc::new(Progress::new());
            let mut body =
                Sending { body: payload.into(), slice: Bytes::new(), progress: progress.clone() };
            assert_eq!(body.size_hint().exact(), Some(SLICE as u64 * 2 + 8));
            tokio::time::advance(Duration::from_secs(7)).await;
            let mut lens = vec![];
            let mut bytes = vec![];
            while let Some(frame) = body.frame().await {
                let data = frame.unwrap().into_data().unwrap();
                lens.push(data.len());
                bytes.extend_from_slice(&data);
                let left = body.size_hint().exact().unwrap();
                assert_eq!(left as usize, SLICE * 2 + 8 - bytes.len());
            }
            assert_eq!(lens, [SLICE, SLICE, 5, 3]);
            let mut expected = vec![1; SLICE * 2 + 5];
            expected.extend([2; 3]);
            assert_eq!(bytes, expected);
            assert!(body.is_end_stream());
            assert_eq!(progress.last.load(Ordering::Relaxed), 7_000_000_000);
        });
    }

    /// A request stalls once no bytes moved for the stall time since they last did.
    #[test]
    fn requests_stall_once_idle_for_the_stall_time() {
        runtime().block_on(async {
            let progress = Progress::new();
            tokio::time::advance(Duration::from_secs(20)).await;
            progress.made();
            let start = Instant::now();
            progress.stalls(Duration::from_secs(30)).await;
            assert_eq!(start.elapsed(), Duration::from_secs(30));
        });
    }

    /// A body that never comes.
    struct Never;

    impl Body for Never {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
            Poll::Pending
        }
    }

    /// A response none of which comes fails as timed out, once its reader waited the stall time.
    #[test]
    fn responses_stall_once_nothing_came_for_the_stall_time() {
        runtime().block_on(async {
            let stall = Duration::from_secs(30);
            let mut body = Receiving { body: reqwest::Body::wrap(Never), stall, waiting: None };
            let start = Instant::now();
            let error = body.frame().await.unwrap().unwrap_err();
            assert_eq!(start.elapsed(), stall);
            assert_eq!(error.kind(), HttpErrorKind::Timeout);
            assert_eq!(
                std::error::Error::source(&error).unwrap().to_string(),
                "no bytes moved for 30s"
            );
        });
    }

    /// The options a client is built from are fxdax's: its connect timeout and no other.
    #[test]
    fn clients_time_out_by_progress_alone() {
        let options = Client::DEFAULT.options();
        let value = |key| options.get_config_value(&key);
        assert_eq!(value(ClientConfigKey::ConnectTimeout).as_deref(), Some("5s"));
        assert_eq!(value(ClientConfigKey::Timeout), None);
        assert_eq!(value(ClientConfigKey::ReadTimeout), None);
        reqwest_client(&options).unwrap();
        let invalid = options.with_config(ClientConfigKey::AllowHttp, "maybe");
        let refused = reqwest_client(&invalid).unwrap_err().to_string();
        assert_eq!(refused, "Generic fxdax error: allow_http: maybe is no boolean");
    }
}
