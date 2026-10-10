//! Waiting for the PMM to allow an allocation.
//!
//! Ported from `zircon/kernel/vm/anonymous_page_request.cc` and
//! `zircon/kernel/vm/include/vm/anonymous_page_request.h`.
//!
//! When free memory is low the PMM tells allocations that can wait to do so
//! (`ZX_ERR_SHOULD_WAIT`). The caller drops its locks and waits here, until
//! reclamation has freed enough memory for the allocation to proceed.
//!
//! Deviations:
//! - There is no scanner state dump before the panic.
//! - A future may wait in place of a thread (`allocate_async`; see `kernel::wakers`).

use crate::kernel::deadline::Deadline;
use crate::vm::page::VmPagePtr;
use crate::vm::pmm;
use zx_status::Status;
use zx_types::zx_duration_mono_t;

#[derive(Default)]
pub struct AnonymousPageRequest {
    active: bool,
    page: Option<VmPagePtr>,
}

impl AnonymousPageRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn make_active(&mut self) {
        debug_assert!(!self.has_page());
        self.active = true;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn cancel(&mut self) {
        self.active = false;
    }

    pub fn take_page(&mut self) -> Option<VmPagePtr> {
        self.page.take()
    }

    pub fn has_page(&self) -> bool {
        self.page.is_some()
    }

    /// Waits until the PMM can allocate a page and allocates it. Must be called without locks
    /// held.
    pub fn allocate(&mut self) -> Result<(), Status> {
        debug_assert!(self.active);
        debug_assert!(!self.has_page());
        let mut waited = 0;
        let result = loop {
            let result = pmm::node().wait_for_single_page_allocation(Self::report_deadline());
            if let Some(result) = Self::retry_unless_done(result, &mut waited) {
                break result;
            }
        };
        self.allocated(result)
    }

    /// Waits for the PMM to allocate a page, as `allocate` does, as a future rather than by
    /// blocking the calling thread.
    pub async fn allocate_async(&mut self) -> Result<(), Status> {
        debug_assert!(self.active);
        debug_assert!(!self.has_page());
        let mut waited = 0;
        let result = loop {
            let result =
                pmm::node().wait_for_single_page_allocation_async(Self::report_deadline()).await;
            if let Some(result) = Self::retry_unless_done(result, &mut waited) {
                break result;
            }
        };
        self.allocated(result)
    }

    /// This should only ever end up waiting momentarily until reclamation catches up. As such if
    /// we end up waiting for a long time then this is probably a sign of a bug in reclamation
    /// somewhere, so we want to make some noise here.
    const REPORT_WAIT_TIME: zx_duration_mono_t = 5_000_000_000;

    fn report_deadline() -> Deadline {
        Deadline::after_mono(Self::REPORT_WAIT_TIME)
    }

    /// The result of waiting for the PMM to allocate a page, `result`, unless the allocation should
    /// be retried. `waited` counts the waits that timed out.
    fn retry_unless_done(
        result: Result<VmPagePtr, Status>,
        waited: &mut u32,
    ) -> Option<Result<VmPagePtr, Status>> {
        const MAX_WAITS: u32 = (30_000_000_000 / AnonymousPageRequest::REPORT_WAIT_TIME) as u32;
        match result {
            // System is not making forward progress, lets try again.
            Err(Status::TIMED_OUT) => {
                *waited += 1;
                let seconds = Self::REPORT_WAIT_TIME * i64::from(*waited) / 1_000_000_000;
                eprintln!("WARNING: Waited {seconds} seconds to retry PMM allocations");
                if *waited == MAX_WAITS {
                    // If we've been waiting for a while without being able to get more memory and
                    // without declaring OOM, the memory watchdog is probably wedged and the system
                    // is in an unrecoverable state.
                    panic!("Allocation stalled for {seconds} seconds");
                }
                None
            }
            // The system reached an allocation allowing event, but we lost the race for the
            // memory resources. We reset the retry count, since the system IS making forward
            // progress.
            Err(Status::SHOULD_WAIT) => {
                *waited = 0;
                None
            }
            result => Some(result),
        }
    }

    /// Takes the page the wait allocated, if it did. Once we return, the request is no longer
    /// active.
    fn allocated(&mut self, result: Result<VmPagePtr, Status>) -> Result<(), Status> {
        self.active = false;
        self.page = Some(result?);
        Ok(())
    }
}

impl Drop for AnonymousPageRequest {
    fn drop(&mut self) {
        if let Some(page) = self.page.take() {
            // SAFETY: The request owned the page.
            unsafe { pmm::free_page(page) };
        }
    }
}
