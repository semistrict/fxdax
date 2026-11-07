// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Mounts a store's filesystem on the host with FUSE (`fxdax::fuse`, TASK-222): an image, an
//! imported base, or a VM's store, which nothing else has open, read and written as any
//! filesystem is.
//!
//! ```sh
//! fxdax-mount [--allow-other] STORE MOUNTPOINT
//! ```
//!
//! STORE is a store's directory, whose filesystem's `default` volume is mounted at MOUNTPOINT, a
//! directory. Mounted, it prints MOUNTPOINT on standard output, and serves the filesystem until
//! it is unmounted: by `umount` or `fusermount3 -u`, or lazily on SIGINT or SIGTERM, once nothing
//! uses it. Then it closes the filesystem, writing back what is dirty, and exits 0.
//!
//! Root mounts it with mount(2), any other user through libfuse's `fusermount3`. With
//! `--allow-other`, users other than the mounting one may use it, as its permissions say: for a
//! user other than root, `/etc/fuse.conf` must have `user_allow_other`.
//!
//! The store is the mount's alone while it is mounted: an extent daemon serving it, or a second
//! mount, fails to open it, as the mount fails to open one in use. Mounting replays the
//! filesystem's journal, which writes to the store: to look at a VM's store and leave it be,
//! mount a fork of it (`fxdaxctl fork`).

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::io::Write as _;
    fxdax::extentd::logging::to_stderr()?;
    // Before any thread is started, which would take the signals otherwise.
    fxdax::stacks::on_sigquit().context("handling SIGQUIT")?;
    let stop = fxdax::signals::Signals::block(&[libc::SIGINT, libc::SIGTERM])
        .context("handling SIGINT and SIGTERM")?;
    const USAGE: &str = "usage: fxdax-mount [--allow-other] STORE MOUNTPOINT";
    let mut options = fxdax::fuse::MountOptions::new();
    let mut paths = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--allow-other") => {
                options.allow_other(true);
            }
            Some(flag) if flag.starts_with("--") => anyhow::bail!("{flag}? {USAGE}"),
            _ => paths.push(std::path::PathBuf::from(arg)),
        }
    }
    let [store, mountpoint] = <[_; 2]>::try_from(paths).map_err(|_| anyhow::anyhow!(USAGE))?;

    let storage =
        fxdax::Storage::open(&store).with_context(|| format!("opening {}", store.display()))?;
    let fs = futures::executor::block_on(fxdax::Filesystem::mount(storage))
        .with_context(|| format!("mounting {}'s filesystem", store.display()))?;
    let mount = options
        .mount(fs, &mountpoint)
        .with_context(|| format!("mounting it at {}", mountpoint.display()))?;
    let unmounter = mount.unmounter();
    fxdax::signals::spawn_taking_no_signals("fxdax-mount signals", move || match stop.wait() {
        Ok(signal) => {
            log::info!("fxdax-mount: unmounting, on signal {signal}, once nothing uses it");
            if let Err(error) = unmounter.unmount(true) {
                log::error!("fxdax-mount: {error:#}");
            }
        }
        Err(error) => log::error!("fxdax-mount: waiting for SIGINT or SIGTERM: {error}"),
    })?;
    log::info!("fxdax-mount: {} mounted at {}", store.display(), mountpoint.display());
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", mountpoint.display())?;
    stdout.flush()?;
    drop(stdout);

    mount.join().context("closing the filesystem")?;
    log::info!("fxdax-mount: {} unmounted and closed", store.display());
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fxdax-mount runs on Linux only: macOS has no open-source FUSE");
    std::process::exit(2);
}
