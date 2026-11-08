// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A guest's automounter of shared volumes (backlog doc-10, TASK-136.6): see the library's docs.
//!
//! ```sh
//! fxfs-automount ROOT FILESYSTEM [--tag TAG] [--timeout SECONDS] [--port PORT]
//! ```
//!
//! mounts an autofs map on the directory ROOT, whose entries' lookups mount the volumes of those
//! names of the guest's Fxfs on the device TAG (`extents`), which it imports through the volume
//! mounted at FILESYSTEM once the sandbox manager attached them, asked over vsock at the host's
//! PORT (5054). Idle for SECONDS (600), a mount expires.

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    fxfs_automount::main(std::env::args().skip(1))
}

#[cfg(not(target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("fxfs-automount runs in a Linux guest")
}
