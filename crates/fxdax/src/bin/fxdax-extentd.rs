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
//! to the socket libkrun's `krun_add_extent_store` names.
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
    let mut args = std::env::args().skip(1);
    let socket = args.next().context("usage: fxdax-extentd SOCKET [MEMORY_BYTES]")?;
    if let Some(memory) = args.next() {
        let memory: u64 = memory.parse().context("MEMORY_BYTES must be a number")?;
        fxdax::kernel::configure(fxdax::kernel::Config::with_memory(memory))?;
    }
    let listener = std::os::unix::net::UnixListener::bind(&socket)
        .with_context(|| format!("listening at {socket}"))?;
    fxdax::extentd::serve(listener).context("accepting connections")?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fxdax-extentd runs on Linux only");
    std::process::exit(2);
}
