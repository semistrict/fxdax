// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A guest's automounter of shared volumes (backlog doc-10, Automount; TASK-136.6): a lookup of a
//! name under a directory, `/repos/linux`, has the sandbox manager attach the base of that name
//! to the sandbox's store, imports it as a volume of the guest's Fxfs, of that name, and mounts
//! it there, the lookup returning once it is mounted. A name the sandbox manager refuses fails the
//! lookup (`ENOENT`), leaving nothing mounted. A volume imported already, by an earlier lookup, or
//! before a reboot, mounts with no ask. A mount idle for the timeout expires: it is unmounted, and
//! its volume closed, what it holds in memory going and its writable layers staying, and its next
//! lookup mounts it again.
//!
//! The kernel's side is Linux's autofs (protocol 5, an indirect map), which is `d_automount`'s own
//! client: it holds every lookup of a name on one wait while this mounts it, so that concurrent
//! lookups of a repo attach it once, lets this process's group alone through the directory
//! unmounted, and finds the mounts idle for the timeout, which this unmounts as it asks. No kernel
//! code of fxdax's is involved.
//!
//! The sandbox manager is asked through [`upcall`]'s protocol, over any stream: the binary,
//! `fxfs-automount`, connects to the host over vsock for each ask, as the guest agent does, and
//! asks again on a new connection while a restore from a snapshot resets it ([`reconnect`],
//! TASK-277). The crate's other binary, `git-remote-fxfs` ([`git`]), moves a repo's fetches to the
//! newest generation of it (TASK-136.7).

pub mod git;
pub mod reconnect;
pub mod upcall;

#[cfg(target_os = "linux")]
mod autofs;
mod daemon;
#[cfg(target_os = "linux")]
mod linux;

pub use daemon::{Attach, Automounter, Volumes};
#[cfg(target_os = "linux")]
pub use linux::{Config, Fxfs, Vsock, git_main, main, run};

/// The host's vsock port the sandbox manager answers on, unless told otherwise.
pub const PORT: u32 = 5054;

/// Whether `name` may name a shared volume: 1 to 64 of ASCII letters, digits, `-`, `_` and `.`,
/// not first, and not the filesystem's own default volume.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name != "default"
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::valid_name;

    #[test]
    fn names_of_shared_volumes() {
        for name in ["linux", "github.com_torvalds_linux", "a", "a-b_c.d", &"x".repeat(64)] {
            assert!(valid_name(name), "{name}");
        }
        for name in ["", ".", "..", ".hidden", "default", "a/b", "a b", "é", &"x".repeat(65)] {
            assert!(!valid_name(name), "{name}");
        }
    }
}
