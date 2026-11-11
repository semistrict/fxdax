// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: an S3 bucket degraded as a test says (TASK-330.4): an HTTP proxy in front of an S3
//! server, which waits so long before each request it passes on, and fails requests as a degraded
//! S3 server does: its writes as a full one, `507 Insufficient Storage`, or every request as one
//! out of service, `503 Service Unavailable`; each after reading the request whole, as a server
//! does. A daemon whose bucket is through it (its `AWS_ENDPOINT`) meets what its own S3 client
//! makes of those, retries and all.
//!
//! Between the daemon and its bucket, not in the daemon (`extent_remote::faults`,
//! `FXDAX_REMOTE_FAULTS`, which fxdax's in-process soak still uses): so that a pin's daemon from
//! before that, LangSmith's, meets the same faults as the candidate's, and a qualification's A/B
//! compares what each did with them. The requests keep their `Host`, which their signatures sign:
//! the server checks them against what they were sent to, this proxy.

use anyhow::Context;
use bytes::Bytes;
use extent_remote::faults::{Failing, Fault};
use http_body_util::{BodyExt as _, Either, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

type Body = Either<Incoming, Full<Bytes>>;

/// A proxy serving on a port of 127.0.0.1 of its own, in front of an S3 server, as long as it
/// lives: on a thread of its own, its runtime's.
pub struct FaultyS3 {
    fault: Arc<Mutex<Fault>>,
    address: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FaultyS3 {
    /// A proxy in front of the S3 server at `upstream`, an `http://HOST:PORT` endpoint, passing
    /// every request on until [`FaultyS3::set`] says otherwise.
    pub fn start(upstream: &str) -> anyhow::Result<Self> {
        let upstream: SocketAddr = (upstream.strip_prefix("http://"))
            .with_context(|| format!("{upstream}: no http:// endpoint"))?
            .trim_end_matches('/')
            .parse()
            .with_context(|| format!("{upstream}: no HOST:PORT"))?;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))?;
        let address = listener.local_addr()?;
        let fault = Arc::new(Mutex::new(Fault::default()));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let served = fault.clone();
        let thread = std::thread::Builder::new().name("faulty-s3".into()).spawn(move || {
            runtime.block_on(async move {
                tokio::select! {
                    _ = stopped => {}
                    () = serve(listener, upstream, served) => {}
                }
            })
        })?;
        Ok(Self { fault, address, stop: Some(stop), thread: Some(thread) })
    }

    /// Its endpoint, an S3 client's `AWS_ENDPOINT`.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Fails requests as `fault` says from now on.
    pub fn set(&self, fault: Fault) {
        *self.fault.lock().unwrap() = fault;
    }
}

impl Drop for FaultyS3 {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Serves each connection `listener` accepts, on a task of its own.
async fn serve(listener: tokio::net::TcpListener, upstream: SocketAddr, fault: Arc<Mutex<Fault>>) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(err) => {
                eprintln!("faulty-s3: accepting: {err}");
                continue;
            }
        };
        let fault = fault.clone();
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request| {
                let fault = *fault.lock().unwrap();
                answer(request, upstream, fault)
            });
            let io = TokioIo::new(stream);
            if let Err(err) =
                hyper::server::conn::http1::Builder::new().serve_connection(io, service).await
            {
                // A client that drops its connection mid-request, as one does that gave up.
                eprintln!("faulty-s3: a connection: {err}");
            }
        });
    }
}

/// What `request` gets under `fault`: the server's answer, after the fault's latency, or the
/// fault's failure.
async fn answer(
    request: Request<Incoming>,
    upstream: SocketAddr,
    fault: Fault,
) -> Result<Response<Body>, hyper::Error> {
    tokio::time::sleep(fault.latency).await;
    let write = matches!(*request.method(), Method::PUT | Method::POST);
    let failure = match fault.failing {
        Failing::NoSpace if write => Some((
            StatusCode::INSUFFICIENT_STORAGE,
            "XRustFSStorageFull",
            "Storage backend has reached its minimum free drive threshold",
        )),
        Failing::Unavailable => {
            Some((StatusCode::SERVICE_UNAVAILABLE, "ServiceUnavailable", "Service unavailable"))
        }
        Failing::Not | Failing::NoSpace => None,
    };
    if let Some((status, code, message)) = failure {
        // Read whole, as a server reads a request before it answers.
        request.into_body().collect().await?;
        return Ok(failed(status, code, message));
    }
    match forward(request, upstream).await {
        Ok(response) => Ok(response.map(Either::Left)),
        Err(err) => {
            eprintln!("faulty-s3: passing a request on: {err:#}");
            Ok(failed(StatusCode::BAD_GATEWAY, "BadGateway", "The S3 server didn't answer"))
        }
    }
}

