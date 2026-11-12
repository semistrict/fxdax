// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Mounts stores' filesystems on the host with FUSE (`fxdax::fuse`, TASK-222): an image, an
//! imported base, or a VM's store, which nothing else has open, read and written as any
//! filesystem is. One store at a mount point; or any number in one mount, tenants' cells attached
//! and detached through its control socket (TASK-302, TASK-364).
//!
//! ```sh
//! fxdax-mount [--allow-other] [--new CAPACITY] STORE MOUNTPOINT
//! fxdax-mount [--allow-other] --control SOCKET --stores DIR [--bucket URL] MOUNTPOINT
//! fxdax-mount attach SOCKET TENANT CELL [TOKEN]
//! fxdax-mount create-volume SOCKET TENANT CELL NAME CAPACITY
//! fxdax-mount detach [--force] SOCKET TENANT CELL
//! fxdax-mount list SOCKET
//! ```
//!
//! STORE is a store's directory, whose filesystem's `default` volume is mounted at MOUNTPOINT, a
//! directory. With `--new`, STORE, which must not exist, is made a new store of CAPACITY bytes, its
//! filesystem empty, as a scratch filesystem's or a test's (CI's pjdfstest, TASK-149.38): its
//! branch kept in a file bucket in its directory, or in the host's bucket on a host of fxdax's
//! system (`FXDAX_EXTENTD_REMOTE`), as `fxdax::Storage::create` keeps it.
//!
//! With `--control`, MOUNTPOINT is a directory of tenants' cells' volumes, none at first, and
//! SOCKET, made the mounting user's alone (mode 0600), serves its control API
//! (`proto/fxdax/mount/v1/mount.proto`), which the commands below call. TENANT and CELL are
//! hexadecimal. `attach` takes CELL (0 for a new one, of no volumes) of the bucket at URL (default:
//! the host's, `FXDAX_EXTENTD_REMOTE`), at the placement whose fencing token is TOKEN if given, and
//! every Fxfs volume of it into a store at DIR/TENANT/CELL/VOLUME, its lease with it, or opens the
//! store there again; shows each at MOUNTPOINT/TENANT/CELL/VOLUME, TENANT and CELL in 16
//! hexadecimal digits; and prints the cell, then each volume, NAME BRANCH, a line each. The
//! cell's other volumes, its SQLite databases and what VMs attach, are not shown. `create-volume`
//! makes a new Fxfs volume NAME of a cell attached, of CAPACITY bytes, its filesystem empty, and
//! prints its branch. `detach` takes the cell's volumes away at once, and returns once their
//! stores are unmounted and written back: once nothing has a file of them open, or a directory of
//! them as its current one; with `--force`, at once, what is open of them failing from then on.
//! `list` prints the cells attached, a line each, TENANT CELL, and under each its volumes, NAME
//! BRANCH, and `detaching` for one being detached.
//!
//! Mounted, it prints MOUNTPOINT on standard output, and serves until it is unmounted: by `umount`
//! or `fusermount3 -u`, or lazily on SIGINT or SIGTERM, once nothing uses it. Then it closes the
//! stores, writing back what is dirty, and exits 0.
//!
//! Root mounts with mount(2), any other user through libfuse's `fusermount3`. With
//! `--allow-other`, users other than the mounting one may use it, as its permissions say: for a
//! user other than root, `/etc/fuse.conf` must have `user_allow_other`.
//!
//! A store is the mount's alone while it is mounted: an extent daemon serving it, or a second
//! mount, fails to open it, as the mount fails to open one in use. Mounting replays the
//! filesystem's journal, which writes to the store: to look at a VM's store and leave it be,
//! mount a fork of it (`fxdaxctl fork`).

