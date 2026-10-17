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
