//! Watches free memory, signaling memory pressure and triggering eviction as it runs low.
//!
//! Ported from `zircon/kernel/object/memory_watchdog.cc` and
//! `zircon/kernel/object/include/object/memory_watchdog.h`.
//!
//! Deviations:
//! - The watchdog is a lazily initialized singleton, started with the PMM rather than by the
//!   executor.
//! - There are no jobs, so killing jobs on running out of memory kills nothing.
//! - Rebooting on running out of memory aborts the host process, after waiting the timeout, as
//!   there is no user mode to acknowledge it or debuglog to flush.
//! - No kcounters, page queue dumps or memory stall statistics.

use super::dispatcher::Dispatcher;
use super::event_dispatcher::EventDispatcher;
use crate::boot_options::{BootOptions, OomBehavior};
use crate::kernel::deadline::{Deadline, current_mono_time};
use crate::kernel::event::Event;
use crate::kernel::timer::Timer;
use crate::vm::evictor::{EvictionLevel, Output, TriggerReason};
use crate::vm::pmm::{self, PmmNode};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use zx_types::{
    ZX_EVENT_SIGNALED, ZX_SYSTEM_EVENT_IMMINENT_OUT_OF_MEMORY,
    ZX_SYSTEM_EVENT_MEMORY_PRESSURE_CRITICAL, ZX_SYSTEM_EVENT_MEMORY_PRESSURE_NORMAL,
    ZX_SYSTEM_EVENT_MEMORY_PRESSURE_WARNING, ZX_SYSTEM_EVENT_OUT_OF_MEMORY, zx_duration_mono_t,
    zx_instant_mono_t,
};

const PAGE_SIZE: u64 = page::SIZE as u64;
const MB: u64 = 1024 * 1024;
const NSEC_PER_MSEC: i64 = 1_000_000;
const NSEC_PER_SEC: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PressureLevel {
    OutOfMemory = 0,
    ImminentOutOfMemory,
    Critical,
    Warning,
    Normal,
}

const NUM_LEVELS: usize = 5;
const NUM_WATERMARKS: usize = NUM_LEVELS - 1;

impl PressureLevel {
    fn from_index(index: u8) -> Self {
        match index {
            0 => Self::OutOfMemory,
            1 => Self::ImminentOutOfMemory,
            2 => Self::Critical,
            3 => Self::Warning,
            4 => Self::Normal,
            _ => unreachable!("pressure level {index}"),
        }
    }

    fn to_str(self) -> &'static str {
        match self {
            Self::OutOfMemory => "OutOfMemory",
            Self::ImminentOutOfMemory => "ImminentOutOfMemory",
            Self::Critical => "Critical",
            Self::Warning => "Warning",
            Self::Normal => "Normal",
        }
    }
}

/// OneShot eviction strategy only triggers eviction events at memory pressure state transitions.
/// Continuous eviction strategy enables continuous background eviction as long the system remains
/// under memory pressure (i.e. at a memory pressure level that is eligible for eviction).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvictionStrategy {
    OneShot,
    Continuous,
}

/// The state only the worker thread uses.
struct WorkerState {
    prev_mem_event_idx: PressureLevel,
    /// Tracks last time the memory state was evaluated (and signaled if required).
    prev_mem_state_eval_time: zx_instant_mono_t,
}

/// Object is thread safe.
pub struct MemoryWatchdog {
    /// Kernel-owned events used to signal userspace at different levels of memory pressure.
    mem_pressure_events: [Arc<EventDispatcher>; NUM_LEVELS],

    /// Event used for communicating memory related state changes between other parts of the
    /// system and the WorkerThread. It unsignals as it releases a waiter.
    mem_state_signal: Arc<Event>,

    /// Relaxed atomic so that debug methods can safely read it.
    mem_event_idx: AtomicU8,

    /// Watermark information is not modified after init and so is safe for multiple threads to
    /// access.
    mem_watermarks: [u64; NUM_WATERMARKS],
    watermark_debounce: u64,

    /// Used to delay signaling memory level transitions in the case of rapid changes.
    hysteresis: zx_duration_mono_t,

    /// Used to delay eviction when going from a pressure level that does not require eviction to
    /// one that does.
    eviction_delay: zx_duration_mono_t,

