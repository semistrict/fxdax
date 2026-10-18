// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The modules of `src/storage/fxfs/platform/src/fuchsia.rs` that back files with the in-process
// kernel's pager and serve directories and symlinks. The directory entry cache, directories, the
// node cache, the pager and symlinks are copied from Fuchsia (directories, paged object handles
// and the pager with the deviations they list); the rest are excerpts and stand-ins, which say
// what they leave out. `shared_blocks` is fxdax's own; `fxextent` is Fuchsia's `fxblob`'s `FxBlob`,
// as an extent of an extent store. fxdax: symlinks are public, for their attributes
// (`FxSymlink::update_attributes`).

pub mod directory;
mod dirent_cache;
mod errors;
pub mod file;
pub mod fxextent;
pub mod memory_pressure;
pub mod node;
mod paged_object_handle;
pub mod pager;
pub mod profile;
pub mod shared_blocks;
pub mod symlink;
pub mod volume;
pub mod volumes_directory;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod directory_tests;
