// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Pager-backed VMOs for the benchmarks, their pages supplied up front, as a pager supplies a file's
//! once it has read them: clean, and evictable. Their pager takes no request, as nothing faults on
//! them.

use std::sync::Arc;
use zircon::kernel::event::Event;
use zircon::vm::page::VmPagePtr;
use zircon::vm::page_source::{
    EventWaitFuture, PageProvider, PageRequestRef, PageRequestType, PageSource,
    PageSourceProperties,
};
use zircon::vm::vm_cow_pages::SupplyOptions;
use zircon::vm::vm_object_paged::VmObjectPaged;
use zircon::vm::vm_page_list::VmPageSpliceList;
use zx_status::Status;

/// A user pager that takes no request: the pages of its VMOs are all supplied up front.
struct SuppliedUpFront;

impl PageProvider for SuppliedUpFront {
    fn properties(&self) -> PageSourceProperties {
        let mut supports_request_type = [false; PageRequestType::COUNT];
        supports_request_type[PageRequestType::Read as usize] = true;
        PageSourceProperties { is_user_pager: true, supports_request_type }
    }

    fn send_async_request(&self, _request: PageRequestRef) {
        panic!("a page was requested of a VMO whose pages are all supplied");
    }

    fn clear_async_request(&self, _request: PageRequestRef) {
        panic!("a page was requested of a VMO whose pages are all supplied");
    }

    fn swap_async_request(&self, _old: PageRequestRef, _new_req: PageRequestRef) {
        panic!("a page was requested of a VMO whose pages are all supplied");
    }

    fn debug_is_page_ok(&self, _page: VmPagePtr, _offset: u64) -> bool {
        true
    }

    fn on_detach(&self) {}

    fn on_close(&self) {}

    fn wait_on_event(&self, _event: &Event, _suspendable: bool) -> Result<(), Status> {
        panic!("a page was requested of a VMO whose pages are all supplied");
    }

    fn wait_on_event_async<'a>(
        &'a self,
        _event: &'a Event,
        _suspendable: bool,
    ) -> EventWaitFuture<'a> {
        panic!("a page was requested of a VMO whose pages are all supplied");
    }

    fn dump(&self, _depth: u32, _max_items: u32) {}
}

/// A pager-backed VMO of `len` bytes, its pages not yet supplied.
pub fn vmo(len: u64) -> Arc<VmObjectPaged> {
    let source = PageSource::new(Arc::new(SuppliedUpFront));
    VmObjectPaged::create_external(source, 0, len).unwrap()
}

/// An anonymous VMO of `len` bytes, of the current arena, its pages committed: pages for [`supply`]
/// to supply, as a pager writes the pages it supplies first.
pub fn committed(len: u64) -> Arc<VmObjectPaged> {
    let vmo = VmObjectPaged::create(0, 0, len).unwrap();
    vmo.commit_range(0, len).unwrap();
    vmo
}

/// Supplies `vmo`, made by [`vmo`], at `offset` with the pages of `from`, made by [`committed`],
/// as its pager would: taken from `from`.
pub fn supply(vmo: &VmObjectPaged, offset: u64, from: &VmObjectPaged) {
    let len = from.size();
    let mut pages = VmPageSpliceList::new();
    from.take_pages(0, len, &mut pages).unwrap();
    vmo.supply_pages(offset, len, &mut pages, SupplyOptions::PagerSupply).unwrap();
}