    /// The highest pressure level we trigger eviction at, OOM being the lowest pressure level (0).
    max_eviction_level: PressureLevel,

    /// The free memory target to aim for when we trigger eviction.
    free_mem_target: u64,

    /// Current minimum amount of memory we want the triggered eviction to reclaim. Only set by the
    /// worker thread while `eviction_trigger` is canceled.
    min_free_target: AtomicU64,

    /// A timer is used to trigger eviction so that user space is given a chance to act upon a
    /// memory pressure signal first.
    eviction_trigger: Timer,

    /// Tracks whether or not continuous eviction is presently happening or not. This value is only
    /// allowed to be set to true by the `eviction_trigger` callback, and is only allowed to be set
    /// to false by the worker thread, preventing any races in transitioning its state.
    continuous_eviction_active: AtomicBool,

    eviction_strategy: EvictionStrategy,
}

static MEMORY_WATCHDOG: OnceLock<MemoryWatchdog> = OnceLock::new();

/// Returns the memory watchdog, which must have been initialized with `init`.
pub fn get_memory_watchdog() -> &'static MemoryWatchdog {
    MEMORY_WATCHDOG.get().expect("memory watchdog initialized")
}

/// Initializes the memory watchdog of `pmm`, starting its worker if the OOM response is enabled.
pub fn init(pmm: &'static PmmNode) {
    let mut started = false;
    let watchdog = MEMORY_WATCHDOG.get_or_init(|| {
        started = true;
        MemoryWatchdog::new(pmm)
    });
    if started && BootOptions::get().oom_enabled {
        std::thread::Builder::new()
            .name("memory-pressure-thread".into())
            .spawn(move || watchdog.worker_thread())
            .expect("create memory pressure thread");
    }
}

fn zx_time_add_duration(
    time: zx_instant_mono_t,
    duration: zx_duration_mono_t,
) -> zx_instant_mono_t {
    time.saturating_add(duration)
}

fn handle_on_oom_reboot() -> ! {
    // Notify the pmm that although we are out of memory, we would like to never wait for memory.
    // This ensures that if userspace needs to allocate to do a graceful shutdown it is able to.
    pmm::node().stop_returning_should_wait();

    let timeout_ms = BootOptions::get().oom_timeout_ms;
    eprintln!("memory-pressure: pausing for {timeout_ms}ms after OOM mem signal");
    let _ = Event::new(false).wait(&Deadline::after_mono(i64::from(timeout_ms) * NSEC_PER_MSEC));
    eprintln!(
        "memory-pressure: rebooting due to OOM. timed out after waiting {timeout_ms}ms for \
         user-mode ack."
    );
    eprintln!("memory-pressure: stowing crashlog\nZIRCON REBOOT REASON (OOM)");
    std::process::abort();
}

