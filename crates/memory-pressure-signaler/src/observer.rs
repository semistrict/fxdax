// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Watches the kernel's memory pressure events.
//!
//! Ported from `src/developer/memory/pressure_signaler/pressure_observer.cc` and
//! `src/developer/memory/pressure_signaler/pressure_observer.h`.
//!
//! Deviations:
//! - There are no jobs, so the events are got without the root job, which the kernel doesn't check.
//! - The observer always watches for changes, and its thread lasts as long as the process.
//! - Level changes are logged without the events' handles, and not traced.

use log::{error, info};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// A kernel memory pressure level, the most severe first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    ImminentOom,
    Critical,
    Warning,
    Normal,
}

const NUM_LEVELS: usize = 4;

impl Level {
    const ALL: [Level; NUM_LEVELS] =
        [Level::ImminentOom, Level::Critical, Level::Warning, Level::Normal];

    fn name(self) -> &'static str {
        match self {
            Level::ImminentOom => "IMMINENT-OOM",
            Level::Critical => "CRITICAL",
            Level::Warning => "WARNING",
            Level::Normal => "NORMAL",
        }
    }
}

/// Observes the kernel's memory pressure level.
pub(crate) struct PressureObserver {
    // Start off with Normal level before the right kernel level has been discovered, so that
    // PressureNotifier notifies clients with a valid level until the level has been initialized.
    //
    // We can end up in this unitialized state if a watcher registers before the PressureObserver
    // has discovered the initial system memory pressure level. Since watcher registration is
    // supposed to return the current level, advertize the current level as Normal. This is fine
    // bacause when we do initialize the level, we will send another signal if it was not Normal.
    level: Arc<AtomicU8>,
}

impl PressureObserver {
    /// Watches for memory pressure changes on a thread of its own, calling `notify` on each.
    pub(crate) fn new(notify: impl Fn() + Send + 'static) -> Self {
        let level = Arc::new(AtomicU8::new(Level::Normal as u8));
        if let Ok(events) = init_mem_pressure_events() {
            // Set up a new thread (memory-pressure-loop) that watches for memory pressure changes
            // from the kernel. All this thread does is wait on memory pressure events in a loop,
            // hence is kept separate from the notifier's thread.
            let watch = Watch {
                events,
                wait_order: [0, 1, 2, 3],
                level: level.clone(),
                level_initialized: false,
                notify,
            };
            std::thread::Builder::new()
                .name("memory-pressure-loop".into())
                .spawn(move || watch.watch_for_changes())
                .expect("start the memory pressure loop");
        }
        Self { level }
    }

    pub(crate) fn current_level(&self) -> Level {
        Level::ALL[self.level.load(Ordering::Relaxed) as usize]
    }

    pub(crate) fn current_level_for_watcher(&self) -> Level {
        // Watchers of the memory pressure service do not recognize the Imminent-OOM level, the
        // highest level that they can receive is Critical.
        match self.current_level() {
            Level::ImminentOom => Level::Critical,
            level => level,
        }
    }
}

/// Gets the kernel's memory pressure events, by level.
fn init_mem_pressure_events() -> Result<[zx::Event; NUM_LEVELS], zx::Status> {
    // There are no jobs: the kernel doesn't check the root job.
    let root_job = zx::Job::from(zx::NullableHandle::invalid());
    let get = |kind, name| {
        zx::system_get_event(&root_job, kind).inspect_err(|status| {
            error!("zx_system_get_event [{name}] returned {status}");
        })
    };
    Ok([
        get(zx::SystemEventKind::ImminentOutOfMemory, "IMMINENT-OOM")?,
        get(zx::SystemEventKind::MemoryPressureCritical, "CRITICAL")?,
        get(zx::SystemEventKind::MemoryPressureWarning, "WARNING")?,
        get(zx::SystemEventKind::MemoryPressureNormal, "NORMAL")?,
    ])
}

/// The state of the memory pressure loop.
struct Watch<F> {
    /// The events, by level.
    events: [zx::Event; NUM_LEVELS],
    /// The levels whose events are waited on, in the order they are waited on: the level last
    /// signaled is last, and not waited on.
    wait_order: [usize; NUM_LEVELS],
    level: Arc<AtomicU8>,
    level_initialized: bool,
    notify: F,
}

impl<F: Fn()> Watch<F> {
    fn watch_for_changes(mut self) {
        loop {
            self.wait_on_level_change();
        }
    }

    fn wait_on_level_change(&mut self) {
        // Wait on all events the first time around.
        let num_wait_items = if self.level_initialized { NUM_LEVELS - 1 } else { NUM_LEVELS };
        let mut wait_items: Vec<_> = self.wait_order[..num_wait_items]
            .iter()
            .map(|&level| self.events[level].wait_item(zx::Signals::EVENT_SIGNALED))
            .collect();

        if let Err(status) = zx::object_wait_many(&mut wait_items, zx::MonotonicInstant::INFINITE) {
            error!("zx_object_wait_many returned {status}");
            return;
        }

        let signaled =
            wait_items.iter().position(|item| item.pending().contains(zx::Signals::EVENT_SIGNALED));
        drop(wait_items);
        if let Some(i) = signaled {
            self.on_level_changed(Level::ALL[self.wait_order[i]]);

            // Move the event currently asserted to the end of the array.
            // Wait on only the first |kNumLevels| - 1 items next time around.
            self.wait_order.swap(i, NUM_LEVELS - 1);
        }
    }

    fn on_level_changed(&mut self, level: Level) {
        let old_level = Level::ALL[self.level.swap(level as u8, Ordering::Relaxed) as usize];

        if !self.level_initialized {
            // Record that the level has been initialized if this is the first time. Before this,
            // the notifier will advertise the pressure level as Normal when watchers register.
            // This is fine because once the level is initialized, we will send out another
            // pressure signal if required (i.e. if the pressure level is not Normal).
            self.level_initialized = true;
            info!("starting at {}", level.name());
        } else {
            info!("{} -> {}", old_level.name(), level.name());
        }

        // Notify the notifier that the level has changed, which simply causes a notification task
        // to be queued on the notifier's thread.
        (self.notify)();
    }
}
