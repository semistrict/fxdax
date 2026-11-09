// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: SQLite databases kept straight on an extent device, with no filesystem between
//! (TASK-296): a [`Volume`] on a `storage_device::Device`, the interface Fxfs keeps itself on,
//! registered as a SQLite VFS ([`register`]).
//!
//! A database's file is pages in write-once extents, which a tree in the same extents names
//! (`pages`); a transaction is one commit of the device, the root naming each file's tree
//! (`volume`). So a crash leaves each database as of one of its transactions, with no journal to
//! roll back, and what the device keeps of its commits, forks and views of them, it keeps of the
//! databases too.

mod format;
mod locks;
mod pages;
mod vfs;
mod volume;

pub use vfs::{Registration, register};
pub use volume::{Failed, NotFound, Options, Volume};
