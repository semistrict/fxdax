// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Ported from `zircon/kernel/vm/unittests/test_helper.cc` and
//! `zircon/kernel/vm/unittests/test_helper.h`.

use crate::kernel::event::Event;
use crate::vm::page::VmPagePtr;
use crate::vm::page_source::{
    MultiPageRequest, PageProvider, PageRequestRef, PageRequestType, PageSource,
    PageSourceProperties,
};
use crate::vm::scanner::AutoVmScannerDisable;
use crate::vm::vm_cow_pages::SupplyOptions;
use crate::vm::vm_object_paged::VmObjectPaged;
use crate::vm::vm_page_list::VmPageSpliceList;
use std::sync::Arc;
use zx_status::Status;

const PAGE_SIZE: u64 = page::SIZE as u64;

/// A page provider that is not expected to receive requests, for VMOs whose pages are all
/// supplied up front.
pub struct StubPageProvider {
    trap_dirty: bool,
    ignore_requests: bool,
}

impl StubPageProvider {
    pub fn new(trap_dirty: bool, ignore_requests: bool) -> Self {
        Self { trap_dirty, ignore_requests }
    }
}

impl PageProvider for StubPageProvider {
    fn properties(&self) -> PageSourceProperties {
        let mut supports_request_type = [false; PageRequestType::COUNT];
        supports_request_type[PageRequestType::Read as usize] = true;
        supports_request_type[PageRequestType::Dirty as usize] = self.trap_dirty;
        PageSourceProperties { is_user_pager: true, supports_request_type }
    }

    fn send_async_request(&self, _request: PageRequestRef) {
        assert!(self.ignore_requests);
    }

    fn clear_async_request(&self, _request: PageRequestRef) {
        assert!(self.ignore_requests);
    }

    fn swap_async_request(&self, _old: PageRequestRef, _new_req: PageRequestRef) {
        assert!(self.ignore_requests);
    }

    fn debug_is_page_ok(&self, _page: VmPagePtr, _offset: u64) -> bool {
        true
    }

    fn on_detach(&self) {}

    fn on_close(&self) {}

    fn wait_on_event(&self, _event: &Event, _suspendable: bool) -> Result<(), Status> {
        panic!("Not implemented");
    }

    fn dump(&self, _depth: u32, _max_items: u32) {}
}

/// Create a pager-backed VMO with size equals `num_pages` pages, and commit `committed_pages` of
/// its pages. `trap_dirty` controls whether modifications to pages must be trapped in order to
/// generate DIRTY page requests. `resizable` controls whether the created VMO is resizable.
/// Returns the pages committed, so that tests can examine their state. Allows tests to work with
/// pager-backed VMOs without blocking on page faults. The `ignore_requests` flag can be set if
/// attempts at sending page faults should be ignored, or result in a panic.
pub fn make_partially_committed_pager_vmo(
    num_pages: usize,
    committed_pages: usize,
    trap_dirty: bool,
    resizable: bool,
    ignore_requests: bool,
) -> Result<(Arc<VmObjectPaged>, Vec<VmPagePtr>), Status> {
    // Disable the scanner so we can safely submit our aux vmo and query pages without eviction
    // happening.
    let _scanner_disable = AutoVmScannerDisable::new();

    // Create a pager backed VMO and jump through some hoops to pre-fill pages for it so we do not
    // actually take any page faults.
    let pager = Arc::new(StubPageProvider::new(trap_dirty, ignore_requests));

    let src = PageSource::new(pager);

    let options = if resizable { VmObjectPaged::RESIZABLE } else { 0 };
    let vmo = VmObjectPaged::create_external(src, options, num_pages as u64 * PAGE_SIZE)?;

    let mut pages = Vec::new();
    if committed_pages > 0 {
        let len = committed_pages as u64 * PAGE_SIZE;
        let aux_vmo = VmObjectPaged::create(0, 0, len)?;

        aux_vmo.commit_range(0, len)?;

        let mut splice_list = VmPageSpliceList::new();
        aux_vmo.take_pages(0, len, &mut splice_list)?;

        vmo.supply_pages(0, len, &mut splice_list, SupplyOptions::PagerSupply)?;

        for i in 0..committed_pages as u64 {
            let mut page_request = MultiPageRequest::default();
            pages.push(vmo.get_page(i * PAGE_SIZE, 0, &mut page_request)?);
        }
    }

    Ok((vmo, pages))
}

/// Convenience wrapper for `make_partially_committed_pager_vmo` that commits all pages.
pub fn make_committed_pager_vmo(
    num_pages: usize,
    trap_dirty: bool,
    resizable: bool,
) -> Result<(Arc<VmObjectPaged>, Vec<VmPagePtr>), Status> {
    make_partially_committed_pager_vmo(num_pages, num_pages, trap_dirty, resizable, false)
}