#[cfg(target_os = "linux")]
const USAGE: &str = "usage: fxdax-mount [--allow-other] [--new CAPACITY] STORE MOUNTPOINT
       fxdax-mount [--allow-other] --control SOCKET --stores DIR [--bucket URL] MOUNTPOINT
       fxdax-mount attach SOCKET TENANT CELL [TOKEN]
       fxdax-mount create-volume SOCKET TENANT CELL NAME CAPACITY
       fxdax-mount detach [--force] SOCKET TENANT CELL
       fxdax-mount list SOCKET";

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context as _;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let words: Vec<&str> = args.iter().map(|arg| arg.to_str().unwrap_or("")).collect();
    let control = |socket: &str| {
        fxdax::fuse::control::Client::connect(socket).map_err(|err| anyhow::anyhow!("{err:?}"))
    };
    let hex = |what: &str, given: &str| {
        u64::from_str_radix(given, 16).with_context(|| format!("{what} must be hex? {USAGE}"))
    };
    match words.as_slice() {
        ["attach", socket, tenant, cell, rest @ ..] if rest.len() <= 1 => {
            let cell = fxdax::Cell { tenant: hex("TENANT", tenant)?, id: hex("CELL", cell)? };
            let token = rest.first().map(|token| token.parse()).transpose();
            let token = token.with_context(|| format!("TOKEN? {USAGE}"))?;
            let attached = control(socket)?.attach(cell, token);
            let (cell, volumes) = attached.map_err(|err| anyhow::anyhow!("{err:?}"))?;
            println!("{:016x}", cell.id);
            for volume in volumes {
                println!("{} {:016x}", volume.name, volume.branch);
            }
            Ok(())
        }
        ["create-volume", socket, tenant, cell, name, capacity] => {
            let cell = fxdax::Cell { tenant: hex("TENANT", tenant)?, id: hex("CELL", cell)? };
            let capacity = capacity.parse().with_context(|| format!("CAPACITY? {USAGE}"))?;
            let made = control(socket)?.create_volume(cell, name, capacity);
            println!("{:016x}", made.map_err(|err| anyhow::anyhow!("{err:?}"))?.id);
            Ok(())
        }
        ["detach", rest @ ..] => {
            let (force, rest) = match rest {
                ["--force", rest @ ..] => (true, rest),
                rest => (false, rest),
            };
            let [socket, tenant, cell] = rest else { anyhow::bail!(USAGE) };
            let cell = fxdax::Cell { tenant: hex("TENANT", tenant)?, id: hex("CELL", cell)? };
            control(socket)?.detach(cell, force).map_err(|err| anyhow::anyhow!("{err:?}"))
        }
        ["list", socket] => {
            for cell in control(socket)?.list().map_err(|err| anyhow::anyhow!("{err:?}"))? {
                println!("{:016x} {:016x}", cell.tenant, cell.cell);
                for volume in cell.volumes {
                    let detaching = if volume.detaching { " detaching" } else { "" };
                    println!("  {} {:016x}{detaching}", volume.name, volume.branch);
                }
            }
            Ok(())
        }
        _ => mount(args),
    }
}