impl MemoryWatchdog {
    fn new(pmm: &'static PmmNode) -> Self {
        let options = BootOptions::get();
        let mem_pressure_events = std::array::from_fn(|i| {
            let level = PressureLevel::from_index(i as u8);
            match EventDispatcher::create(0) {
                Ok((event, _rights)) => event.release(),
                Err(status) => panic!(
                    "memory-pressure: create memory event {} failed: {status}",
                    level.to_str()
                ),
            }
        });

        let mut watchdog = Self {
            mem_pressure_events,
            mem_state_signal: Arc::new(Event::new_autounsignal(false)),
            mem_event_idx: AtomicU8::new(PressureLevel::Normal as u8),
            mem_watermarks: [0; NUM_WATERMARKS],
            watermark_debounce: 0,
            hysteresis: 10 * NSEC_PER_SEC,
            eviction_delay: 5000 * NSEC_PER_MSEC,
            max_eviction_level: PressureLevel::Critical,
            free_mem_target: 0,
            min_free_target: AtomicU64::new(0),
            eviction_trigger: Timer::new(),
            continuous_eviction_active: AtomicBool::new(false),
            eviction_strategy: EvictionStrategy::OneShot,
        };

        if options.oom_enabled {
            let watermarks = &mut watchdog.mem_watermarks;
            watermarks[PressureLevel::OutOfMemory as usize] =
                options.oom_out_of_memory_threshold_mb * MB;
            watermarks[PressureLevel::ImminentOutOfMemory as usize] = watermarks
                [PressureLevel::OutOfMemory as usize]
                + options.oom_imminent_oom_delta_mb * MB;
            watermarks[PressureLevel::Critical as usize] = options.oom_critical_threshold_mb * MB;
            watermarks[PressureLevel::Warning as usize] = options.oom_warning_threshold_mb * MB;

            watchdog.watermark_debounce = options.oom_debounce_mb * MB;
            if options.oom_evict_at_warning {
                watchdog.max_eviction_level = PressureLevel::Warning;
            }

            // Validate our watermarks and debounce settings makes sense.
            let watermarks = watchdog.mem_watermarks;
            let debounce = watchdog.watermark_debounce;
            for j in 0..NUM_WATERMARKS {
                let prev = if j == 0 { 0 } else { watermarks[j - 1] };
                let next = if j == NUM_WATERMARKS - 1 { u64::MAX } else { watermarks[j + 1] };
                let imminent = j == PressureLevel::ImminentOutOfMemory as usize;
                let oom = j == PressureLevel::OutOfMemory as usize;
                // The watermarks should be in increasing order, with a minimum of
                // watermark_debounce difference between consecutive levels. The only exception is
                // if the ImminentOOM level is disabled (by setting oom_imminent_oom_delta_mb to 0),
                // in which case the OOM and ImminentOOM watermarks will be the same, and the
                // ImminentOOM level will never be entered; we will either be in OOM or Critical.
                assert!(watermarks[j] > prev || (imminent && watermarks[j] == prev));
                assert!(watermarks[j] < next || (oom && watermarks[j] == next));
                assert!(watermarks[j] - prev > debounce || (imminent && watermarks[j] == prev));
                assert!(next - watermarks[j] > debounce || (oom && watermarks[j] == next));
            }

            // Set our eviction target to be such that we try to get completely out of the max
            // eviction level, taking into account the debounce.
            watchdog.free_mem_target =
                watermarks[watchdog.max_eviction_level as usize] + watchdog.watermark_debounce;

            watchdog.hysteresis = options.oom_hysteresis_seconds as i64 * NSEC_PER_SEC;
            watchdog.eviction_delay = options.oom_eviction_delay_ms as i64 * NSEC_PER_MSEC;

            eprintln!(
                "memory-pressure: memory watermarks - OutOfMemory: {}MB, Critical: {}MB, \
                 Warning: {}MB, Debounce: {}MB",
                watermarks[PressureLevel::OutOfMemory as usize] / MB,
                watermarks[PressureLevel::Critical as usize] / MB,
                watermarks[PressureLevel::Warning as usize] / MB,
                watchdog.watermark_debounce / MB
            );

            eprintln!(
                "memory-pressure: hysteresis interval - {} seconds",
                watchdog.hysteresis / NSEC_PER_SEC
            );

            if options.oom_evict_continuous {
                watchdog.eviction_strategy = EvictionStrategy::Continuous;
                pmm.page_queues().set_aging_event(Some(watchdog.mem_state_signal.clone()));
            } else {
                watchdog.eviction_strategy = EvictionStrategy::OneShot;
            }

            eprintln!(
                "memory-pressure: eviction: level - {}, strategy - {}, delay - {} ms",
                watchdog.max_eviction_level.to_str(),
                if options.oom_evict_continuous { "continuous" } else { "one-shot" },
                watchdog.eviction_delay / NSEC_PER_MSEC
            );

            if watchdog.is_imminent_oom_enabled() {
                eprintln!(
                    "memory-pressure: ImminentOutOfMemory watermark - {}MB",
                    watermarks[PressureLevel::ImminentOutOfMemory as usize] / MB
                );
            }
        }
        watchdog
    }

    /// Returns the event signaled while memory pressure is at the level `kind` describes.
    pub fn get_mem_pressure_event(&self, kind: u32) -> Option<Arc<EventDispatcher>> {
        let level = match kind {
            ZX_SYSTEM_EVENT_OUT_OF_MEMORY => PressureLevel::OutOfMemory,
            ZX_SYSTEM_EVENT_IMMINENT_OUT_OF_MEMORY => PressureLevel::ImminentOutOfMemory,
            ZX_SYSTEM_EVENT_MEMORY_PRESSURE_CRITICAL => PressureLevel::Critical,
            ZX_SYSTEM_EVENT_MEMORY_PRESSURE_WARNING => PressureLevel::Warning,
            ZX_SYSTEM_EVENT_MEMORY_PRESSURE_NORMAL => PressureLevel::Normal,
            _ => return None,
        };
        Some(self.mem_pressure_events[level as usize].clone())
    }

