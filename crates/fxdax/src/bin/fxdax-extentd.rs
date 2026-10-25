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
//! MEMORY_BYTES is how much memory its kernel takes at most, for the extents it caches, and the
//! open extents VMs map writably: the kernel's default otherwise (1 GiB). Out of it, or past a
//! limit the control API sets, the daemon evicts the cached pages read longest ago, and fails what
//! it cannot make room for with ENOMEM, rather than stop (TASK-50.3). A VMM's extent device
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
//! (`fxdax::extentd`, Isolation).
//!
//! With `FXDAX_EXTENTD_GRPC` naming a path, the daemon serves its control API there too, gRPC over a
//! Unix socket (`fxdax::extentd::control`; `crates/fxdax/proto/fxdax/extentd/v1/extentd.proto`),
//! through which the host's VM manager makes, forks, syncs and deletes stores, and grants tenants
//! VM sockets of their own: whoever the socket's permissions let connect may do all of it. The
//! tenants' sockets go in the directory `FXDAX_EXTENTD_TENANTS` names, or `tenants` beside the
//! control socket. The control socket serves the standard gRPC health service too
//! (`grpc.health.v1.Health`, TASK-48): SERVING while the daemon accepts VM processes, NOT_SERVING
//! as it starts and as it stops. SIGTERM stops it: once the health service reports so, it exits.
//!
//! With `FXDAX_EXTENTD_METRICS` naming an address (`HOST:PORT`), the daemon serves its metrics
//! there, over HTTP, at `/metrics`, in Prometheus' text format (`fxdax::extentd::metrics`,
//! TASK-47).
//!
//! `FXDAX_EXTENTD_REMOTE` names the bucket (`s3://bucket/prefix`, `gs://bucket/prefix`,
//! `az://container/prefix`) each store it opens keeps its branch in (backlog doc-7), with
//! credentials from the environment's `AWS_*`, `GOOGLE_*` or `AZURE_*` variables: durable commits are uploaded behind them, within a second. With
//! `FXDAX_EXTENTD_CACHE` naming a directory too, what stores read from the bucket is kept there,
//! sparse, leaving 4 GiB of its filesystem free; it starts empty.
//! With `FXDAX_EXTENTD_STATS` naming a file too, the daemon keeps there what it did with the
//! bucket, its requests by kind and its bytes up and down (`extent_remote::store::Stats::line`),
//! what the stores' followers are, what serving the stores cost it (`fxdax::stats::Stats`), and
//! the VM processes' requests it served, by kind, rewritten as they change.

/// Logs to standard error: what goes wrong serving a VM process.
#[cfg(target_os = "linux")]
struct Stderr;

#[cfg(target_os = "linux")]
impl log::Log for Stderr {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{} {}: {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    log::set_logger(&Stderr).map(|()| log::set_max_level(log::LevelFilter::Info))?;
    // fxdax: the failpoints FAILPOINTS sets, in a daemon built with them (TASK-44.10).
    let _failpoints = fail::FailScenario::setup();
    // fxdax: before any thread is started, which SIGQUIT would end otherwise (TASK-44.13).
    fxdax::stacks::on_sigquit().context("handling SIGQUIT")?;
    stop_on_sigterm().context("handling SIGTERM")?;
    fxdax::extentd::raise_open_files_limit().context("raising the limit of open files")?;
    // A host of the multi-node system: the bucket and this host's placement token named.
    fxdax::remote::Host::from_env().map_err(anyhow::Error::msg)?;
    let mut args = std::env::args().skip(1);
    let socket = args.next().context("usage: fxdax-extentd SOCKET [MEMORY_BYTES]")?;
    if let Some(memory) = args.next() {
        let memory: u64 = memory.parse().context("MEMORY_BYTES must be a number")?;
        fxdax::kernel::configure(fxdax::kernel::Config::with_memory(memory))?;
    }
    let listener = std::os::unix::net::UnixListener::bind(&socket)
        .with_context(|| format!("listening at {socket}"))?;
    if let Some(control) = std::env::var_os("FXDAX_EXTENTD_GRPC").filter(|path| !path.is_empty()) {
        let tenants = match std::env::var_os("FXDAX_EXTENTD_TENANTS").filter(|dir| !dir.is_empty())
        {
            Some(dir) => std::path::PathBuf::from(dir),
            None => std::path::Path::new(&control).with_file_name("tenants"),
        };
        std::fs::create_dir_all(&tenants)
            .with_context(|| format!("making {}", tenants.display()))?;
        let control = std::os::unix::net::UnixListener::bind(&control)
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
    Ok(())
}

/// Stops the daemon on SIGTERM, as an orchestrator stops it: its health service reports it no
/// longer serving (TASK-48), then it exits. SIGTERM is blocked in every thread but one, made here
/// first, which waits for it.
#[cfg(target_os = "linux")]
fn stop_on_sigterm() -> std::io::Result<()> {
    // SAFETY: A signal set, filled by the calls below.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: Plain calls on a signal set this function owns, and on the calling thread's mask,
    // before any other thread is made, which inherit it.
    unsafe {
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        let status = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status));
        }
    }
    std::thread::Builder::new().name("extentd signals".into()).spawn(move || {
        let mut signal = 0;
        // SAFETY: Waits for a signal of the set, which every thread blocks.
        let status = unsafe { libc::sigwait(&set, &mut signal) };
        if status != 0 {
            log::error!(
                "extentd: waiting for SIGTERM: {}",
                std::io::Error::from_raw_os_error(status)
            );
            return;
        }
        log::info!("extentd: stopping, on SIGTERM");
        fxdax::extentd::stop_serving();
        std::process::exit(0);
    })?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fxdax-extentd runs on Linux only");
    std::process::exit(2);
}
