// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's metrics, in Prometheus' text exposition format, served over HTTP to whoever scrapes
//! them (TASK-47): `GET /metrics` on the address `FXDAX_EXTENTD_METRICS` names.
//!
//! - `fxdax_extentd_requests_total{kind}`, `fxdax_extentd_request_errors_total{kind}`, and
//!   `fxdax_extentd_request_duration_seconds{kind}`, a histogram: the VM processes' requests, by
//!   kind, as the daemon served them, from their dispatch to their reply's sending.
//! - `fxdax_extentd_connections`, `fxdax_extentd_stores_open` and `fxdax_extentd_follower_views`:
//!   the VM processes connected, the stores their connections write, and the followers' views.
//! - `fxdax_extentd_<counter>_total`: the daemon's counters, as its stats file reports them
//!   (`crate::stats::Stats::named`, its dashes underscores): maps, faults, page-ins, reads and
//!   their nanoseconds and bytes, the control API's `GetStats` too; and the VM processes' stage
//!   operations, as they told the daemon, which `GetStats` doesn't report (TASK-91.16).
//! - `fxdax_extentd_memory_bytes`, `fxdax_extentd_memory_limit_bytes` and
//!   `fxdax_extentd_memory_capacity_bytes`: the memory the daemon's cached and open extents' pages
//!   hold, every tenant's together, its limit, if any, and the most the daemon's kernel has
//!   (TASK-50.3); `fxdax_extentd_tenant_memory_bytes{tenant}` and
//!   `fxdax_extentd_tenant_memory_limit_bytes{tenant}`: each tenant's alive, by a hash of its name
//!   (`super::memory::label`), not the name.
//! - `fxdax_extentd_upload_pending_bytes{branch}`, `fxdax_extentd_upload_oldest_pending_seconds
//!   {branch}` and `fxdax_extentd_upload_stalls_total{branch}`: for each open store whose commits
//!   upload, its commits' bytes not yet uploaded, how long the oldest of them has waited, and how
//!   many commits waited for the lag to shrink (TASK-91.3); its branch in hexadecimal.
//! - `fxdax_extentd_object_storage_requests_total{request}`,
//!   `fxdax_extentd_object_storage_uploaded_bytes_total{object}` and
//!   `fxdax_extentd_object_storage_downloaded_bytes_total`: what the daemon did with its bucket,
//!   once it opened a store.

use super::wire::*;
use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The kinds of request metrics are kept of, and their names.
const KINDS: [(u32, &str); 19] = [
    (OPEN, "open"),
    (READ, "read"),
    (WRITE, "write"),
    (FREE, "free"),
    (MAP, "map"),
    (UNMAP, "unmap"),
    (COMMIT, "commit"),
    (ROOT, "root"),
    (INFO, "info"),
    (WINDOW, "window"),
    (FAULT, "fault"),
    (FORK, "fork"),
    (RESIZED, "resized"),
    (MAPPINGS, "mappings"),
    (ADVANCE, "advance"),
    (LET_GO, "let_go"),
    (PIN, "pin"),
    (UNPIN, "unpin"),
    (TRANSFER, "transfer"),
];

/// The upper bounds of the requests' duration histograms' buckets, in seconds: from what the
/// fastest take (tens of microseconds) to what durable commits may (a second).
const BUCKETS: [f64; 14] = [
    10e-6, 25e-6, 50e-6, 100e-6, 250e-6, 500e-6, 1e-3, 2.5e-3, 5e-3, 10e-3, 25e-3, 100e-3, 250e-3,
    1.0,
];

/// The requests of a kind served: how many took at most each bucket's bound, how many in all,
/// their nanoseconds, and how many failed.
#[derive(Default)]
struct Requests {
    buckets: [AtomicU64; BUCKETS.len()],
    count: AtomicU64,
    nanos: AtomicU64,
    errors: AtomicU64,
}

/// Each kind's requests, in [`KINDS`]' order.
static REQUESTS: LazyLock<[Requests; KINDS.len()]> = LazyLock::new(Default::default);

/// Counts a request of `kind` served, which took `took`, and whether it `failed`.
pub(super) fn served(kind: u32, took: Duration, failed: bool) {
    let Some(at) = KINDS.iter().position(|(known, _)| *known == kind) else { return };
    let requests = &REQUESTS[at];
    let seconds = took.as_secs_f64();
    if let Some(bucket) = BUCKETS.iter().position(|bound| seconds <= *bound) {
        requests.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }
    requests.count.fetch_add(1, Ordering::Relaxed);
    requests.nanos.fetch_add(took.as_nanos() as u64, Ordering::Relaxed);
    if failed {
        requests.errors.fetch_add(1, Ordering::Relaxed);
    }
}

