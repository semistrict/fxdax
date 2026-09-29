// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// `src/storage/fxfs/platform/src/fuchsia/memory_pressure.rs`, copied verbatim except as follows.
//
// - The monitor serves fuchsia.memorypressure/Watcher in-process (see the `fidl` stand-in): the
//   watcher's calls update the level, rather than a task reading them from a request stream, and
//   dropping the monitor closes the connection, as dropping that task does.
// - The provider it registers with is the process's memory pressure signaler (see the
//   `memory-pressure-signaler` crate), rather than a component it connects to.

use event_listener::{Event, EventListener};
use fidl::endpoints::{ServerEnd, Serving, create_proxy};
use fidl_fuchsia_memorypressure::{WatcherMarker, WatcherServer};
use futures::FutureExt;
use futures::future::BoxFuture;
use futures::stream::{FusedStream, Stream};
use futures::task::{self, Poll};
use fxfs::log::*;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

pub use fidl_fuchsia_memorypressure::Level as MemoryPressureLevel;

/// Represents a connection to a system service that provides memory pressure updates.
///
/// Upon dropping, all memory pressure updates will be terminated.
pub struct MemoryPressureMonitor {
    inner: Arc<Inner>,

    /// The connection on which [`Inner`] receives memory pressure updates.
    ///
    /// This is held so that dropping this will also close the connection.
    _watch_task_handle: Serving,
}

impl MemoryPressureMonitor {
    /// Connect and start listening to the memory pressure updates from the system service.
    ///
    /// Call `[MemoryPressureMonitor::get_level_stream()`] to get an async [`Stream`] of memory
    /// pressure updates.
    pub fn start() -> Result<Self, anyhow::Error> {
        debug!("Attempting to connect to fuchsia.memorypressure/Provider");
        let provider = memory_pressure_signaler::provider();
        debug!("Successfully connected to fuchsia.memorypressure/Provider");

        let (watcher_client, watcher_server) = create_proxy::<WatcherMarker>();
        provider.register_watcher(watcher_client)?;

        debug!("Successfully registered as a fuchsia.memorypressure/Watcher");

        Self::try_from(watcher_server)
    }

    /// Gets the current memory pressure level.
    pub fn level(&self) -> MemoryPressureLevel {
        self.inner.level()
    }

    /// Returns a [`Stream`] that provides memory pressure updates.
    pub fn get_level_stream(&self) -> MemoryPressureLevelStream {
        MemoryPressureLevelStream::new(self.inner.clone())
    }
}

impl TryFrom<ServerEnd<WatcherMarker>> for MemoryPressureMonitor {
    type Error = anyhow::Error;

    /// Creates an instance of [`MemoryPressureMonitor`] from the server end of a
    /// `fuchsia.memorypressure.Watcher` connection and starts listening.
    fn try_from(watcher_server: ServerEnd<WatcherMarker>) -> Result<Self, Self::Error> {
        let inner = Inner::new();

        // Serve the connection to continuously receive memory pressure updates.
        let watch_task_handle =
            watcher_server.serve_until_dropped(inner.clone() as Arc<dyn WatcherServer>);

        Ok(Self { inner, _watch_task_handle: watch_task_handle })
    }
}

struct Inner {
    level: AtomicU32,

    /// An event that is signaled by the [`MemoryPressureMonitor`] when there is a new memory
    /// pressure update.
    event: Event,
}

impl Inner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            level: AtomicU32::new(MemoryPressureLevel::Normal.into_primitive()),
            event: Event::new(),
        })
    }

    /// Gets the current memory pressure level.
    fn level(&self) -> MemoryPressureLevel {
        let raw_level = self.level.load(Ordering::Acquire);
        MemoryPressureLevel::from_primitive(raw_level).expect("Unexpected memory level value")
    }

    /// Updates the current memory pressure level.
    fn update_level(&self, level: MemoryPressureLevel) {
        let level = level.into_primitive();
        if self.level.swap(level, Ordering::AcqRel) != level {
            // Notify relaxed since reading from `self.level` is appropriately ordered
            self.event.notify_relaxed(usize::MAX);
        }
    }

    fn listen(&self) -> EventListener {
        self.event.listen()
    }
}

impl WatcherServer for Inner {
    /// Processes a new memory pressure level update.
    fn on_level_changed(&self, level: MemoryPressureLevel) -> BoxFuture<'_, ()> {
        fxfs_trace::duration!("MemoryPressureMonitor::process_level_update");
        self.update_level(level);
        futures::future::ready(()).boxed()
    }
}

pub struct MemoryPressureLevelStream {
    inner: Arc<Inner>,
    prev_level: MemoryPressureLevel,
    listener: Option<EventListener>,
}

impl MemoryPressureLevelStream {
    fn new(inner: Arc<Inner>) -> Self {
        Self { inner, prev_level: MemoryPressureLevel::Normal, listener: None }
    }
}

impl Stream for MemoryPressureLevelStream {
    type Item = MemoryPressureLevel;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(listener) = &mut self.listener {
                futures::ready!(listener.poll_unpin(cx));
            }
            // Immediately reset the listener
            self.listener = Some(self.inner.listen());

            let level = self.inner.level();
            if level != self.prev_level {
                self.prev_level = level;
                return Poll::Ready(Some(level));
            }
        }
    }
}

impl FusedStream for MemoryPressureLevelStream {
    fn is_terminated(&self) -> bool {
        // This never terminates.
        false
    }
}
