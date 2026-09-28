// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The modules of `src/storage/fxfs/platform/src/fuchsia.rs` that back files with the in-process
// kernel's pager and serve directories and symlinks. The directory entry cache, directories, the
// node cache, the pager and symlinks are built from the vendored sources (directories, paged
// object handles and the pager as scripts/kernel-cfg.py builds them for the kernel); the rest are
// excerpts and stand-ins, which say what they leave out.

#[path = "../../../vendor/fuchsia-kernel/src/storage/fxfs/platform/src/fuchsia/directory.rs"]
pub mod directory;
#[path = "../../../vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/dirent_cache.rs"]
mod dirent_cache;
mod errors;
pub mod file;
#[path = "../../../vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/node.rs"]
pub mod node;
#[path = "../../../vendor/fuchsia-kernel/src/storage/fxfs/platform/src/fuchsia/paged_object_handle.rs"]
mod paged_object_handle;
#[path = "../../../vendor/fuchsia-kernel/src/storage/fxfs/platform/src/fuchsia/pager.rs"]
pub mod pager;
pub mod profile;
#[path = "../../../vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/symlink.rs"]
mod symlink;
pub mod volume;
pub mod volumes_directory;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod directory_tests;
