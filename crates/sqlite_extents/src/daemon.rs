// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A volume on a store the host's extent daemon serves (TASK-300): its reads, and its window's
//! pages, are the daemon's cache's, the one copy of each extent every store and VM it serves
//! shares; its writes and commits are the daemon's requests, as a VM's are.
//!
//! The window is a DAX window in this process's memory, as a VM process's is (doc-13): the daemon
//! maps its cache's pages there, read-only, and this process's faults there go to it, through
//! userfaultfd. So a page SQLite reads in place is the cache's page itself.

use crate::window::{Contents, Run, Window};
use crate::{Options, Volume};
use anyhow::{Context as _, Error, ensure};
use fxdax::{MappingContents, RemoteDevice, RemoteExtentStore, WindowMapping};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;
use storage_device::{Device, DeviceHolder};

/// A volume on the store at `store` that the daemon listening at `socket` serves, made with
/// `capacity` bytes if it doesn't exist, its files mapped into a window of `window` bytes of this
/// process's address space, which takes memory only as the daemon maps its pages there.
pub fn open(
    socket: &Path,
    store: &Path,
    capacity: u64,
    window: u64,
    options: Options,
) -> Result<Arc<Volume>, Error> {
    // A SQLite volume of its cell, if the daemon makes it (TASK-364): one the FUSE mounts of its
    // cell leave be, as no filesystem's.
    let opened = fxdax::DaemonConnection::connect(socket)
        .and_then(|daemon| daemon.open_sqlite(store, capacity));
    let store = opened.map_err(Error::new).with_context(|| {
        format!("opening {} on the daemon at {}", store.display(), socket.display())
    })?;
    let store = Arc::new(store);
    let window = DaemonWindow::new(store.clone(), window)?;
    let device = DeviceHolder::new(RemoteDevice::new(store)?);
    Volume::open_mapped(device, Box::new(window), options)
}

/// A store's DAX window in this process's memory. See the module docs.
pub struct DaemonWindow {
    /// The store, which goes before the memory does.
    store: Option<Arc<RemoteExtentStore>>,
    memory: NonNull<u8>,
    size: u64,
}

// SAFETY: the memory is the daemon's to change, through the store, whose requests may come from
// any thread; this process only reads it.
unsafe impl Send for DaemonWindow {}
unsafe impl Sync for DaemonWindow {}

impl DaemonWindow {
    /// `store`'s window, in `size` bytes of new address space.
    pub fn new(store: Arc<RemoteExtentStore>, size: u64) -> Result<Self, Error> {
        // SAFETY: new anonymous memory, which the window takes over.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        ensure!(
            memory != libc::MAP_FAILED,
            "mapping {size} bytes for the window: {}",
            std::io::Error::last_os_error()
        );
        let memory = NonNull::new(memory.cast::<u8>()).expect("a mapping");
        let window = Self { store: Some(store), memory, size };
        // A writer's window copies on write, of one private page at least; this process never
        // writes it, so that none is ever made.
        // SAFETY: the memory is this window's alone, and outlives the store (`Drop`).
        unsafe { window.store().create_dax_window_in_host_memory(memory, size, None, 1) }
            .map_err(Error::new)
            .context("creating the window")?;
        Ok(window)
    }

    fn store(&self) -> &RemoteExtentStore {
        self.store.as_ref().expect("a store until dropped")
    }
}

impl Window for DaemonWindow {
    fn memory(&self) -> (NonNull<u8>, u64) {
        (self.memory, self.size)
    }

    fn map(&self, _device: &dyn Device, runs: &[Run]) -> Result<(), Error> {
        let mappings: Vec<_> = (runs.iter())
            .map(|run| WindowMapping {
                window_offset: run.at,
                len: run.len,
                contents: match run.contents {
                    Contents::Extent { extent, offset } => MappingContents::Extent {
                        extent: fxdax::ExtentId {
                            branch: extent.branch,
                            sequence: extent.sequence,
                        },
                        offset,
                    },
                    Contents::Zeros => MappingContents::Zeros,
                },
            })
            .collect();
        self.store().map_many(&mappings).map_err(Error::new).context("mapping the window")
    }

    fn unmap(&self, at: u64, len: u64) -> Result<(), Error> {
        self.store().unmap(at, len).map_err(Error::new).context("unmapping the window")
    }
}

impl Drop for DaemonWindow {
    fn drop(&mut self) {
        // The memory goes once the store, whose fault thread serves it, has: unless something
        // holds the store still, when the memory stays, as the store may yet map it.
        match Arc::try_unwrap(self.store.take().expect("a store")) {
            Ok(store) => drop(store),
            Err(_) => {
                log::warn!("sqlite_extents: a window's store outlives it: its memory stays mapped");
                return;
            }
        }
        // SAFETY: the window's memory, which nothing uses with the store gone.
        unsafe { libc::munmap(self.memory.as_ptr().cast(), self.size as usize) };
    }
}