/// `request`, sent on to the server at `upstream` on a connection of its own, and its answer.
async fn forward(
    request: Request<Incoming>,
    upstream: SocketAddr,
) -> anyhow::Result<Response<Incoming>> {
    let stream = tokio::net::TcpStream::connect(upstream).await?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            eprintln!("faulty-s3: a connection to the server: {err}");
        }
    });
    Ok(sender.send_request(request).await?)
}

/// An S3 error's answer.
fn failed(status: StatusCode, code: &str, message: &str) -> Response<Body> {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Error><Code>{code}</Code><Message>{message}</Message></Error>"
    );
    Response::builder()
        .status(status)
        .header("content-type", "application/xml")
        .body(Either::Right(Full::new(Bytes::from(body))))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::time::{Duration, Instant};

    /// An S3 server's stand-in: answers each request 200, saying the `Host` it was sent with and
    /// the body's length; its endpoint.
    fn upstream() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let (head, body) = request(&mut stream);
                let host = (head.lines())
                    .find_map(|line| line.strip_prefix("host: ").or(line.strip_prefix("Host: ")))
                    .unwrap_or("-")
                    .to_owned();
                let said = format!("host={host} body={}", body.len());
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{said}",
                    said.len()
                );
                stream.write_all(answer.as_bytes()).unwrap();
            }
        });
        format!("http://{address}")
    }

    /// A request's head and body, read from `stream`, its body by its content-length.
    fn request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
        let mut read = Vec::new();
        let mut byte = [0];
        while !read.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            read.push(byte[0]);
        }
        let head = String::from_utf8(read).unwrap();
        let length = (head.lines())
            .find_map(|line| {
                line.to_ascii_lowercase().strip_prefix("content-length: ")?.parse().ok()
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        (head, body)
    }

    /// `method` of an object through the proxy at `endpoint`, with a body of `body` bytes, as a
    /// client signing for `signed.example:9000` sends it: the answer's status and body.
    fn ask(endpoint: &str, method: &str, body: usize) -> (u16, String) {
        let mut stream =
            std::net::TcpStream::connect(endpoint.trim_start_matches("http://")).unwrap();
        let head = format!(
            "{method} /fxdax/object HTTP/1.1\r\nHost: signed.example:9000\r\ncontent-length: \
             {body}\r\nconnection: close\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&vec![7; body]).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        let status = answer[9..12].parse().unwrap();
        (status, answer.split_once("\r\n\r\n").unwrap().1.to_owned())
    }

    /// TASK-330.4: requests pass on whole, their Host as signed; a full bucket fails writes alone,
    /// 507, after reading them; one out of service fails every request, 503; latency delays each.
    #[test]
    fn the_proxy_degrades_the_bucket_as_it_is_told() {
        let bucket = FaultyS3::start(&upstream()).unwrap();
        let endpoint = bucket.endpoint();
        assert_eq!(ask(&endpoint, "PUT", 4096), (200, "host=signed.example:9000 body=4096".into()));
        bucket.set(Fault::parse("failing=no-space").unwrap());
        let (status, said) = ask(&endpoint, "PUT", 1 << 20);
        assert_eq!(status, 507);
        assert!(said.contains("<Code>XRustFSStorageFull</Code>"), "{said}");
        assert_eq!(ask(&endpoint, "GET", 0), (200, "host=signed.example:9000 body=0".into()));
        bucket.set(Fault::parse("failing=unavailable").unwrap());
        assert_eq!(ask(&endpoint, "GET", 0).0, 503);
        bucket.set(Fault::parse("latency-ms=300").unwrap());
        let began = Instant::now();
        assert_eq!(ask(&endpoint, "GET", 0).0, 200);
        assert!(began.elapsed() >= Duration::from_millis(300), "{:?}", began.elapsed());
    }
}
