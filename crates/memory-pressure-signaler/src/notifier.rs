// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Serves `fuchsia.memorypressure/Provider`: notifies the registered watchers of each level change.
//!
//! Ported from `src/developer/memory/pressure_signaler/pressure_notifier.cc` and
//! `src/developer/memory/pressure_signaler/pressure_notifier.h`.
//!
//! Deviations:
//! - No crash reports on critical memory pressure or imminent OOM, and no simulated levels
//!   (`DebugNotify`).
//! - The provider's dispatcher is a thread of its own running a local executor, which the notifier
//!   lasts as long as.
//! - A watcher is released when a notification to it fails, as its connection is closed: the
//!   in-process connections have no error handler, which upstream releases a watcher from once
//!   its callbacks are done, and could leave it pending forever.

use crate::observer::{Level, PressureObserver};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_memorypressure::{self as fmp, ProviderMarker, ProviderServer, WatcherProxy};
use futures::StreamExt;
use futures::channel::mpsc;
use futures::executor::{LocalPool, LocalSpawner};
use futures::task::LocalSpawnExt;
use log::error;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Converts `level` to the level signalled by the fuchsia.memorypressure service.
fn convert_to_memory_pressure_service_level(level: Level) -> Option<fmp::Level> {
    match level {
        Level::Critical => Some(fmp::Level::Critical),
        Level::Warning => Some(fmp::Level::Warning),
        Level::Normal => Some(fmp::Level::Normal),
        Level::ImminentOom => None,
    }
}

/// The work posted to the provider's dispatcher.
enum Task {
    PostLevelChange,
    RegisterWatcher(WatcherProxy),
}

/// Starts the notifier and its observer, serving `fuchsia.memorypressure/Provider` on `server_end`.
pub(crate) fn start(server_end: ServerEnd<ProviderMarker>) {
    let (tasks, receiver) = mpsc::unbounded();
    let observer_tasks = tasks.clone();
    std::thread::Builder::new()
        .name("memory-pressure-signaler".into())
        .spawn(move || {
            let mut pool = LocalPool::new();
            let notifier = Rc::new(PressureNotifier {
                spawner: pool.spawner(),
                observer: PressureObserver::new(move || {
                    // Posts the notification to the provider's dispatcher, which lasts as long as
                    // the observer.
                    observer_tasks.unbounded_send(Task::PostLevelChange).unwrap();
                }),
                watchers: RefCell::new(Vec::new()),
            });
            pool.run_until(receiver.for_each(|task| {
                match task {
                    Task::PostLevelChange => notifier.post_level_change(),
                    Task::RegisterWatcher(watcher) => notifier.register_watcher(watcher),
                }
                futures::future::ready(())
            }));
        })
        .expect("start the memory pressure signaler");
    server_end.serve(Arc::new(Provider(tasks)));
}

/// The provider protocol, which posts registrations to the provider's dispatcher.
struct Provider(mpsc::UnboundedSender<Task>);

impl ProviderServer for Provider {
    fn register_watcher(&self, watcher: WatcherProxy) {
        self.0.unbounded_send(Task::RegisterWatcher(watcher)).unwrap();
    }
}

struct WatcherState {
    proxy: WatcherProxy,
    level_sent: Level,
    pending_callback: bool,
    needs_free: bool,
}

struct PressureNotifier {
    spawner: LocalSpawner,
    observer: PressureObserver,
    watchers: RefCell<Vec<Rc<RefCell<WatcherState>>>>,
}

impl PressureNotifier {
    fn post_level_change(self: &Rc<Self>) {
        let level_to_send = self.observer.current_level();
        if level_to_send == Level::ImminentOom {
            // Nothing else to do. This is a diagnostic-only level that is not signaled to watchers.
            return;
        }

        // TODO(rashaeqbal): Throttle notifications to prevent thrashing.
        let watchers = self.watchers.borrow().clone();
        for watcher in watchers {
            // Notify the watcher only if we received a response for the previous level change,
            // i.e. there is no pending callback.
            if !watcher.borrow().pending_callback {
                watcher.borrow_mut().pending_callback = true;
                self.notify_watcher(watcher, level_to_send);
            }
        }
    }

    fn notify_watcher(self: &Rc<Self>, watcher: Rc<RefCell<WatcherState>>, level: Level) {
        // We should already have set |pending_callback| when the notification was posted, to
        // prevent removing |WatcherState| from |watchers_| in the error handler.
        assert!(watcher.borrow().pending_callback);

        // We should not be notifying a watcher if |needs_free| is set - indicating that a delayed
        // free is required. This can only happen if there was a pending callback when we tried to
        // release the watcher. No new notifications can be sent out while there is a pending
        // callback. And when the callback is invoked, the |WatcherState| is removed from the
        // |watchers_| vector, so we won't post any new notifications after that.
        assert!(!watcher.borrow().needs_free);

        watcher.borrow_mut().level_sent = level;
        let level = convert_to_memory_pressure_service_level(level).unwrap();
        let proxy = watcher.borrow().proxy.clone();
        let this = self.clone();
        self.spawner
            .spawn_local(async move {
                match proxy.on_level_changed(level).await {
                    Ok(()) => this.on_level_changed_callback(&watcher),
                    Err(error) => {
                        error!("Failed to signal pressure change: {error}");
                        // The watcher's connection is closed.
                        watcher.borrow_mut().pending_callback = false;
                        this.release_watcher(&watcher);
                    }
                }
            })
            .expect("the provider's dispatcher is running");
    }

    fn on_level_changed_callback(self: &Rc<Self>, watcher: &Rc<RefCell<WatcherState>>) {
        watcher.borrow_mut().pending_callback = false;

        // The error handler invoked ReleaseWatcher(), but we could not free the |WatcherState|
        // because of this outstanding callback. It is safe to free the watcher now. There are no
        // more outstanding callbacks, and no new notifications (since a new notification is posted
        // only if there is no pending callback).
        if watcher.borrow().needs_free {
            self.release_watcher(watcher);
            return;
        }

        let current_level = self.observer.current_level_for_watcher();
        // The watcher might have missed a level change if it occurred before this callback. If the
        // level has changed, notify the watcher.
        if watcher.borrow().level_sent != current_level {
            watcher.borrow_mut().pending_callback = true;
            self.notify_watcher(watcher.clone(), current_level);
        }
    }

    fn register_watcher(self: &Rc<Self>, proxy: WatcherProxy) {
        let current_level = self.observer.current_level_for_watcher();
        // Create the watcher's state, which will hold the client.
        let watcher = Rc::new(RefCell::new(WatcherState {
            proxy,
            level_sent: current_level,
            pending_callback: false,
            needs_free: false,
        }));
        self.watchers.borrow_mut().push(watcher.clone());

        // Set |pending_callback| and notify the current level.
        watcher.borrow_mut().pending_callback = true;
        self.notify_watcher(watcher, current_level);
    }

    fn release_watcher(&self, watcher: &Rc<RefCell<WatcherState>>) {
        let mut watchers = self.watchers.borrow_mut();
        let Some(index) = watchers.iter().position(|target| Rc::ptr_eq(target, watcher)) else {
            // Not found.
            return;
        };

        // There is a pending callback. Freeing the watcher now could lose it. Set |needs_free| to
        // indicate that we need a delayed free, when the pending callback is executed.
        if watchers[index].borrow().pending_callback {
            watchers[index].borrow_mut().needs_free = true;
        } else {
            watchers.remove(index);
        }
    }
}
