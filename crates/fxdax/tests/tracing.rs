// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-258: the daemon's binary exports its spans over OTLP, to the collector
//! `OTEL_EXPORTER_OTLP_ENDPOINT` names, with `FXDAX_EXTENTD_TRACES=otlp`; and sends nothing
//! without. The collector is one of the test's own, which takes OTLP over HTTP as a real one does
//! (`docs/tracing.md` runs Jaeger's), and keeps what it is sent.

#![cfg(target_os = "linux")]

use fxdax::{ExtentId, ExtentStore, RemoteExtentStore, VolatileSlice};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A collector of OTLP over HTTP, on a thread: its endpoint, and the bodies of the exports it
/// takes, each acknowledged, as they come.
fn collector() -> (String, mpsc::Receiver<(String, Vec<u8>)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sent, exports) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = BufReader::new(stream.unwrap());
            loop {
                let mut line = String::new();
                if stream.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let target = line.split_whitespace().nth(1).unwrap_or_default().to_owned();
                let mut len = 0;
                loop {
                    let mut header = String::new();
                    stream.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        len = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                stream.read_exact(&mut body).unwrap();
                // Kept before it is acknowledged, which the daemon may exit on.
                if sent.send((target, body)).is_err() {
                    return;
                }
                let ok = "HTTP/1.1 200 OK\r\ncontent-type: application/x-protobuf\r\n\
                          content-length: 0\r\n\r\n";
                stream.get_mut().write_all(ok.as_bytes()).unwrap();
            }
        }
    });
    (endpoint, exports)
}

/// A daemon serving at `socket`, its stores' branches in a bucket in its memory, as `traces` says
/// of its spans, OpenTelemetry's variables naming `endpoint` the collector, every trace kept.
fn daemon(socket: &Path, traces: Option<&str>, endpoint: &str) -> KillOnDrop {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"));
    command
        .arg(socket)
        .env("FXDAX_EXTENTD_REMOTE", "memory:///branches")
        .env("FXDAX_EXTENTD_TOKEN", "1")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
        .env("OTEL_TRACES_SAMPLER", "always_on")
        .env_remove("OTEL_SERVICE_NAME");
    match traces {
        Some(traces) => command.env("FXDAX_EXTENTD_TRACES", traces),
        None => command.env_remove("FXDAX_EXTENTD_TRACES"),
    };
    let daemon = KillOnDrop(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::os::unix::net::UnixStream::connect(socket).is_err() {
        assert!(Instant::now() < deadline, "the daemon never served");
        std::thread::sleep(Duration::from_millis(20));
    }
    daemon
}

/// Has `daemon` write, commit and close a store in `dir`, then drains it with SIGTERM, which
/// exports the spans it has not yet: what it was sent, in order, once it exited.
fn serve_and_drain(dir: &Path, traces: Option<&str>) -> Vec<(String, Vec<u8>)> {
    let (endpoint, exports) = collector();
    let socket = dir.join("extentd.sock");
    let mut daemon = daemon(&socket, traces, &endpoint);
    let store = RemoteExtentStore::connect(&socket, dir.join("store"), 1 << 28).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(&[7; BLOCK][..])]).unwrap();
    store.commit(Some(b"traced")).unwrap();
    drop(store);
    // SAFETY: Signals the daemon this test started.
    assert_eq!(unsafe { libc::kill(daemon.0.id() as i32, libc::SIGTERM) }, 0);
    let status = daemon.0.wait().unwrap();
    assert!(status.success(), "{status}");
    exports.try_iter().collect()
}

fn holds(body: &[u8], text: &str) -> bool {
    body.windows(text.len()).any(|window| window == text.as_bytes())
}

#[test]
fn the_daemon_exports_its_spans_over_otlp() {
    let dir = tempfile::tempdir().unwrap();
    let exports = serve_and_drain(dir.path(), Some("otlp"));
    let targets: BTreeSet<&str> = exports.iter().map(|(target, _)| target.as_str()).collect();
    assert_eq!(targets, BTreeSet::from(["/v1/traces"]));
    let body: Vec<u8> = exports.into_iter().flat_map(|(_, body)| body).collect();
    let missing: Vec<&str> = [
        "fxdax-extentd",
        "extentd.open",
        "extentd.write",
        "extentd.commit",
        "store.create",
        "store.commit",
        "commit.seal",
        "commit.write",
        "store.flush",
        "commit.upload_wait",
        "upload.segment",
        "bucket.create",
        "fxdax.request.kind",
        "fxdax.branch",
        "fxdax.bytes",
    ]
    .into_iter()
    .filter(|name| !holds(&body, name))
    .collect();
    assert_eq!(missing, Vec::<&str>::new(), "not exported");
}

#[test]
fn the_daemon_exports_nothing_unless_told_to() {
    let dir = tempfile::tempdir().unwrap();
    let exports = serve_and_drain(dir.path(), None);
    assert_eq!(exports.len(), 0);
}