/// Mounts a store, or a directory of stores, as `args` say, and serves it until it is unmounted.
#[cfg(target_os = "linux")]
fn mount(args: Vec<std::ffi::OsString>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::io::Write as _;
    use std::path::PathBuf;
    fxdax::extentd::logging::to_stderr()?;
    // Before any thread is started, which would take the signals otherwise.
    fxdax::stacks::on_sigquit().context("handling SIGQUIT")?;
    let stop = fxdax::signals::Signals::block(&[libc::SIGINT, libc::SIGTERM])
        .context("handling SIGINT and SIGTERM")?;
    let mut options = fxdax::fuse::MountOptions::new();
    let (mut paths, mut new) = (Vec::new(), None);
    let (mut control, mut stores, mut bucket) = (None, None, None);
    let mut args = args.into_iter();
    let value = |args: &mut std::vec::IntoIter<std::ffi::OsString>, flag: &str| {
        args.next().map(PathBuf::from).with_context(|| format!("{flag} what? {USAGE}"))
    };
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--allow-other") => {
                options.allow_other(true);
            }
            Some("--new") => {
                let capacity = args.next().and_then(|capacity| capacity.to_str()?.parse().ok());
                new = Some(capacity.with_context(|| format!("--new CAPACITY, in bytes? {USAGE}"))?);
            }
            Some("--control") => control = Some(value(&mut args, "--control")?),
            Some("--stores") => stores = Some(value(&mut args, "--stores")?),
            Some("--bucket") => {
                let url = value(&mut args, "--bucket")?.into_os_string().into_string();
                bucket = Some(url.map_err(|_| anyhow::anyhow!("--bucket URL, UTF-8? {USAGE}"))?);
            }
            Some(flag) if flag.starts_with("--") => anyhow::bail!("{flag}? {USAGE}"),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    // Each mount, its unmounter, where it is, and what waits until it is unmounted and closes it.
    type Joined = Box<dyn FnOnce() -> anyhow::Result<()>>;
    let (unmounter, mountpoint, joined): (_, _, Joined) = match (control, stores) {
        (Some(socket), Some(dir)) => {
            let [at] = <[_; 1]>::try_from(paths).map_err(|_| anyhow::anyhow!(USAGE))?;
            anyhow::ensure!(new.is_none(), "--new mounts one store: {USAGE}");
            let host = std::env::var("FXDAX_EXTENTD_REMOTE").ok().filter(|url| !url.is_empty());
            let bucket = bucket.or(host).with_context(|| {
                format!("--bucket URL: no host's bucket is set (FXDAX_EXTENTD_REMOTE). {USAGE}")
            })?;
            let listener = fxdax::extentd::bind_owner_only(&socket)
                .with_context(|| format!("making the control socket {}", socket.display()))?;
            let mount = options
                .mount_stores(&at)
                .with_context(|| format!("mounting a directory of stores at {}", at.display()))?;
            let server = mount.server().clone();
            let stores = fxdax::fuse::control::Stores { dir, bucket };
            fxdax::signals::spawn_taking_no_signals("fxdax-mount control", move || {
                if let Err(error) = fxdax::fuse::control::serve(listener, server, stores) {
                    log::error!("fxdax-mount: serving the control API: {error}");
                }
            })?;
            (mount.unmounter(), at, Box::new(move || mount.join().context("closing the stores")))
        }
        (None, None) => {
            anyhow::ensure!(bucket.is_none(), "--bucket is for --control: {USAGE}");
            let [store, at] = <[_; 2]>::try_from(paths).map_err(|_| anyhow::anyhow!(USAGE))?;
            let fs = match new {
                Some(capacity) => {
                    let storage = fxdax::Storage::create(&store, capacity)
                        .with_context(|| format!("making {}", store.display()))?;
                    futures::executor::block_on(fxdax::Filesystem::format(storage))
                        .with_context(|| format!("formatting {}", store.display()))?
                }
                None => {
                    let storage = fxdax::Storage::open(&store)
                        .with_context(|| format!("opening {}", store.display()))?;
                    futures::executor::block_on(fxdax::Filesystem::mount(storage))
                        .with_context(|| format!("mounting {}'s filesystem", store.display()))?
                }
            };
            let mount = options
                .mount(fs, &at)
                .with_context(|| format!("mounting it at {}", at.display()))?;
            let unmounter = mount.unmounter();
            let joined = move || mount.join().map(drop).context("closing the filesystem");
            (unmounter, at, Box::new(joined))
        }
        _ => anyhow::bail!("--control and --stores go together: {USAGE}"),
    };
    fxdax::signals::spawn_taking_no_signals("fxdax-mount signals", move || match stop.wait() {
        Ok(signal) => {
            log::info!("fxdax-mount: unmounting, on signal {signal}, once nothing uses it");
            if let Err(error) = unmounter.unmount(true) {
                log::error!("fxdax-mount: {error:#}");
            }
        }
        Err(error) => log::error!("fxdax-mount: waiting for SIGINT or SIGTERM: {error}"),
    })?;
    log::info!("fxdax-mount: mounted at {}", mountpoint.display());
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", mountpoint.display())?;
    stdout.flush()?;
    drop(stdout);

    joined()?;
    log::info!("fxdax-mount: {} unmounted and closed", mountpoint.display());
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fxdax-mount runs on Linux only: macOS has no open-source FUSE");
    std::process::exit(2);
}
