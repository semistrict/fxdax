// Copyright The libkrun Authors.
// Copyright 2026 The fxdax Authors: changed for fxdax's guest tests.
// SPDX-License-Identifier: Apache-2.0

//! Common utilities used by multiple test

use anyhow::Context;
use std::fs;
use std::fs::create_dir;
use std::path::{Path, PathBuf};

use crate::{ShouldRun, TestSetup};

fn copy_guest_agent(dir: &Path) -> anyhow::Result<()> {
    let path = std::env::var_os("FXDAX_GUEST_AGENT")
        .context("FXDAX_GUEST_AGENT does not name the guest agent")?;

    let output_path = dir.join("guest-agent");
    fs::copy(path, output_path).context("Failed to copy executable into vm")?;
    Ok(())
}

/// Creates the root filesystem directory, if the runner didn't, and copies the guest agent into
/// it. Returns the path to the root directory.
pub fn setup_rootfs(test_setup: &TestSetup) -> anyhow::Result<PathBuf> {
    let root_dir = test_setup.tmp_dir.join("rootfs");
    if !root_dir.exists() {
        create_dir(&root_dir).context("Failed to create rootfs directory")?;
    }
    copy_guest_agent(&root_dir)?;
    Ok(root_dir)
}

/// Whether a test of fxdax's guest kernel's Fxfs runs: FXDAX_FIRECRACKER names the VMM, and
/// FXDAX_GUEST_KERNEL the kernel, and FXDAX_EXTENTD the socket of the extent daemon that serves
/// the device's store (as fxdax's scripts/test-e2e.sh has them).
pub fn fxdax_guest_should_run() -> ShouldRun {
    if !crate::vm::has_vmm() {
        return ShouldRun::No("FXDAX_FIRECRACKER does not name fxdax's Firecracker");
    }
    if std::env::var_os("FXDAX_GUEST_KERNEL").is_none() {
        return ShouldRun::No("FXDAX_GUEST_KERNEL does not name a guest kernel");
    }
    if std::env::var_os("FXDAX_EXTENTD").is_none() {
        return ShouldRun::No("FXDAX_EXTENTD does not name the extent daemon's socket");
    }
    ShouldRun::Yes
}

/// An extent daemon of the test's own, FXDAX_EXTENTD_BIN, serving at `socket`, and its control API
/// beside it ([`control`]), once this returns, with `env` in its environment, logging to `<name>.log` in the test's directory.
pub fn spawn_daemon(
    test_setup: &TestSetup,
    socket: &Path,
    name: &str,
    env: &[(&str, &std::ffi::OsStr)],
) -> anyhow::Result<std::process::Child> {
    use std::time::{Duration, Instant};

    let bin = std::env::var_os("FXDAX_EXTENTD_BIN").context("FXDAX_EXTENTD_BIN")?;
    let log = fs::File::create(test_setup.tmp_dir.join(format!("{name}.log")))?;
    let daemon = std::process::Command::new(bin)
        .arg(socket)
        .env("FXDAX_EXTENTD_GRPC", socket.with_extension("control"))
        .envs(env.iter().copied())
        .stderr(log)
        .spawn()?;
    test_setup.register_cleanup_pid(daemon.id());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match control(socket) {
            Ok(_) => return Ok(daemon),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(anyhow::anyhow!("the daemon {name}: {err:#}")),
        }
    }
}

/// A client of the control API of the extent daemon whose VM socket is at `socket`: at its control
/// socket, beside it, `<socket>.control` for `<socket>.sock`, as fxdax's scripts/test-e2e.sh and
/// [`spawn_daemon`] place it.
pub fn control(socket: &Path) -> Result<fxdax::extentd::control::Client, fxdax::Error> {
    fxdax::extentd::control::Client::connect(socket.with_extension("control"))
}

/// Copies the directory `from` to `to`, which must not exist, with everything in it.
pub fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    create_dir(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}
