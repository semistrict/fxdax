// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's extent daemon (`fxdax::extentd`): serves the extent stores of the VMs on this host, from
//! one memory, to VM processes that connect to its socket.
//!
//! ```sh
//! fxdax-extentd SOCKET [MEMORY_BYTES]
//! ```
//!
//! MEMORY_BYTES is how much memory its kernel takes at most, for the sealed extents it caches,
//! whose pages VMs' DAX windows map read-only: the kernel's default otherwise (1 GiB). A store's
//! private pages, which copy-on-write DAX gives a VM's stores, are of the VM's memory, not of it
//! (backlog doc-13). Out of it, or past a limit the control API sets, the daemon evicts the cached
//! extents asked for longest ago, and fails what it cannot make room for with ENOMEM, rather than
//! stop (TASK-50.3). A VMM's extent device
//! connects to the socket its configuration names (fxdax's Firecracker's `extentd_socket`).
//!
//! It runs as a host of fxdax's multi-node system: `FXDAX_EXTENTD_REMOTE` names the bucket its
//! stores' branches are kept in, and `FXDAX_EXTENTD_TOKEN` its placement's fencing token, which
//! the system's control plane gives it, and which fences earlier placements' writes and deletes;
//! without them, it refuses to start (`fxdax::remote::Host`, TASK-45). Tests and development use
//! an in-memory bucket (`memory:///NAME`).
//!
//! One daemon serves every VM on a host. Connections to SOCKET are of the default tenant: with
//! `FXDAX_EXTENTD_ROOTS` naming directories, separated by colons, they name stores in them alone
//! (`fxdax::extentd`, Isolation). SOCKET is the daemon's user's alone, mode 0600: a VM process of
//! another user, as a jail's, connects to a tenant's socket, granted to its user, and never to this
//! one, mounted or not (TASK-155). The VM manager runs jails as another user than the daemon's, and
//! hands them SOCKET's path no more than it does the control socket's.
//!
//! With `FXDAX_EXTENTD_GRPC` naming a path, the daemon serves its control API there too, gRPC over a
//! Unix socket (`fxdax::extentd::control`; `crates/fxdax/proto/fxdax/extentd/v1/extentd.proto`),
//! through which the host's VM manager makes, forks, syncs and deletes stores, and grants tenants
//! VM sockets of their own: whoever connects may do all of it, so that it is the daemon's user's
//! alone, mode 0600, as SOCKET is, whatever the umask (TASK-159): the VM manager runs as that
//! user, or as root. The tenants' sockets go in the directory `FXDAX_EXTENTD_TENANTS` names, or
//! `tenants` beside the control socket, each mode 0660 from the first. The control socket serves
//! the standard gRPC health service too (`grpc.health.v1.Health`, TASK-48): SERVING while the
//! daemon accepts VM processes, NOT_SERVING as it starts and as it stops.
//!
//! SIGTERM drains it, and it exits (`fxdax::extentd::drain`, TASK-141.4): once the health service
//! reports NOT_SERVING, it takes no more VM processes nor control calls (`UNAVAILABLE`), closes
//! every VM process's connection, whose VM goes as at the daemon's exit, and closes their stores,
//! durable, and the rest it has open, and waits for their uploads: within `FXDAX_EXTENTD_DRAIN`
//! seconds (60 by default), exiting 0; past them, it logs what is left and exits 1. A host is
//! upgraded so, its VMs moved off or stopped first (`docs/upgrading-a-host.md`).
//!
//! With `FXDAX_EXTENTD_METRICS` naming an address (`HOST:PORT`), the daemon serves its metrics
//! there, over HTTP, at `/metrics`, in Prometheus' text format (`fxdax::extentd::metrics`,
//! TASK-47).
//!
//! `FXDAX_EXTENTD_REMOTE` names the bucket (`s3://bucket/prefix`, `gs://bucket/prefix`,
//! `az://container/prefix`) each store it opens keeps its branch in (backlog doc-7), with
//! credentials from the environment's `AWS_*`, `GOOGLE_*` or `AZURE_*` variables: durable commits are uploaded behind them, within a second. With
//! `FXDAX_EXTENTD_CACHE` naming a directory too, what stores read from the bucket is kept there,
//! sparse, leaving 4 GiB of its filesystem free, and kept across the daemon's restarts: one daemon
//! at a time has the directory.
//! However far behind the bucket its stores fall, what waits for it is on the host's disk, each
//! store's write-ahead log, bounded by its lag (backlog doc-7, Uploads); what of it the daemon
//! holds in memory is what it uploads at once, `FXDAX_EXTENTD_UPLOAD_MEMORY` bytes at most (512 MiB
//! by default, 64 MiB at least: an extent, which goes alone), every store's together (TASK-317).
//! With `FXDAX_EXTENTD_STATS` naming a file too, the daemon keeps there what it did with the
//! bucket, its requests by kind and its bytes up and down (`extent_remote::store::Stats::line`),
//! what the stores' followers are, what serving the stores cost it (`fxdax::stats::Stats`), and
//! the VM processes' requests it served, by kind, rewritten as they change.
//!
//! With `FXDAX_EXTENTD_TRACES` set to `otlp`, the daemon exports spans of what it does, over OTLP,
//! to the collector `OTEL_EXPORTER_OTLP_ENDPOINT` names, OpenTelemetry's variables configuring
//! them (`fxdax::extentd::otel`, TASK-258; `docs/tracing.md`): off otherwise.
//!
//! It logs to standard error, a line a record (`fxdax::extentd::logging`): text by default, or,
//! with `FXDAX_LOG_FORMAT=json`, a JSON object a line, of the record's `timestamp`, `level`,
//! `target`, `message` and key-values (TASK-259). `FXDAX_LOG_LEVEL` says how much, `info` by
//! default, by target too: `info,extent_remote::store=debug` logs each request of object storage.
//! A record about a VM's request names it, `request_id`, the guest's number of it, with its
//! `store` (TASK-260).

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    fxdax::extentd::logging::to_stderr()?;
    // fxdax: its large buffers given back as they are freed, before any thread is made (TASK-317).
    fxdax::extentd::memory::return_large_allocations();
    // fxdax: a panic holding a lock shared beyond one store, which no fence contains, stops the
    // daemon, rather than poison the lock for every user after (TASK-149.24).
    extent_store::shared::abort_on_panics();
    // fxdax: the failpoints FAILPOINTS sets, in a daemon built with them (TASK-44.10).
    let _failpoints = fail::FailScenario::setup();
    fxdax::faults::from_seed().map_err(anyhow::Error::msg)?;
    // fxdax: before any thread is started, which SIGQUIT would end otherwise (TASK-44.13).
    fxdax::stacks::on_sigquit().context("handling SIGQUIT")?;
    let drain = fxdax::extentd::drain::bound().map_err(anyhow::Error::msg)?;
    drain_on_sigterm(drain).context("handling SIGTERM")?;
    // fxdax: its spans, over OTLP, if FXDAX_EXTENTD_TRACES says so (TASK-258): its exporter's
    // threads made after SIGTERM is blocked, as every thread but the drain's.
    if fxdax::extentd::otel::from_env().map_err(anyhow::Error::msg)? {
        log::info!("extentd: exporting spans over OTLP");
    }
    fxdax::extentd::raise_open_files_limit().context("raising the limit of open files")?;
    // A host of the multi-node system: the bucket and this host's placement token named.
    fxdax::remote::Host::from_env().map_err(anyhow::Error::msg)?;
    // fxdax: what its uploads may hold at once (TASK-317).
    if let Some(bytes) = std::env::var_os("FXDAX_EXTENTD_UPLOAD_MEMORY").filter(|b| !b.is_empty()) {
        let bytes = bytes.to_str().and_then(|bytes| bytes.parse().ok());
        let bytes = bytes.context("FXDAX_EXTENTD_UPLOAD_MEMORY must be a number of bytes")?;
        fxdax::remote::set_upload_memory(bytes).map_err(anyhow::Error::msg)?;
    }
    let mut args = std::env::args().skip(1);
    let socket = args.next().context("usage: fxdax-extentd SOCKET [MEMORY_BYTES]")?;
    if let Some(memory) = args.next() {
        let memory: u64 = memory.parse().context("MEMORY_BYTES must be a number")?;
        fxdax::pages::configure(fxdax::pages::Config::with_memory(memory))?;
    }
    let listener = fxdax::extentd::bind_owner_only(&socket)
        .with_context(|| format!("listening at {socket}"))?;
    if let Some(control) = std::env::var_os("FXDAX_EXTENTD_GRPC").filter(|path| !path.is_empty()) {
        let tenants = match std::env::var_os("FXDAX_EXTENTD_TENANTS").filter(|dir| !dir.is_empty())
        {
            Some(dir) => std::path::PathBuf::from(dir),
            None => std::path::Path::new(&control).with_file_name("tenants"),
        };
        std::fs::create_dir_all(&tenants)
            .with_context(|| format!("making {}", tenants.display()))?;
        // It can do everything: its user's alone, as the VM socket (TASK-159).
        let control = fxdax::extentd::bind_owner_only(&control)
            .with_context(|| format!("listening at {}", control.display()))?;
        std::thread::Builder::new().name("extentd control".into()).spawn(move || {
            if let Err(err) = fxdax::extentd::serve_control(control, tenants) {
                log::error!("extentd: serving the control API: {err}");
                std::process::exit(1);
            }
        })?;
    }
    if let Some(address) =
        std::env::var_os("FXDAX_EXTENTD_METRICS").filter(|address| !address.is_empty())
    {
        let address = address.to_str().context("FXDAX_EXTENTD_METRICS must be an address")?;
        let metrics = std::net::TcpListener::bind(address)
            .with_context(|| format!("serving metrics at {address}"))?;
        std::thread::Builder::new().name("extentd metrics".into()).spawn(move || {
            if let Err(err) = fxdax::extentd::serve_metrics(metrics) {
                log::error!("extentd: serving metrics: {err}");
            }
        })?;
    }
    fxdax::extentd::serve(listener);
    // Serving stops only as the daemon drains, whose thread exits the process once it has
    // (`drain_on_sigterm`): not here, its stores still closing.
    loop {
        std::thread::park();
    }
}