    fn pressure_event(&self, level: PressureLevel) -> &dyn Dispatcher {
        &*self.mem_pressure_events[level as usize]
    }

    fn mem_event_idx(&self) -> PressureLevel {
        PressureLevel::from_index(self.mem_event_idx.load(Ordering::Relaxed))
    }

    fn set_mem_event_idx(&self, level: PressureLevel) {
        self.mem_event_idx.store(level as u8, Ordering::Relaxed);
    }

    fn is_imminent_oom_enabled(&self) -> bool {
        self.mem_watermarks[PressureLevel::ImminentOutOfMemory as usize]
            != self.mem_watermarks[PressureLevel::OutOfMemory as usize]
    }

    fn eviction_trigger(&'static self) {
        // This runs from a timer, as such we do not want to be performing synchronous eviction and
        // blocking some random thread. Therefore we use the asynchronous eviction trigger that will
        // cause the eviction thread to perform the actual eviction work.
        if self.eviction_strategy == EvictionStrategy::Continuous {
            // Under a continuous eviction strategy we must set continuous_eviction_active to true
            // so that the worker thread knows it should bump the evictor when memory states change.
            self.continuous_eviction_active.store(true, Ordering::Release);
        }
        pmm::node().evictor().evict_asynchronous(
            self.min_free_target.load(Ordering::Relaxed),
            self.free_mem_target,
            EvictionLevel::OnlyOldest,
            Output::Print,
        );
    }

    /// Helper called by the memory pressure thread when OOM state is entered.
    fn on_oom(&self) {
        match BootOptions::get().oom_behavior {
            OomBehavior::JobKill => {
                eprintln!("memory-pressure: no alive job has a kill bit");

                // Since killing is asynchronous, sleep for a short period for the system to
                // quiesce. And if we don't find a killable job, don't just spin since the next
                // iteration probably won't find a one either.
                let _ = Event::new(false).wait(&Deadline::after_mono(500 * NSEC_PER_MSEC));
            }
            OomBehavior::Reboot => handle_on_oom_reboot(),
        }
    }

    /// Called by the worker thread to determine if a kernel event needs to be signaled
    /// corresponding to pressure change to level `idx`.
    fn is_signal_due(
        &self,
        worker: &WorkerState,
        idx: PressureLevel,
        time_now: zx_instant_mono_t,
    ) -> bool {
        // We signal a memory state change immediately if any of these conditions are met:
        // 1) The current index is lower than the previous one signaled (i.e. available memory is
        // lower now), so that clients can act on the signal quickly.
        // 2) `hysteresis` has elapsed since the last time we examined the state.
        idx < worker.prev_mem_event_idx
            || time_now.saturating_sub(worker.prev_mem_state_eval_time) >= self.hysteresis
    }

    /// Called by the worker thread to determine if kernel eviction (asynchronous) needs to be
    /// triggered in response to pressure change to level `idx`.
    fn is_eviction_required(&self, worker: &WorkerState, idx: PressureLevel) -> bool {
        // Trigger asynchronous eviction if:
        // 1) the memory availability state is more critical than the previous one
        // AND
        // 2) we're configured to evict at that level.
        //
        // Do not trigger asynchronous eviction at the OOM level, as we have already performed
        // synchronous eviction to attempt a quick recovery before reaching here. At this point we
        // are about to signal filesystems to shut down on OOM, after which eviction will be a
        // no-op anyway, since there will no longer be any pager-backed memory to evict.
        idx < worker.prev_mem_event_idx
            && idx <= self.max_eviction_level
            && idx != PressureLevel::OutOfMemory
    }

