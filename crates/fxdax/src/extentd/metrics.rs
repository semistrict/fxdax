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
//! - `fxdax_extentd_extent_cache_pinned_entries` and `fxdax_extentd_extent_cache_idle_entries`:
//!   the sealed extents cached that something but the cache holds, and those nothing else does
//!   (`crate::cache::entries`); `fxdax_extentd_window_mappings`,
//!   `fxdax_extentd_window_mapped_bytes` and `fxdax_extentd_window_pages`: the ranges the VMs'
//!   windows map, their bytes, and the pages of them mapped in the VM processes (`crate::dax::live`,
//!   TASK-131.6).
//! - `fxdax_extentd_<counter>_total`: the counters the control API's `GetStats` reports
//!   (`crate::stats::Stats::named`, its dashes underscores): maps, faults, page-ins, reads and
//!   their nanoseconds and bytes; the VM processes' changes to their windows (`stage_ops`) and
//!   their nanoseconds in them (`stage_ns`), whose ratio is what a window change costs a VM process.
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
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The kinds of request metrics are kept of, and their names.
const KINDS: [(u32, &str); 20] = [
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
    (FREE_MANY, "free_many"),
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

    let (entries, live) = (crate::cache::entries(), crate::dax::live());
    let gauges = [
        ("connections", "VM processes connected.", super::server::connections() as u64),
        (
            "stores_open",
            "Stores VM processes' connections write.",
            super::stores::open_count() as u64,
        ),
        ("follower_views", "Followers' views of stores.", super::stores::follower_views() as u64),
        (
            "extent_cache_pinned_entries",
            "Sealed extents cached that a window's mapping, a read or a page-in holds.",
            entries.pinned,
        ),
        (
            "extent_cache_idle_entries",
            "Sealed extents cached that nothing but the cache holds.",
            entries.idle,
        ),
        ("window_mappings", "Ranges the VMs' DAX windows map, all windows'.", live.mappings),
        ("window_mapped_bytes", "Bytes those ranges span.", live.mapped_bytes),
        (
            "window_pages",
            "Pages the VMs' DAX windows have mapped in their VM processes, all windows'.",
            live.pages,
        ),
    ];
    for (name, help, value) in gauges {
        let _ = writeln!(out, "# HELP fxdax_extentd_{name} {help}");
        let _ = writeln!(out, "# TYPE fxdax_extentd_{name} gauge");
        let _ = writeln!(out, "fxdax_extentd_{name} {value}");
    }

    let memory = [
        ("memory_bytes", "Memory extents' pages hold, every tenant's.", Some(physmem::arenas_bytes())),
        ("memory_limit_bytes", "The limit of every tenant's memory.", physmem::total_cap()),
        (
            "memory_capacity_bytes",
            "The most memory the daemon has.",
            Some(physmem::count_total_bytes()),
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
        let _ = writeln!(out, "# HELP {name} As the control API's GetStats reports it.");
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

/// How long a client has to send its request and take the response: one deadline for the whole
/// exchange, as a client sending a byte at a time, or taking nothing, would hold its turn for good
/// against a timeout of each read or write (TASK-79.27).
const EXCHANGE: Duration = Duration::from_secs(5);

/// The most bytes of a request's line and headers read: a scraper's are a line and a few headers.
/// More are refused, unread, as a line read whole would grow without end.
const HEAD_MAX: u64 = 8 << 10;

/// How many clients are served at once: those after wait to be accepted, holding nothing, so that
/// clients slow to send or take what they asked for hold up no scrape but past these.
const AT_ONCE: usize = 4;

/// Serves the metrics to whoever connects to `listener` and asks for `/metrics` over HTTP, until
/// it listens no more (`EINVAL`): each client on a thread of its own, [`AT_ONCE`] at most, within
/// [`EXCHANGE`]. An accept that fails otherwise, as with as many files open as the daemon may, is
/// tried again after a while (`server::ACCEPT_AGAIN`), as the daemon's VM sockets' are: given up
/// on, the daemon's metrics would be served no more, the daemon serving on (TASK-79.35).
pub fn serve_metrics(listener: TcpListener) -> std::io::Result<()> {
    let serving = std::sync::Arc::new((std::sync::Mutex::new(0usize), std::sync::Condvar::new()));
    let mut failing = false;
    loop {
        {
            let (count, done) = &*serving;
            let mut count = count.lock().unwrap();
            while *count == AT_ONCE {
                count = done.wait(count).unwrap();
            }
            *count += 1;
        }
        let accepted = listener.accept();
        if accepted.is_err() {
            *serving.0.lock().unwrap() -= 1;
        }
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(err) if err.raw_os_error() == Some(libc::EINVAL) => return Err(err),
            // That client's, gone before it was accepted.
            Err(err) if err.kind() == std::io::ErrorKind::ConnectionAborted => continue,
            Err(err) => {
                if !failing {
                    log::error!("extentd: accepting a metrics client: {err}: trying again");
                    failing = true;
                }
                std::thread::sleep(super::server::ACCEPT_AGAIN);
                continue;
            }
        };
        failing = false;
        let served = serving.clone();
        let spawned = std::thread::Builder::new().name("extentd metrics".into()).spawn(move || {
            if let Err(err) = answer(stream) {
                log::warn!("extentd: serving metrics: {err}");
            }
            let (count, done) = &*served;
            *count.lock().unwrap() -= 1;
            done.notify_one();
        });
        if let Err(err) = spawned {
            log::warn!("extentd: serving metrics: {err}");
            *serving.0.lock().unwrap() -= 1;
        }
    }
}

/// A client's connection within its exchange's deadline: each read and write waits at most for
/// what is left of it, and fails with `TimedOut` once nothing is.
struct Deadlined<'a> {
    stream: &'a TcpStream,
    deadline: Instant,
}

impl Deadlined<'_> {
    fn left(&self) -> std::io::Result<Duration> {
        match self.deadline.saturating_duration_since(Instant::now()) {
            Duration::ZERO => Err(std::io::ErrorKind::TimedOut.into()),
            left => Ok(left),
        }
    }
}