/// Drains the daemon on SIGTERM, as an orchestrator stops it, within `bound`, and exits
/// (`fxdax::extentd::drain`, TASK-141.4): 0 once it drained, 1 past its bound. SIGTERM is blocked
/// in every thread but one, made here first, which waits for it (`fxdax::signals`).
#[cfg(target_os = "linux")]
fn drain_on_sigterm(bound: std::time::Duration) -> std::io::Result<()> {
    let sigterm = fxdax::signals::Signals::block(&[libc::SIGTERM])?;
    fxdax::signals::spawn_taking_no_signals("extentd signals", move || {
        if let Err(error) = sigterm.wait() {
            log::error!("extentd: waiting for SIGTERM: {error}");
            return;
        }
        log::info!("extentd: draining, on SIGTERM, within {}s", bound.as_secs());
        let began = std::time::Instant::now();
        let drained = fxdax::extentd::drain::drain(bound);
        fxdax::extentd::otel::shutdown();
        match drained {
            Ok(()) => {
                log::info!("extentd: drained in {:.1?}: every store closed", began.elapsed());
                std::process::exit(0);
            }
            Err(left) => {
                log::error!("extentd: not drained within {}s: {left}", bound.as_secs());
                std::process::exit(1);
            }
        }
    })
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fxdax-extentd runs on Linux only");
    std::process::exit(2);
}
