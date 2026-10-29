// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! fxdax's own: the quarantine of freed pages a hypervisor's second stage may map still
//! (TASK-79.9). A window's second stage, as the extent daemon's connection to a VM process is,
//! makes the window's changes as they are sent, after the sender made them, and they are sent with
//! the sender's locks held, so it waits for none: a page whose mapping it took away is reached
//! through the stage until the stage has made the change. So a page a second stage mapped since
//! it was allocated ([`Page::mark_staged`]) is not freed as its holder drops it, but quarantined:
//! handed to what the PMM's user gives ([`quarantine_with`]), which frees it once every second
//! stage has made the changes it was sent so far, by dropping it. Without one, it is freed at once,
//! as a second stage of this process makes each change before it returns.
//!
//! [`Page::mark_staged`]: crate::Page::mark_staged

use super::node;
use crate::frames::Frames;
use crate::sync::rules;
#[cfg(all(test, feature = "shuttle"))]
use std::sync::Arc;
use std::sync::OnceLock;

/// Pages freed that a second stage mapped since they were allocated, which go to the PMM once
/// dropped.
pub struct Quarantined {
    frames: Vec<u32>,
}

impl Quarantined {
    /// fxdax: `frames`' pages, freed, quarantined, as their arenas count them until they come back
    /// (TASK-106).
    fn new(frames: Vec<u32>) -> Self {
        node().quarantining(&frames);
        Self { frames }
    }

    /// How many pages it holds.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

impl std::fmt::Debug for Quarantined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Quarantined").field("frames", &self.frames).finish()
    }
}

impl Drop for Quarantined {
    fn drop(&mut self) {
        // The pages are the quarantine's, which their holder gave up as it freed them, and which
        // their arenas counted as they were quarantined.
        node().free_quarantined(&self.frames);
    }
}

/// What quarantines the pages freed, if the PMM's user gave it.
static QUARANTINE: OnceLock<Box<dyn Fn(Quarantined) + Send + Sync>> = OnceLock::new();

/// Has `quarantine` take the pages freed that a second stage mapped since they were allocated,
/// from now on: it frees them, dropping them, once every second stage has made the changes it was
/// sent so far, which take them away. It is called without the PMM's lock, but with whatever locks
/// the page's holder drops it under: it may free them at once, dropping them, as their holder
/// would have. Given once, for the process's life; Shuttle tests give theirs with
/// `quarantine_for_test`.
pub fn quarantine_with(quarantine: impl Fn(Quarantined) + Send + Sync + 'static) {
    let given = QUARANTINE.set(Box::new(quarantine));
    assert!(given.is_ok(), "the quarantine is given once");
}

/// The quarantine of the test that holds one ([`quarantine_for_test`]).
#[cfg(all(test, feature = "shuttle"))]
type TestQuarantine = Arc<dyn Fn(Quarantined) + Send + Sync>;

/// The quarantine of the test holding one, if any ([`quarantine_for_test`]).
#[cfg(all(test, feature = "shuttle"))]
static TEST_QUARANTINE: std::sync::Mutex<Option<TestQuarantine>> = std::sync::Mutex::new(None);

/// Has `quarantine` take the pages freed that a second stage mapped, as [`quarantine_with`] would,
/// for as long as the test holds what it returns: the process's quarantine, given once for its
/// tests, hands them to the quarantine of the test holding one, and, with none, frees them at once,
/// as with no quarantine. Tests holding one run one at a time, so that tests run in one process,
/// as `cargo test` runs them, quarantine nothing of another's. For the Shuttle tests, whose runs
/// share the process's quarantine.
#[cfg(all(test, feature = "shuttle"))]
pub(crate) fn quarantine_for_test(
    quarantine: impl Fn(Quarantined) + Send + Sync + 'static,
) -> QuarantineForTest {
    use std::sync::PoisonError;
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static GIVEN: std::sync::Once = std::sync::Once::new();
    let turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    GIVEN.call_once(|| {
        quarantine_with(|pages| {
            // Called without the lock, which a quarantine's own locks may follow.
            let test = TEST_QUARANTINE.lock().unwrap_or_else(PoisonError::into_inner).clone();
            match test {
                Some(quarantine) => quarantine(pages),
                None => drop(pages),
            }
        })
    });
    *TEST_QUARANTINE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(quarantine));
    QuarantineForTest { _turn: turn }
}

/// A test's quarantine ([`quarantine_for_test`]), which it holds until dropped.
#[cfg(all(test, feature = "shuttle"))]
pub(crate) struct QuarantineForTest {
    _turn: std::sync::MutexGuard<'static, ()>,
}

#[cfg(all(test, feature = "shuttle"))]
impl Drop for QuarantineForTest {
    fn drop(&mut self) {
        let test = TEST_QUARANTINE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        drop(test);
    }
}

/// Quarantines `frame`'s page, freed, if a second stage mapped it since it was allocated and
/// [`quarantine_with`] gave a quarantine: whether it did, leaving the PMM nothing to free.
pub(super) fn sift(frames: &Frames, frame: u32) -> bool {
    let Some(quarantine) = QUARANTINE.get() else { return false };
    if !frames.take_staged(frame) || !rules().staged_pages_wait_for_their_stages {
        return false;
    }
    quarantine(Quarantined::new(vec![frame]));
    true
}