    fn worker_thread(&'static self) -> ! {
        let options = BootOptions::get();
        let node = pmm::node();
        let mut worker = WorkerState {
            prev_mem_event_idx: self.mem_event_idx(),
            prev_mem_state_eval_time: zx_types::ZX_TIME_INFINITE_PAST,
        };
        // Tracks whether we have logged entering the OOM state. Used to rate limit logs when
        // bouncing in and out of OOM rapidly. Hysteresis delays confirming the transition to a
        // lower pressure state, so we can re-enter OOM multiple times before that happens.
        let mut oom_entry_logged = false;
        // Accumulates the total number of pages evicted during a contiguous OOM period (which may
        // involve multiple bounces in and out of OOM level).
        let mut total_oom_evicted_pages = 0;

        loop {
            // If we've hit OOM level perform some immediate synchronous eviction to attempt to
            // avoid OOM.
            if self.mem_event_idx() == PressureLevel::OutOfMemory {
                // Log only the first time we enter OOM in this period to avoid spam.
                if !oom_entry_logged {
                    eprintln!(
                        "memory-pressure: beginning reclamation to avoid OOM. Allocations are \
                         now disabled"
                    );
                    oom_entry_logged = true;
                }

                // Keep trying to perform eviction for as long as we are evicting non-zero pages
                // and we remain in the out of memory state.
                while self.mem_event_idx() == PressureLevel::OutOfMemory {
                    let evicted_pages = node
                        .evictor()
                        .evict_synchronous(
                            MB * options.oom_eviction_delta_at_oom_mb,
                            0,
                            EvictionLevel::IncludeNewest,
                            Output::NoPrint,
                            TriggerReason::Oom,
                        )
                        .counts
                        .non_loaned_total();
                    if evicted_pages == 0 {
                        eprintln!("memory-pressure: found no pages to evict");
                        break;
                    }
                    // Accumulate total pages evicted across all bounces in this OOM period.
                    total_oom_evicted_pages += evicted_pages;
                    self.set_mem_event_idx(self.calculate_pressure_level());
                }
            }

            // Check to see if the PMM has failed any allocations. If the PMM has ever failed to
            // allocate because it was out of memory, then escalate the pressure level to trigger
            // an OOM response immediately. The idea here is that usermode processes may not be
            // able to handle allocation failure and therefore could have become wedged in some
            // way.
            if options.oom_trigger_on_alloc_failure && node.has_alloc_failed_no_mem() {
                let first_failure = node.get_first_alloc_failure();
                eprintln!(
                    "memory-pressure: failed one or more allocations (first reported type: {:?}, \
                     size: {}, free memory: {}MB), escalating to oom...",
                    first_failure.r#type,
                    first_failure.size,
                    first_failure.free_count * PAGE_SIZE / MB
                );
                self.set_mem_event_idx(PressureLevel::OutOfMemory);
            }

            let time_now = current_mono_time();
            let mem_event_idx = self.mem_event_idx();

            if self.is_signal_due(&worker, mem_event_idx, time_now) {
                // Log the total pages reclaimed during this OOM period (if any) and reset the OOM
                // logging flag.
                if oom_entry_logged {
                    if total_oom_evicted_pages > 0 {
                        eprintln!(
                            "memory-pressure: reclaimed {}MB to avoid OOM",
                            total_oom_evicted_pages * PAGE_SIZE / MB
                        );
                        total_oom_evicted_pages = 0;
                    }
                    oom_entry_logged = false;
                }

                eprintln!(
                    "memory-pressure: memory availability state - {}",
                    mem_event_idx.to_str()
                );

                if self.is_eviction_required(&worker, mem_event_idx) {
                    // Clear any previous eviction trigger. Once cancel completes we know that we
                    // will not race with the callback and are free to update the targets. Cancel
                    // will return true if the timer was canceled before it called back, i.e. an
                    // eviction was outstanding.
                    let eviction_was_outstanding = self.eviction_trigger.cancel();

                    let min_free_target = if options.oom_evict_with_min_target {
                        let free_mem = pmm::count_free_pages() * PAGE_SIZE;
                        // Set the minimum amount to free as half the amount required to reach our
                        // desired free memory level. This minimum ensures that even if the user
                        // reduces memory in reaction to this signal we will always attempt to free
                        // a bit.
                        if free_mem < self.free_mem_target {
                            (self.free_mem_target - free_mem) / 2
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    self.min_free_target.store(min_free_target, Ordering::Relaxed);

                    // If eviction was outstanding when we canceled the eviction trigger, trigger
                    // eviction immediately without any delay. We are here because of a rapid
                    // allocation spike which caused the memory pressure to become more critical
                    // in a very short interval, so it might be better to evict pages as soon as
                    // possible to try and counter the allocation spike. Otherwise if eviction was
                    // not outstanding, trigger the eviction for slightly in the future. Half the
                    // hysteresis time here is a balance between giving user space time to release
                    // memory and the eviction running before the end of the hysteresis period.
                    if eviction_was_outstanding || self.eviction_delay == 0 {
                        self.eviction_trigger();
                    } else {
                        self.eviction_trigger.set_oneshot(
                            zx_time_add_duration(time_now, self.eviction_delay),
                            move || self.eviction_trigger(),
                        );
                    }
                } else if self.eviction_strategy == EvictionStrategy::Continuous
                    && mem_event_idx > self.max_eviction_level
                {
                    // If we're out of the max configured eviction-eligible memory pressure level,
                    // disable continuous eviction.

                    // Cancel any outstanding eviction trigger, so that eviction is not
                    // accidentally enabled *after* we disable it here.
                    self.eviction_trigger.cancel();
                    self.continuous_eviction_active.store(false, Ordering::Release);
                }

                // Unsignal the last event that was signaled.
                let prev = worker.prev_mem_event_idx;
                if let Err(status) =
                    self.pressure_event(prev).user_signal_self(ZX_EVENT_SIGNALED, 0)
                {
                    panic!(
                        "memory-pressure: unsignal memory event {} failed: {status}",
                        prev.to_str()
                    );
                }

                // Signal event corresponding to the new memory state.
                if let Err(status) =
                    self.pressure_event(mem_event_idx).user_signal_self(0, ZX_EVENT_SIGNALED)
                {
                    panic!(
                        "memory-pressure: signal memory event {} failed: {status}",
                        mem_event_idx.to_str()
                    );
                }
                worker.prev_mem_event_idx = mem_event_idx;
                worker.prev_mem_state_eval_time = time_now;

                // If we're below the out-of-memory watermark, trigger OOM behavior.
                if mem_event_idx == PressureLevel::OutOfMemory {
                    self.on_oom();
                }

                // Wait for the memory state to change again.
                self.wait_for_mem_change(&Deadline::infinite());
            } else {
                worker.prev_mem_state_eval_time = time_now;

                // We are ignoring this memory state transition. Wait for only `hysteresis`, and
                // then re-evaluate the memory state. Otherwise we could remain stuck at the lower
                // memory state if mem_state_signal is not signaled.
                self.wait_for_mem_change(&Deadline::no_slack(zx_time_add_duration(
                    time_now,
                    self.hysteresis,
                )));
            }
        }
    }

    fn wait_for_mem_change(&self, deadline: &Deadline) {
        let options = BootOptions::get();
        let node = pmm::node();
        let prev = self.mem_event_idx();
        // Coming into this method we must not be in the OutOfMemory state, as if this were possible
        // allocations would be stalled and we would be waiting for a memory state change *before*
        // triggering the evictor, which would cause a deadlock.
        debug_assert_ne!(prev, PressureLevel::OutOfMemory);
        let mut status = Ok(());
        // Count how many times in a row the setting of the free memory signal fails. This should
        // only fail in the case of an unlikely race, and failing repeatedly could indicate a bug
        // and also means the free memory signal in the pmm is not getting signaled.
        let mut set_free_memory_failed_iterations: u32 = 0;
        // This loop can iterate many times as it is woken up by both pmm state changes and, if
        // continuous eviction is enabled, page queues state changes.
        loop {
            // We cannot enter this method in the OutOfMemory state and since we would exit this
            // loop if the state were to change we can never be configuring the free memory signal
            // in this state.
            debug_assert_ne!(self.mem_event_idx(), PressureLevel::OutOfMemory);
            let (lower, upper) = self.free_mem_bounds_for_level(self.mem_event_idx());
            let delay_alloc_level = (self.mem_watermarks[PressureLevel::OutOfMemory as usize]
                - self.watermark_debounce)
                / PAGE_SIZE;
            if node.set_free_memory_signal(
                lower / PAGE_SIZE,
                upper / PAGE_SIZE,
                delay_alloc_level,
                self.mem_state_signal.clone(),
            ) {
                // After having successfully set the event check again for any allocation failures.
                // This is to ensure that if an allocation failure happened while we did not have an
                // event set that it is not missed.
                set_free_memory_failed_iterations = 0;
                if options.oom_trigger_on_alloc_failure && node.has_alloc_failed_no_mem() {
                    return;
                }
                status = self.mem_state_signal.wait(deadline);
            } else {
                set_free_memory_failed_iterations += 1;
                // Setting failed, must've raced. Fall through and compute the new pressure level.
                if set_free_memory_failed_iterations > 5
                    && set_free_memory_failed_iterations.is_power_of_two()
                {
                    eprintln!(
                        "memory-pressure: WARNING pmm_set_free_memory_signal has failed {} times \
                         in a row",
                        set_free_memory_failed_iterations
                    );
                }
            }
            self.set_mem_event_idx(self.calculate_pressure_level());
            // If continuous eviction is currently active then let the evictor know that it may have
            // some work to do. This is done by requesting an asynchronous eviction to the free
            // memory target. As we are running in the context of the worker thread, there is no
            // race where active could transition from true->false, so we will not trigger
            // eviction unnecessarily. Should there be a false->true race (due to the
            // eviction_trigger callback running) then this is fine, since that callback will set
            // the eviction target.
            if self.continuous_eviction_active.load(Ordering::Acquire) {
                node.evictor().evict_asynchronous(
                    0,
                    self.free_mem_target,
                    EvictionLevel::OnlyOldest,
                    Output::NoPrint,
                );
            }
            // In the case where we raced with additional pmm actions keep looping unless the
            // deadline was reached.
            if !(self.mem_event_idx() == prev && status == Ok(())) {
                break;
            }
        }
    }

    fn free_mem_bounds_for_level(&self, level: PressureLevel) -> (u64, u64) {
        // If ImminentOOM is disabled, we will never enter that level, and so we should never be
        // asked to compute its bounds.
        debug_assert!(
            self.is_imminent_oom_enabled() || level != PressureLevel::ImminentOutOfMemory
        );
        // Calculate the range, including debounce, for the current memory level.
        let mut lower = 0;
        let mut upper = u64::MAX;
        if level > PressureLevel::OutOfMemory {
            lower = self.mem_watermarks[level as usize - 1] - self.watermark_debounce;
        }
        if (level as usize) < NUM_WATERMARKS {
            upper = self.mem_watermarks[level as usize] + self.watermark_debounce;
        }
        (lower, upper)
    }

    fn calculate_pressure_level(&self) -> PressureLevel {
        let current = self.mem_event_idx();
        // Get the bounds for the current level, this is inclusive of the debounce.
        let (lower, upper) = self.free_mem_bounds_for_level(current);

        // Retrieve current free memory.
        let free_mem = pmm::count_free_pages() * PAGE_SIZE;

        // Check if still inside the bounds.
        if free_mem >= lower && free_mem <= upper {
            // No change.
            return current;
        }

        // Determine the new level using a simple O(N).
        let mut new_level = PressureLevel::OutOfMemory as usize;
        while new_level < NUM_WATERMARKS && free_mem > self.mem_watermarks[new_level] {
            new_level += 1;
        }
        let new_level = PressureLevel::from_index(new_level as u8);
        debug_assert_ne!(new_level, current);
        // If ImminentOOM is disabled, we should never compute it as the current level.
        debug_assert!(
            self.is_imminent_oom_enabled() || new_level != PressureLevel::ImminentOutOfMemory
        );
        new_level
    }

    pub fn dump(&self) {
        let watermarks = (0..NUM_WATERMARKS)
            .map(|i| {
                let level = PressureLevel::from_index(i as u8);
                format!("{}: {}MB", level.to_str(), self.mem_watermarks[i] / MB)
            })
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("watermarks: [{watermarks}]");
        let current = self.mem_event_idx();
        let (lower, upper) = self.free_mem_bounds_for_level(current);
        eprintln!("debounce: {}MB", self.watermark_debounce / MB);
        eprintln!("current state: {} [{}]", current as u8, current.to_str());
        eprintln!("current bounds: [{lower}, {upper}]");
        eprintln!("free memory: {}MB", pmm::count_free_pages() * PAGE_SIZE / MB);
    }
}