impl Read for Deadlined<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.left()?))?;
        (&*self.stream).read(bytes)
    }
}

impl Write for Deadlined<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.left()?))?;
        (&*self.stream).write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Answers the HTTP request `stream` carries, within [`EXCHANGE`]: the metrics, for `GET
/// /metrics`; `431`, for a request whose line and headers are more than [`HEAD_MAX`] bytes.
fn answer(stream: TcpStream) -> std::io::Result<()> {
    let deadline = Instant::now() + EXCHANGE;
    let mut reader = BufReader::new(Deadlined { stream: &stream, deadline }.take(HEAD_MAX));
    let mut request = String::new();
    let mut whole = read_line(&mut reader, &mut request)?;
    // The headers, which say nothing it needs.
    while whole {
        let mut line = String::new();
        whole = read_line(&mut reader, &mut line)?;
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let too_large = !whole && reader.get_ref().limit() == 0;
    let mut parts = request.split_whitespace();
    let (status, body) = match (parts.next(), parts.next()) {
        _ if too_large => ("431 Request Header Fields Too Large", "request too large\n".to_owned()),
        (Some("GET"), Some("/metrics")) => ("200 OK", render()),
        _ => ("404 Not Found", "not found\n".to_owned()),
    };
    let mut connection = Deadlined { stream: &stream, deadline };
    write!(
        connection,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )?;
    if too_large {
        // What it sent past its head is read and dropped, until it is done or its time is: the
        // connection closed with bytes unread would be reset, and the response lost with it.
        stream.shutdown(std::net::Shutdown::Write)?;
        std::io::copy(&mut Deadlined { stream: &stream, deadline }, &mut std::io::sink())?;
    }
    Ok(())
}

/// Reads a line into `line` as `read_line` does: whether it is whole, ended by a newline; not, at
/// the end of what `reader` reads.
fn read_line(reader: &mut impl BufRead, line: &mut String) -> std::io::Result<bool> {
    reader.read_line(line)?;
    Ok(line.ends_with('\n'))
}
