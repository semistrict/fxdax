// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Git's remote helper for repos kept as shared volumes (backlog doc-10, TASK-136.7): see the
//! library's `git` module.

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    fxfs_automount::git_main(std::env::args().skip(1))
}

#[cfg(not(target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("git-remote-fxfs runs in a Linux guest")
}
