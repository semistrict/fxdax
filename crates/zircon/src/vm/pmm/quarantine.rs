// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! fxdax's own: the quarantine of freed pages a hypervisor's second stage may map still
//! (TASK-79.9). A window's second stage, as the extent daemon's connection to a VM process is,
//! makes the window's changes as the kernel sends them, after the kernel made them, and the kernel
//! sends them with its locks held, so it waits for none: a page whose mapping it took away is
//! reached through the stage until the stage has made the change. So a page a second stage mapped
//! since it was allocated (`VmPage::stage`) is not freed as the kernel frees it, but quarantined:
//! handed to what the kernel's user gives ([`quarantine_with`]), which frees it once every second
//! stage has made the changes it was sent so far, by dropping it. Without one, it is freed at once,
//! as a second stage of this process makes each change before it returns.

use super::{PmmOptDelayReuse, node};
use crate::sync::rules;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use std::sync::OnceLock;

/// Pages freed that a second stage mapped since they were allocated, which go to the PMM once
/// dropped.
pub struct Quarantined {
    pages: VmPageDoublyLinkedList,
    delay_reuse: PmmOptDelayReuse,
}

impl Quarantined {
    /// How many pages it holds.
    pub fn len(&self) -> usize {
        self.pages.iter().count()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
}

impl Drop for Quarantined {
    fn drop(&mut self) {
        // SAFETY: The pages are the quarantine's, which their owner gave up as it freed them.
        unsafe { node().free_list(&mut self.pages, self.delay_reuse) };
    }
}

/// What quarantines the pages freed, if the kernel's user gave it.
static QUARANTINE: OnceLock<Box<dyn Fn(Quarantined) + Send + Sync>> = OnceLock::new();

/// Has `quarantine` take the pages freed that a second stage mapped since they were allocated,
/// from now on: it frees them, dropping them, once every second stage has made the changes it was
/// sent so far, which take them away. It is called without the PMM's lock, but with whatever locks
/// the page's owner frees it under, and must take none of the kernel's. Called once.
pub fn quarantine_with(quarantine: impl Fn(Quarantined) + Send + Sync + 'static) {
    let given = QUARANTINE.set(Box::new(quarantine));
    assert!(given.is_ok(), "the quarantine is given once");
}

/// Quarantines the pages of `list` a second stage mapped since they were allocated, taking them
/// out of it, if [`quarantine_with`] gave a quarantine: those left the PMM frees now.
pub(super) fn sift(list: &mut VmPageDoublyLinkedList, delay_reuse: PmmOptDelayReuse) {
    let Some(quarantine) = QUARANTINE.get() else { return };
    let mut pages = VmPageDoublyLinkedList::new();
    let mut page = list.front();
    while let Some(staged) = page {
        page = list.next(staged);
        if staged.take_staged() && rules().staged_pages_wait_for_their_stages {
            list.erase(staged);
            pages.push_back(staged);
        }
    }
    if !pages.is_empty() {
        quarantine(Quarantined { pages, delay_reuse });
    }
}

/// Quarantines `page` if a second stage mapped it since it was allocated, as [`sift`] does a list:
/// whether it did, leaving the PMM nothing to free.
pub(super) fn sift_page(page: VmPagePtr, delay_reuse: PmmOptDelayReuse) -> bool {
    let Some(quarantine) = QUARANTINE.get() else { return false };
    if !page.take_staged() || !rules().staged_pages_wait_for_their_stages {
        return false;
    }
    let mut pages = VmPageDoublyLinkedList::new();
    pages.push_back(page);
    quarantine(Quarantined { pages, delay_reuse });
    true
}
