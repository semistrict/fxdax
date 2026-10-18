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
//! open extents VMs map writably: the kernel's default otherwise. A VMM's extent device connects
//! to the socket its configuration names (fxdax's Firecracker's `extentd_socket`).
//!
//! It runs as a host of fxdax's multi-node system: `FXDAX_EXTENTD_REMOTE` names the bucket its
//! stores' branches are kept in, and `FXDAX_EXTENTD_TOKEN` its placement's fencing token, which
//! the system's control plane gives it, and which fences earlier placements' writes and deletes;
//! without them, it refuses to start, unless asked to run single-node, for tests and development
//! (`FXDAX_EXTENTD_LOCAL_ONLY=1`, no bucket; `fxdax::remote::Mode`, TASK-45).
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
//! control socket.
//!
//! With `FXDAX_EXTENTD_REMOTE` naming a bucket (`s3://bucket/prefix`, `gs://bucket/prefix`), each
//! store it opens keeps its branch there (backlog doc-7), with credentials from the environment's
//! `AWS_*` or `GOOGLE_*` variables; durable commits are uploaded ones. With `FXDAX_EXTENTD_CACHE`
//! naming a directory too, what stores read from the bucket is kept there, sparse, leaving 4 GiB of
//! its filesystem free; it starts empty.
//! With `FXDAX_EXTENTD_STATS` naming a file too, the daemon keeps there what it did with the
//! bucket, its requests by kind and its bytes up and down, rewritten as they change
//! (`extent_remote::store::Stats::line`).

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
    // Multi-node, the bucket and this host's placement token named; or single-node, asked for.
    fxdax::remote::Mode::from_env().map_err(anyhow::Error::msg)?;
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
    fxdax::extentd::serve(listener).context("accepting connections")?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fxdax-extentd runs on Linux only");
    std::process::exit(2);
}