/// The requests served, as a line of `NAME=COUNT` after `requests:`: of each kind served, how
/// many (`KIND`) and their nanoseconds (`KIND-ns`), which the daemon's stats file keeps
/// (`super::stores`).
pub(super) fn requests_line() -> String {
    let mut line = String::from("requests:");
    for ((_, kind), requests) in KINDS.iter().zip(REQUESTS.iter()) {
        let count = requests.count.load(Ordering::Relaxed);
        if count > 0 {
            let nanos = requests.nanos.load(Ordering::Relaxed);
            let _ = write!(line, " {kind}={count} {kind}-ns={nanos}");
        }
    }
    line
}

/// The metrics as of now, in Prometheus' text format.
pub fn render() -> String {
    let mut out = String::new();
    let load = |count: &AtomicU64| count.load(Ordering::Relaxed);

    out += "# HELP fxdax_extentd_requests_total VM processes' requests served, by kind.\n";
    out += "# TYPE fxdax_extentd_requests_total counter\n";
    for ((_, kind), requests) in KINDS.iter().zip(REQUESTS.iter()) {
        let _ = writeln!(
            out,
            "fxdax_extentd_requests_total{{kind=\"{kind}\"}} {}",
            load(&requests.count)
        );
    }
    out +=
        "# HELP fxdax_extentd_request_errors_total VM processes' requests that failed, by kind.\n";
    out += "# TYPE fxdax_extentd_request_errors_total counter\n";
    for ((_, kind), requests) in KINDS.iter().zip(REQUESTS.iter()) {
        let _ = writeln!(
            out,
            "fxdax_extentd_request_errors_total{{kind=\"{kind}\"}} {}",
            load(&requests.errors)
        );
    }
    out += "# HELP fxdax_extentd_request_duration_seconds How long VM processes' requests took to \
            serve, from their dispatch to their reply, by kind.\n";
    out += "# TYPE fxdax_extentd_request_duration_seconds histogram\n";
    for ((_, kind), requests) in KINDS.iter().zip(REQUESTS.iter()) {
        let name = "fxdax_extentd_request_duration_seconds";
        let mut cumulative = 0;
        for (bound, bucket) in BUCKETS.iter().zip(&requests.buckets) {
            cumulative += load(bucket);
            let _ = writeln!(out, "{name}_bucket{{kind=\"{kind}\",le=\"{bound}\"}} {cumulative}");
        }
        let count = load(&requests.count);
        let _ = writeln!(out, "{name}_bucket{{kind=\"{kind}\",le=\"+Inf\"}} {count}");
        let sum = load(&requests.nanos) as f64 / 1e9;
        let _ = writeln!(out, "{name}_sum{{kind=\"{kind}\"}} {sum}");
        let _ = writeln!(out, "{name}_count{{kind=\"{kind}\"}} {count}");
    }

    let gauges = [
        ("connections", "VM processes connected.", super::server::connections() as u64),
        (
            "stores_open",
            "Stores VM processes' connections write.",
            super::stores::open_count() as u64,
        ),
        ("follower_views", "Followers' views of stores.", super::stores::follower_views() as u64),
    ];
    for (name, help, value) in gauges {
        let _ = writeln!(out, "# HELP fxdax_extentd_{name} {help}");
        let _ = writeln!(out, "# TYPE fxdax_extentd_{name} gauge");
        let _ = writeln!(out, "fxdax_extentd_{name} {value}");
    }

    let node = zircon::vm::pmm::node();
    let memory = [
        ("memory_bytes", "Memory extents' pages hold, every tenant's.", Some(node.arenas_bytes())),
        ("memory_limit_bytes", "The limit of every tenant's memory.", node.total_cap()),
        (
            "memory_capacity_bytes",
            "The most memory the daemon has.",
            Some(node.count_total_bytes()),
        ),
    ];
    for (name, help, value) in memory {
        let Some(value) = value else { continue };
        let _ = writeln!(out, "# HELP fxdax_extentd_{name} {help}");
        let _ = writeln!(out, "# TYPE fxdax_extentd_{name} gauge");
        let _ = writeln!(out, "fxdax_extentd_{name} {value}");
    }
    let tenants = super::memory::tenants();
    let name = "fxdax_extentd_tenant_memory_bytes";
    let _ =
        writeln!(out, "# HELP {name} Memory a tenant's extents' pages hold, by its name's hash.");
    let _ = writeln!(out, "# TYPE {name} gauge");
    for tenant in &tenants {
        let _ = writeln!(out, "{name}{{tenant=\"{}\"}} {}", tenant.label, tenant.bytes);
    }
    let name = "fxdax_extentd_tenant_memory_limit_bytes";
    let _ = writeln!(out, "# HELP {name} The limit of a tenant's memory, by its name's hash.");
    let _ = writeln!(out, "# TYPE {name} gauge");
    for tenant in &tenants {
        if let Some(limit) = tenant.limit {
            let _ = writeln!(out, "{name}{{tenant=\"{}\"}} {limit}", tenant.label);
        }
    }

    for (name, value) in crate::stats::stats().named() {
        let name = format!("fxdax_extentd_{}_total", name.replace('-', "_"));
        let _ = writeln!(out, "# HELP {name} As the daemon's stats file reports it.");
        let _ = writeln!(out, "# TYPE {name} counter");
        let _ = writeln!(out, "{name} {value}");
    }

    let uploads = super::stores::upload_statuses();
    let series: [(&str, &str, &str, fn(&crate::remote::UploadStatus) -> String); 3] = [
        (
            "upload_pending_bytes",
            "gauge",
            "Bytes of a store's commits not yet uploaded, by its branch.",
            |status| status.pending_bytes.to_string(),
        ),
        (
            "upload_oldest_pending_seconds",
            "gauge",
            "How long a store's oldest commit not yet uploaded has waited, by its branch.",
            |status| {
                let waited = status.oldest_pending.and_then(|since| since.elapsed().ok());
                waited.unwrap_or_default().as_secs_f64().to_string()
            },
        ),
        (
            "upload_stalls_total",
            "counter",
            "A store's commits that waited for its upload lag to shrink, by its branch.",
            |status| status.stalls.to_string(),
        ),
    ];
    for (name, kind, help, value) in series {
        let _ = writeln!(out, "# HELP fxdax_extentd_{name} {help}");
        let _ = writeln!(out, "# TYPE fxdax_extentd_{name} {kind}");
        for (branch, status) in &uploads {
            let _ =
                writeln!(out, "fxdax_extentd_{name}{{branch=\"{branch:x}\"}} {}", value(status));
        }
    }

    if let Some(stats) = super::stores::bucket_stats() {
        use extent_remote::store::{Request, Upload};
        let name = "fxdax_extentd_object_storage_requests_total";
        let _ = writeln!(out, "# HELP {name} Requests to the bucket, by kind.");
        let _ = writeln!(out, "# TYPE {name} counter");
        for request in Request::ALL {
            let _ = writeln!(
                out,
                "{name}{{request=\"{}\"}} {}",
                request.name(),
                stats.requests(request)
            );
        }
        let name = "fxdax_extentd_object_storage_uploaded_bytes_total";
        let _ = writeln!(out, "# HELP {name} Bytes uploaded to the bucket, by kind of object.");
        let _ = writeln!(out, "# TYPE {name} counter");
        for upload in Upload::ALL {
            let _ =
                writeln!(out, "{name}{{object=\"{}\"}} {}", upload.name(), stats.uploaded(upload));
        }
        let name = "fxdax_extentd_object_storage_downloaded_bytes_total";
        let _ = writeln!(out, "# HELP {name} Bytes downloaded from the bucket.");
        let _ = writeln!(out, "# TYPE {name} counter");
        let _ = writeln!(out, "{name} {}", stats.downloaded);
    }
    out
}

/// Serves the metrics to whoever connects to `listener` and asks for `/metrics` over HTTP, until
/// accepting fails: a request at a time, as scrapers come every few seconds.
pub fn serve_metrics(listener: TcpListener) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        if let Err(err) = answer(stream) {
            log::warn!("extentd: serving metrics: {err}");
        }
    }
    Ok(())
}

/// Answers the HTTP request `stream` carries: the metrics, for `GET /metrics`.
fn answer(stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(&stream);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // The headers, which say nothing it needs.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut parts = request.split_whitespace();
    let (status, body) = match (parts.next(), parts.next()) {
        (Some("GET"), Some("/metrics")) => ("200 OK", render()),
        _ => ("404 Not Found", "not found\n".to_owned()),
    };
    let mut stream = &stream;
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}
