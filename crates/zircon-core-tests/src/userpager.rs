// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A userspace pager for tests, ported from `zircon/system/utest/core/pager/userpager.cc` and
//! `zircon/system/utest/core/pager/userpager.h`.
//!
//! Deviations:
//! - VMOs are shared as `Arc<Vmo>` rather than raw pointers, so test threads can hold them.
//! - No VMO children, so `Vmo::Clone` and `PollNumChildren` are not ported.
//! - No unbounded VMOs.

use crate::test_thread::TestThread;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use zx::sys::zx_page_request_command_t;

unsafe extern "C" {
    /// From `zircon/process.h`.
    fn zx_vmar_root_self() -> zx::sys::zx_handle_t;
}

/// The root VMAR of the process.
pub fn root_vmar() -> zx::Unowned<'static, zx::Vmar> {
    // SAFETY: The root VMAR handle lives as long as the process.
    unsafe { zx::Unowned::from_raw_handle(zx_vmar_root_self()) }
}

pub fn page_size() -> u64 {
    zx::system_get_page_size().into()
}

/// A range of dirty pages, as `zx_pager_query_dirty_ranges` returns them, from
/// `zircon/system/public/zircon/syscalls-next.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct zx_vmo_dirty_range_t {
    pub offset: u64,
    pub length: u64,
    pub options: u64,
}

/// From `zircon/system/public/zircon/types.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct zx_pager_vmo_stats_t {
    modified: u32,
}

const ZX_PAGER_VMO_STATS_MODIFIED: u32 = 1;
const ZX_PAGER_RESET_VMO_STATS: u32 = 1;

unsafe extern "C" {
    fn zx_pager_query_dirty_ranges(
        pager: zx::sys::zx_handle_t,
        pager_vmo: zx::sys::zx_handle_t,
        offset: u64,
        length: u64,
        buffer: *mut u8,
        buffer_size: usize,
        actual: *mut usize,
        avail: *mut usize,
    ) -> zx::sys::zx_status_t;

    fn zx_pager_query_vmo_stats(
        pager: zx::sys::zx_handle_t,
        pager_vmo: zx::sys::zx_handle_t,
        options: u32,
        buffer: *mut u8,
        buffer_size: usize,
    ) -> zx::sys::zx_status_t;
}

struct VmoState {
    // These are set in the ctor, but can be changed by Vmo::Resize.
    // The region described by this range remains mapped for the lifetime of the object, and we
    // are responsible for unmapping it on destruction.
    size: u64,
    base_addr: usize,
    page_fault_supply_limit: u64,
}

/// A pager-backed VMO, mapped for its life.
///
/// This class is thread-safe and operations may be called concurrently, from the main test thread
/// and from additional instances of TestThread that the main thread spawns.
pub struct Vmo {
    // Use this mutex to protect state as sparingly as possible; the primary objective of this lock
    // is to prevent data races.
    //  - Do not hold it on paths that might block on page requests, because the UserPager might
    //  need the lock to resolve the page requests too, and we will deadlock.
    //  - Do not hold it over long critical sections as it might defeat the intended concurrency of
    //  test threads by serializing on this mutex instead.
    mutex: Mutex<VmoState>,
    vmo: zx::Vmo,
    // This value is used for both the port packet key and to populate the contents of supplied
    // pages.
    key: u64,
}

impl Drop for Vmo {
    fn drop(&mut self) {
        let state = self.mutex.lock();
        if state.size != 0 {
            // SAFETY: Nothing refers to the mapping any more.
            let _ = unsafe { root_vmar().unmap(state.base_addr, state.size as usize) };
        }
    }
}

impl Vmo {
    pub fn create(vmo: zx::Vmo, size: u64, key: u64) -> Option<Arc<Vmo>> {
        assert_eq!(size % page_size(), 0);
        let mut addr = 0;
        if size != 0 {
            match root_vmar().map(
                0,
                &vmo,
                0,
                size as usize,
                zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
            ) {
                Ok(a) => addr = a,
                Err(status) => {
                    eprintln!("vmar map failed with {status}");
                    return None;
                }
            }
        }
        Some(Arc::new(Vmo {
            mutex: Mutex::new(VmoState {
                size,
                base_addr: addr,
                page_fault_supply_limit: u64::MAX,
            }),
            vmo,
            key,
        }))
    }

    pub fn size(&self) -> u64 {
        self.mutex.lock().size
    }

    pub fn base_addr(&self) -> usize {
        self.mutex.lock().base_addr
    }

    pub fn vmo(&self) -> &zx::Vmo {
        &self.vmo
    }

    pub fn key(&self) -> u64 {
        self.key
    }

    /// Validates this vmo's content in the specified pages using a mapped vmar.
    pub fn check_vmar(&self, offset: u64, len: u64, expected: Option<&[u8]>) -> bool {
        // The size can change once we're inside this function and blocked on a page request for
        // example, but the specified range should at least initially be within bounds.
        if offset + len > self.size() / page_size() {
            return false;
        }
        let len = len * page_size();
        let offset = offset * page_size();
        let base = self.base_addr() as *const u64;
        for i in offset / 8..(offset + len) / 8 {
            // SAFETY: The page is mapped.
            let actual_val = unsafe { base.add(i as usize).read_volatile() };
            let expected_val = match expected {
                Some(expected) => {
                    let index = (i - offset / 8) as usize * 8;
                    u64::from_ne_bytes(expected[index..index + 8].try_into().unwrap())
                }
                None => self.key + i,
            };
            if actual_val != expected_val {
                println!(
                    "mismatch at byte {}: expected {expected_val:x}, actual {actual_val:x}",
                    i * 8
                );
                return false;
            }
        }
        true
    }

    /// Validates this vmo's content in the specified pages using vmo_read.
    pub fn check_vmo(&self, offset: u64, len: u64, expected: Option<&[u8]>) -> bool {
        let len = len * page_size();
        let offset = offset * page_size();
        let tmp_vmo = match zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, len) {
            Ok(vmo) => vmo,
            Err(status) => {
                eprintln!("vmo create failed with {status}");
                return false;
            }
        };
        let buf = match root_vmar().map(
            0,
            &tmp_vmo,
            0,
            len as usize,
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
        ) {
            Ok(buf) => buf,
            Err(status) => {
                eprintln!("vmar map failed with {status}");
                return false;
            }
        };
        struct Unmap(usize, usize);
        impl Drop for Unmap {
            fn drop(&mut self) {
                // SAFETY: Nothing refers to the mapping any more.
                let _ = unsafe { root_vmar().unmap(self.0, self.1) };
            }
        }
        let _unmap = Unmap(buf, len as usize);
        // SAFETY: The buffer is mapped.
        let data = unsafe { std::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
        if self.vmo.read(data, offset).is_err() {
            return false;
        }
        for i in 0..len / 8 {
            let index = i as usize * 8;
            let actual = u64::from_ne_bytes(data[index..index + 8].try_into().unwrap());
            let expected = match expected {
                Some(expected) => {
                    u64::from_ne_bytes(expected[index..index + 8].try_into().unwrap())
                }
                None => self.key + offset / 8 + i,
            };
            if actual != expected {
                return false;
            }
        }
        true
    }

    /// Resizes the vmo.
    pub fn resize(&self, new_page_count: u64) -> bool {
        // Acquire the lock so that another racing resize does not interfere with our VMO resize
        // and mapping creation.
        let mut state = self.mutex.lock();
        let new_size = new_page_count * page_size();
        // The old and new sizes are both page-aligned, so setting the size cannot block. We won't
        // need to request a page from the UserPager to zero it partially.
        if let Err(status) = self.vmo.set_size(new_size) {
            eprintln!("vmo resize failed with {status}");
            return false;
        }
        // If we're growing the VMO, tear down the old mapping and create a new one to be able to
        // access the new range.
        if new_size > state.size {
            if state.size > 0 {
                // SAFETY: Nothing refers to the mapping any more.
                if let Err(status) =
                    unsafe { root_vmar().unmap(state.base_addr, state.size as usize) }
                {
                    eprintln!("vmar unmap failed with {status}");
                    return false;
                }
            }
            let mut addr = 0;
            if new_size > 0 {
                match root_vmar().map(
                    0,
                    &self.vmo,
                    0,
                    new_size as usize,
                    zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
                ) {
                    Ok(a) => addr = a,
                    Err(status) => {
                        eprintln!("vmar map failed with {status}");
                        return false;
                    }
                }
            }
            state.base_addr = addr;
        }
        state.size = new_size;
        true
    }

    fn op_range(&self, op: zx::VmoOp, page_offset: u64, page_count: u64) -> bool {
        self.vmo.op_range(op, page_offset * page_size(), page_count * page_size()).is_ok()
    }

    /// Commits the specified pages in this vmo.
    pub fn commit(&self, page_offset: u64, page_count: u64) -> bool {
        self.op_range(zx::VmoOp::COMMIT, page_offset, page_count)
    }

    pub fn prefetch(&self, page_offset: u64, page_count: u64) -> bool {
        self.op_range(zx::VmoOp::PREFETCH, page_offset, page_count)
    }

    /// Generates this vmo contents at the specified offset.
    pub fn generate_buffer_contents(
        &self,
        dest_buffer: &mut [u8],
        page_count: u64,
        paged_vmo_page_offset: u64,
    ) {
        let len = page_count * page_size();
        let paged_vmo_offset = paged_vmo_page_offset * page_size();
        for idx in 0..len / 8 {
            let value = self.key + paged_vmo_offset / 8 + idx;
            let index = idx as usize * 8;
            dest_buffer[index..index + 8].copy_from_slice(&value.to_ne_bytes());
        }
    }

    /// Polls until the VMO's populated bytes are `expected_bytes`.
    pub fn poll_populated_bytes(&self, expected_bytes: u64) -> bool {
        loop {
            let Ok(info) = self.vmo.info() else { return false };
            if info.populated_bytes == expected_bytes {
                return true;
            }
            println!(
                "polling again. actual bytes {} ({} pages); expected bytes {expected_bytes} ({} pages)",
                info.populated_bytes,
                info.populated_bytes / page_size(),
                expected_bytes / page_size()
            );
            zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(50)).sleep();
        }
    }

    /// Set the limit of the range that may be automatically supplied to this VMO from the
    /// UserPager::StartTaggedPageFaultHandler thread.
    pub fn set_page_fault_supply_limit(&self, pages_limit: u64) {
        self.mutex.lock().page_fault_supply_limit = pages_limit * page_size();
    }

    /// Retrieve the current page fault supply limit in pages.
    pub fn get_page_fault_supply_limit(&self) -> u64 {
        self.mutex.lock().page_fault_supply_limit
    }
}

/// Checks the content of pages, with a mapping or with vmo_read.
pub fn check_buffer_data(vmo: &Vmo, offset: u64, len: u64, data: &[u8], check_vmar: bool) -> bool {
    if check_vmar {
        vmo.check_vmar(offset, len, Some(data))
    } else {
        vmo.check_vmo(offset, len, Some(data))
    }
}

/// Checks pages have the content the pager supplies.
pub fn check_buffer(vmo: &Vmo, offset: u64, len: u64, check_vmar: bool) -> bool {
    if check_vmar { vmo.check_vmar(offset, len, None) } else { vmo.check_vmo(offset, len, None) }
}

/// A page request received on the port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Request {
    key: u64,
    command: zx_page_request_command_t,
    offset: u64,
    length: u64,
}

fn request_of(packet: &zx::Packet) -> Option<Request> {
    match packet.contents() {
        zx::PacketContents::Pager(request) => Some(Request {
            key: packet.key(),
            command: request.command(),
            offset: request.range().start,
            length: request.range().end - request.range().start,
        }),
        _ => None,
    }
}

struct PagerLocked {
    vmos: Vec<Arc<Vmo>>,
    requests: VecDeque<Request>,
}

/// A userspace pager, and the VMOs it creates.
///
/// This class is not thread-safe and is only expected to be accessed from the main test thread.
pub struct UserPager {
    pager: Option<Arc<zx::Pager>>,
    port: Option<Arc<zx::Port>>,
    next_key: u64,
    // Lock to guard modifications to vmos and requests so the overtime handler can safely inspect
    // them.
    pager_mutex: Arc<Mutex<PagerLocked>>,
    shutdown_event: Option<zx::Event>,
    pager_thread: Option<TestThread>,
    overtime_event: Option<zx::Event>,
    timeout_thread: Option<TestThread>,
}

const SHUTDOWN_KEY: u64 = 1;

impl Default for UserPager {
    fn default() -> Self {
        Self::new()
    }
}

impl UserPager {
    pub fn new() -> Self {
        Self {
            pager: None,
            port: None,
            next_key: SHUTDOWN_KEY + 1,
            pager_mutex: Arc::new(Mutex::new(PagerLocked {
                vmos: Vec::new(),
                requests: VecDeque::new(),
            })),
            shutdown_event: None,
            pager_thread: None,
            overtime_event: None,
            timeout_thread: None,
        }
    }

    /// Initializes the UserPager.
    pub fn init(&mut self) -> bool {
        match zx::Pager::create(zx::PagerOptions::empty()) {
            Ok(pager) => self.pager = Some(Arc::new(pager)),
            Err(status) => {
                eprintln!("pager create failed with {status}");
                return false;
            }
        }
        self.port = Some(Arc::new(zx::Port::create()));
        // Create a timeout event to report in the event we think we have become blocked.
        let overtime_event = zx::Event::create();
        let event = overtime_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        self.overtime_event = Some(overtime_event);
        let pager_mutex = self.pager_mutex.clone();
        let mut timeout_thread = TestThread::new(move || {
            overtime_handler(&event, &pager_mutex);
            true
        });
        let started = timeout_thread.start();
        self.timeout_thread = Some(timeout_thread);
        started
    }

    /// Closes the pager handle.
    pub fn close_pager_handle(&mut self) {
        self.pager = None;
    }

    /// Closes the pager's port handle.
    pub fn close_port_handle(&mut self) {
        self.port = None;
    }

    /// Returns the pager's port.
    pub fn port(&self) -> &zx::Port {
        self.port.as_ref().unwrap()
    }

    pub fn pager(&self) -> &zx::Pager {
        self.pager.as_ref().unwrap()
    }

    /// Creates a new paged vmo.
    pub fn create_vmo(&mut self, num_pages: u64) -> Option<Arc<Vmo>> {
        self.create_vmo_with_options(num_pages, zx::VmoOptions::empty())
    }

    /// Creates a new paged vmo with the provided create `options`.
    pub fn create_vmo_with_options(
        &mut self,
        num_pages: u64,
        options: zx::VmoOptions,
    ) -> Option<Arc<Vmo>> {
        if self.shutdown_event.is_some() {
            eprintln!("creating vmo after starting pager thread");
            return None;
        }
        let byte_size = num_pages * page_size();
        let vmo = match self.pager().create_vmo(options, self.port(), self.next_key, byte_size) {
            Ok(vmo) => vmo,
            Err(status) => {
                eprintln!("pager create_vmo failed with {status}");
                return None;
            }
        };
        // Give the VMO a name for debugging.
        let _ = vmo.set_name(&zx::Name::new_lossy("testvmo"));
        let Some(paged_vmo) = Vmo::create(vmo, byte_size, self.next_key) else {
            eprintln!("could not create Vmo instance");
            return None;
        };
        // Add 1 in case size is 0, which could be the case for a resizable VMO.
        self.next_key += byte_size / 8 + 1;
        self.pager_mutex.lock().vmos.push(paged_vmo.clone());
        Some(paged_vmo)
    }

    /// Detaches the paged vmo.
    pub fn detach_vmo(&self, vmo: &Vmo) -> bool {
        if let Err(status) = self.pager().detach_vmo(vmo.vmo()) {
            eprintln!("pager detach_vmo failed with {status}");
            return false;
        }
        true
    }

    /// Destroys the paged vmo.
    pub fn release_vmo(&self, vmo: &Arc<Vmo>) {
        // Generate an assertion error as there is no return code.
        assert!(self.shutdown_event.is_none(), "releasing vmo after starting pager thread");
        self.pager_mutex.lock().vmos.retain(|v| !Arc::ptr_eq(v, vmo));
    }

    /// Populates the specified pages with autogenerated content. `src_page_offset` is used to
    /// offset where in the temporary vmo the content is generated.
    pub fn supply_pages(&self, paged_vmo: &Vmo, dest_offset: u64, length: u64) -> bool {
        self.supply_pages_at(paged_vmo, dest_offset, length, 0)
    }

    pub fn supply_pages_at(
        &self,
        paged_vmo: &Vmo,
        dest_offset: u64,
        length: u64,
        src_offset: u64,
    ) -> bool {
        supply_pages_impl(self.pager(), paged_vmo, dest_offset, length, src_offset)
    }

    /// Populates the specified pages with the content in `src` starting at `src_page_offset`.
    pub fn supply_pages_from(
        &self,
        paged_vmo: &Vmo,
        dest_offset: u64,
        length: u64,
        src: zx::Vmo,
        src_offset: u64,
    ) -> bool {
        supply_pages_from_impl(self.pager(), paged_vmo, dest_offset, length, &src, src_offset)
    }

    /// Signals failure to populate pages in the specified range.
    pub fn fail_pages(
        &self,
        paged_vmo: &Vmo,
        page_offset: u64,
        page_count: u64,
        error_status: zx::Status,
    ) -> bool {
        let range = page_offset * page_size()..(page_offset + page_count) * page_size();
        if let Err(status) =
            self.pager().op_range(zx::PagerOp::Fail(error_status), paged_vmo.vmo(), range)
        {
            eprintln!("pager op_range FAIL failed with {status}");
            return false;
        }
        true
    }

    /// Signals that pages in the specified range can be marked dirty.
    pub fn dirty_pages(&self, paged_vmo: &Vmo, page_offset: u64, page_count: u64) -> bool {
        let range = page_offset * page_size()..(page_offset + page_count) * page_size();
        if let Err(status) = self.pager().op_range(zx::PagerOp::Dirty, paged_vmo.vmo(), range) {
            eprintln!("pager op_range DIRTY failed with {status}");
            return false;
        }
        true
    }

    /// Begins writeback on pages in the specified range.
    pub fn writeback_begin_pages(
        &self,
        paged_vmo: &Vmo,
        page_offset: u64,
        page_count: u64,
    ) -> bool {
        let range = page_offset * page_size()..(page_offset + page_count) * page_size();
        let op = zx::PagerOp::WritebackBegin(zx::PagerWritebackBeginOptions::empty());
        if let Err(status) = self.pager().op_range(op, paged_vmo.vmo(), range) {
            eprintln!("pager op_range WRITEBACK_BEGIN failed with {status}");
            return false;
        }
        true
    }

    /// Begins writeback of zeroes on pages in the specified range.
    pub fn writeback_begin_zero_pages(
        &self,
        paged_vmo: &Vmo,
        page_offset: u64,
        page_count: u64,
    ) -> bool {
        let range = page_offset * page_size()..(page_offset + page_count) * page_size();
        let op = zx::PagerOp::WritebackBegin(zx::PagerWritebackBeginOptions::DIRTY_RANGE_IS_ZERO);
        if let Err(status) = self.pager().op_range(op, paged_vmo.vmo(), range) {
            eprintln!("pager op_range WRITEBACK_BEGIN (zero_range) failed with {status}");
            return false;
        }
        true
    }

    /// Ends writeback on pages in the specified range.
    pub fn writeback_end_pages(&self, paged_vmo: &Vmo, page_offset: u64, page_count: u64) -> bool {
        let range = page_offset * page_size()..(page_offset + page_count) * page_size();
        if let Err(status) =
            self.pager().op_range(zx::PagerOp::WritebackEnd, paged_vmo.vmo(), range)
        {
            eprintln!("pager op_range WRITEBACK_END failed with {status}");
            return false;
        }
        true
    }

    /// Queries pager vmo stats, and returns whether the `paged_vmo` has been modified since the
    /// last query.
    pub fn verify_modified(&self, paged_vmo: &Vmo) -> bool {
        let mut stats = zx_pager_vmo_stats_t::default();
        // SAFETY: The buffer is the size of the stats.
        let status = unsafe {
            zx_pager_query_vmo_stats(
                self.pager().raw_handle(),
                paged_vmo.vmo().raw_handle(),
                ZX_PAGER_RESET_VMO_STATS,
                (&raw mut stats).cast(),
                size_of::<zx_pager_vmo_stats_t>(),
            )
        };
        if status != zx::sys::ZX_OK {
            eprintln!("failed to query pager vmo stats with {}", zx::Status::err_from_raw(status));
            return false;
        }
        stats.modified == ZX_PAGER_VMO_STATS_MODIFIED
    }

    /// Queries dirty ranges of pages in the specified range and verifies that they match the ones
    /// provided in `dirty_ranges_to_verify`, whose offsets and lengths are in pages.
    pub fn verify_dirty_ranges(
        &self,
        paged_vmo: &Vmo,
        dirty_ranges_to_verify: &[zx_vmo_dirty_range_t],
    ) -> bool {
        // We will query ranges twice, using both populated and unpopulated user mode buffers.
        // Unpopulated buffers will verify that faults are being captured and resolved correctly in
        // the query syscall.
        //
        // Initialize a set of buffers for the first query. Keep the number of entries > 1 but small
        // enough so that we end up making more than one syscall in the average case.
        const MAX_RANGES: usize = 2;
        let mut ranges_no_fault = [zx_vmo_dirty_range_t::default(); MAX_RANGES];
        let mut num_ranges_no_fault = 0usize;
        if !self.verify_dirty_ranges_helper(
            paged_vmo,
            dirty_ranges_to_verify,
            ranges_no_fault.as_mut_ptr(),
            MAX_RANGES * size_of::<zx_vmo_dirty_range_t>(),
            &mut num_ranges_no_fault,
        ) {
            eprintln!("failed to query dirty ranges");
            return false;
        }

        // Use a mapped VMO with no committed pages as the buffer for the second query.
        //
        // Use separate pages for the ranges buffer and the count, so they can trigger page faults
        // separately. Also set up the buffer such that its elements straddle a page boundary - map
        // an extra page and start the first element just before a page boundary, with the rest of
        // the elements on the following page; this ensures more than a single page fault for the
        // buffer.
        let page = page_size() as usize;
        let vmo_size =
            (MAX_RANGES * size_of::<zx_vmo_dirty_range_t>()).next_multiple_of(page) + 2 * page;
        let tmp_vmo = match zx::Vmo::create(vmo_size as u64) {
            Ok(vmo) => vmo,
            Err(status) => {
                eprintln!("vmo create failed with {status}");
                return false;
            }
        };
        let buf = match root_vmar().map(
            0,
            &tmp_vmo,
            0,
            vmo_size,
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
        ) {
            Ok(buf) => buf,
            Err(status) => {
                eprintln!("vmar map failed with {status}");
                return false;
            }
        };
        let ranges = (buf + page - size_of::<zx_vmo_dirty_range_t>()) as *mut zx_vmo_dirty_range_t;
        let num_ranges = (buf + vmo_size - page) as *mut usize;
        // SAFETY: Both are within the mapping.
        let result = self.verify_dirty_ranges_helper(
            paged_vmo,
            dirty_ranges_to_verify,
            ranges,
            MAX_RANGES * size_of::<zx_vmo_dirty_range_t>(),
            unsafe { &mut *num_ranges },
        );
        // SAFETY: Nothing refers to the mapping any more.
        let _ = unsafe { root_vmar().unmap(buf, vmo_size) };
        result
    }

    fn verify_dirty_ranges_helper(
        &self,
        paged_vmo: &Vmo,
        dirty_ranges_to_verify: &[zx_vmo_dirty_range_t],
        ranges_buf: *mut zx_vmo_dirty_range_t,
        ranges_buf_size: usize,
        num_ranges_buf: &mut usize,
    ) -> bool {
        let mut verify_index = 0;
        let mut start = 0u64;
        let max_ranges = ranges_buf_size / size_of::<zx_vmo_dirty_range_t>();
        let mut found_extra_ranges = false;

        // Make an initial call to verify the total number of dirty ranges, before we get into the
        // loop and advance start per iteration.
        let mut avail = 0usize;
        // SAFETY: No buffer is passed.
        let status = unsafe {
            zx_pager_query_dirty_ranges(
                self.pager().raw_handle(),
                paged_vmo.vmo().raw_handle(),
                0,
                paged_vmo.size(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut avail,
            )
        };
        if status != zx::sys::ZX_OK {
            eprintln!("query dirty ranges failed with {}", zx::Status::err_from_raw(status));
            return false;
        }
        if avail != dirty_ranges_to_verify.len() {
            eprintln!("available ranges {avail} not as expected {}", dirty_ranges_to_verify.len());
            return false;
        }

        loop {
            // SAFETY: The buffer has `ranges_buf_size` bytes.
            let status = unsafe {
                zx_pager_query_dirty_ranges(
                    self.pager().raw_handle(),
                    paged_vmo.vmo().raw_handle(),
                    start,
                    paged_vmo.size() - start,
                    ranges_buf.cast(),
                    ranges_buf_size,
                    num_ranges_buf,
                    &mut avail,
                )
            };
            if status != zx::sys::ZX_OK {
                eprintln!("query dirty ranges failed with {}", zx::Status::err_from_raw(status));
                return false;
            }
            let queried_ranges = *num_ranges_buf;
            // Something went wrong if the available ranges is less than the number copied out.
            if avail < queried_ranges {
                eprintln!("available ranges {avail} smaller than actual {queried_ranges}");
                return false;
            }
            // No dirty ranges found.
            if queried_ranges == 0 {
                break;
            }
            // If there are more ranges available, we should be using up all the space available
            // in buffer.
            if queried_ranges < avail && queried_ranges != max_ranges {
                eprintln!("queried ranges do not fully occupy buffer");
                return false;
            }
            // Verify the dirty ranges returned.
            for i in 0..queried_ranges {
                if verify_index == dirty_ranges_to_verify.len() {
                    found_extra_ranges = true;
                    break;
                }
                // SAFETY: The entry was written by the query.
                let range = unsafe { ranges_buf.add(i).read_unaligned() };
                let expected = dirty_ranges_to_verify[verify_index];
                if range.offset != expected.offset * page_size()
                    || range.length != expected.length * page_size()
                    || range.options != expected.options
                {
                    eprintln!(
                        "mismatch in queried range. expected {{{}, {}, {}}} actual {{{}, {}, {}}}",
                        expected.offset * page_size(),
                        expected.length * page_size(),
                        expected.options,
                        range.offset,
                        range.length,
                        range.options
                    );
                    return false;
                }
                verify_index += 1;
            }
            // No more ranges to verify.
            if verify_index == dirty_ranges_to_verify.len() {
                break;
            }
            // Adjust the start and length for the next query.
            // SAFETY: As above.
            let last = unsafe { ranges_buf.add(queried_ranges - 1).read_unaligned() };
            start = last.offset + last.length;
            if queried_ranges >= avail {
                break;
            }
        }
        verify_index == dirty_ranges_to_verify.len() && !found_extra_ranges
    }

    /// Checks if there is a read request for the range, waiting until `deadline`.
    pub fn wait_for_page_read(
        &self,
        vmo: &Vmo,
        page_offset: u64,
        page_count: u64,
        deadline: zx::MonotonicInstant,
    ) -> bool {
        self.wait_for_page_request(
            zx_page_request_command_t::ZX_PAGER_VMO_READ,
            vmo,
            page_offset,
            page_count,
            deadline,
        )
    }

    /// Checks if there is a dirty request for the range, waiting until `deadline`.
    pub fn wait_for_page_dirty(
        &self,
        vmo: &Vmo,
        page_offset: u64,
        page_count: u64,
        deadline: zx::MonotonicInstant,
    ) -> bool {
        self.wait_for_page_request(
            zx_page_request_command_t::ZX_PAGER_VMO_DIRTY,
            vmo,
            page_offset,
            page_count,
            deadline,
        )
    }

    /// Checks if there is a complete request for the VMO with `key`, waiting until `deadline`.
    pub fn wait_for_page_complete(&self, key: u64, deadline: zx::MonotonicInstant) -> bool {
        let expected = Request {
            key,
            command: zx_page_request_command_t::ZX_PAGER_VMO_COMPLETE,
            offset: 0,
            length: 0,
        };
        self.wait_for_request(None, |actual| *actual == expected, deadline)
    }

    fn wait_for_page_request(
        &self,
        command: zx_page_request_command_t,
        vmo: &Vmo,
        page_offset: u64,
        page_count: u64,
        deadline: zx::MonotonicInstant,
    ) -> bool {
        let expected = Request {
            key: vmo.key(),
            command,
            offset: page_offset * page_size(),
            length: page_count * page_size(),
        };
        self.wait_for_request(Some(vmo), |actual| *actual == expected, deadline)
    }

    /// Gets the first page read request, returning its offset and length in pages.
    pub fn get_page_read_request(
        &self,
        vmo: &Vmo,
        deadline: zx::MonotonicInstant,
    ) -> Option<(u64, u64)> {
        self.get_page_request(vmo, zx_page_request_command_t::ZX_PAGER_VMO_READ, deadline)
    }

    /// Gets the first page dirty request, returning its offset and length in pages.
    pub fn get_page_dirty_request(
        &self,
        vmo: &Vmo,
        deadline: zx::MonotonicInstant,
    ) -> Option<(u64, u64)> {
        self.get_page_request(vmo, zx_page_request_command_t::ZX_PAGER_VMO_DIRTY, deadline)
    }

    /// Gets the first page request with `command`, returning its offset and length in pages.
    pub fn get_page_request(
        &self,
        vmo: &Vmo,
        command: zx_page_request_command_t,
        deadline: zx::MonotonicInstant,
    ) -> Option<(u64, u64)> {
        let mut found = None;
        let key = vmo.key();
        let matched = self.wait_for_request(
            Some(vmo),
            |request| {
                if request.key == key && request.command == command {
                    found = Some((request.offset / page_size(), request.length / page_size()));
                    return true;
                }
                false
            },
            deadline,
        );
        if matched { found } else { None }
    }

    fn wait_for_request(
        &self,
        vmo: Option<&Vmo>,
        mut cmp_fn: impl FnMut(&Request) -> bool,
        deadline: zx::MonotonicInstant,
    ) -> bool {
        {
            let mut locked = self.pager_mutex.lock();
            if let Some(index) = locked.requests.iter().position(&mut cmp_fn) {
                locked.requests.remove(index);
                return true;
            }
        }
        let mut now = zx::MonotonicInstant::get();
        let deadline = deadline.max(now);
        while now <= deadline {
            // TODO: this can block forever if the thread that's supposed to generate the request
            // unexpectedly dies.
            match self.port().wait(deadline) {
                Ok(packet) => {
                    let request = request_of(&packet).expect("page request packet");
                    if cmp_fn(&request) {
                        return true;
                    }
                    // Received a message that was not for us. Before continuing to wait check if
                    // we were given a specific related VMO and check it for any
                    // committed_change_events. Doing this should catch some cases where we are
                    // waiting for one kind of request, but an unexpected reclamation happened and
                    // hence we received a different event.
                    if let Some(vmo) = vmo {
                        let info = vmo.vmo().info().unwrap();
                        assert_eq!(info.committed_change_events, 0);
                    }
                    self.pager_mutex.lock().requests.push_front(request);
                }
                // Don't advance now on success, to make sure we read any pending requests
                Err(_) => now = zx::MonotonicInstant::get(),
            }
        }
        false
    }

    /// Starts a thread to handle any page faults. Faulted in pages are initialized with the
    /// default page tagged data as per SupplyPages. This function is not thread safe, and should
    /// only be called once. After starting a pager thread it is an error to create or destroy
    /// VMOs, as this could lead to data races.
    pub fn start_tagged_page_fault_handler(&mut self) -> bool {
        if self.shutdown_event.is_some() {
            eprintln!("Page fault handler already created");
            return false;
        }
        let shutdown_event = zx::Event::create();
        if shutdown_event
            .wait_async(self.port(), SHUTDOWN_KEY, zx::Signals::USER_0, zx::WaitAsyncOpts::empty())
            .is_err()
        {
            eprintln!("Failed to associate shutdown event with port");
            return false;
        }
        self.shutdown_event = Some(shutdown_event);
        let port = self.port.clone().unwrap();
        let pager = self.pager.clone().unwrap();
        let pager_mutex = self.pager_mutex.clone();
        let mut pager_thread = TestThread::new(move || {
            page_fault_handler(&port, &pager, &pager_mutex);
            true
        });
        if !pager_thread.start() {
            eprintln!("Failed to start page fault handling thread");
            return false;
        }
        self.pager_thread = Some(pager_thread);
        true
    }
}

impl Drop for UserPager {
    fn drop(&mut self) {
        // If a pager thread was started, gracefully shut it down.
        if let Some(shutdown_event) = &self.shutdown_event {
            shutdown_event.signal(zx::Signals::NONE, zx::Signals::USER_0).unwrap();
            self.pager_thread.as_mut().unwrap().wait();
        }
        if let Some(overtime_event) = &self.overtime_event {
            overtime_event.signal(zx::Signals::NONE, zx::Signals::USER_0).unwrap();
            self.timeout_thread.as_mut().unwrap().wait();
        }
        self.pager_mutex.lock().vmos.clear();
    }
}

fn supply_pages_impl(
    pager: &zx::Pager,
    paged_vmo: &Vmo,
    dest_offset: u64,
    length: u64,
    src_offset: u64,
) -> bool {
    let vmo = match zx::Vmo::create((length + src_offset) * page_size()) {
        Ok(vmo) => vmo,
        Err(status) => {
            eprintln!("vmo create failed with {status}");
            return false;
        }
    };
    let mut data = vec![0u8; page_size() as usize];
    for cur in 0..length {
        paged_vmo.generate_buffer_contents(&mut data, 1, dest_offset + cur);
        if let Err(status) = vmo.write(&data, (src_offset + cur) * page_size()) {
            eprintln!("vmo write failed with {status}");
            return false;
        }
    }
    supply_pages_from_impl(pager, paged_vmo, dest_offset, length, &vmo, src_offset)
}

fn supply_pages_from_impl(
    pager: &zx::Pager,
    paged_vmo: &Vmo,
    dest_offset: u64,
    length: u64,
    src: &zx::Vmo,
    src_offset: u64,
) -> bool {
    let range = dest_offset * page_size()..(dest_offset + length) * page_size();
    if let Err(status) = pager.supply_pages(paged_vmo.vmo(), range, src, src_offset * page_size()) {
        eprintln!("pager supply_pages failed with {status}");
        return false;
    }
    true
}

fn page_fault_handler(port: &zx::Port, pager: &zx::Pager, pager_mutex: &Mutex<PagerLocked>) {
    loop {
        let packet = match port.wait(zx::MonotonicInstant::INFINITE) {
            Ok(packet) => packet,
            Err(status) => {
                eprintln!("Unexpected err {status} waiting on port");
                return;
            }
        };
        if packet.key() == SHUTDOWN_KEY {
            assert!(matches!(packet.contents(), zx::PacketContents::SignalOne(_)));
            return;
        }
        let request = request_of(&packet).expect("page request packet");
        if request.command == zx_page_request_command_t::ZX_PAGER_VMO_READ {
            let locked = pager_mutex.lock();
            // Just brute force find matching VMO keys, no need for efficiency.
            for vmo in locked.vmos.iter().filter(|vmo| vmo.key() == request.key) {
                // Trim the request to the VMOs supply limit, and then supply any non-zero
                // resulting range.
                let request_limit =
                    (request.offset + request.length).min(vmo.get_page_fault_supply_limit());
                if request_limit > request.offset {
                    let supply_len = request_limit - request.offset;
                    supply_pages_impl(
                        pager,
                        vmo,
                        request.offset / page_size(),
                        supply_len / page_size(),
                        0,
                    );
                }
            }
        }
    }
}

fn overtime_handler(overtime_event: &zx::Event, pager_mutex: &Mutex<PagerLocked>) {
    // The timeout here is sized somewhat low since it's okay if we occasionally spuriously fire
    // as we are just printing out information, not modifying any state.
    const OVERTIME_SECONDS: i64 = 60;
    loop {
        let result = overtime_event.wait_one(
            zx::Signals::USER_0,
            zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(OVERTIME_SECONDS)),
        );
        if result.to_result().is_ok_and(|pending| pending.contains(zx::Signals::USER_0)) {
            return;
        }
        println!(
            "Suspected test hang, UserPager object has been active for over {OVERTIME_SECONDS} seconds"
        );
        let locked = pager_mutex.lock();
        println!("UserPager has {} unhandled requests:", locked.requests.len());
        for request in &locked.requests {
            println!(
                "Key: {} command: {:?} offset: {} length: {}",
                request.key, request.command, request.offset, request.length
            );
        }
        println!("UserPager has {} registered VMOs:", locked.vmos.len());
        for vmo in &locked.vmos {
            match vmo.vmo().info() {
                Ok(info) => println!(
                    "\tKey: {} size: {} children: {} mappings: {} commited: {} populated: {} change events: {}",
                    vmo.key(),
                    info.size_bytes,
                    info.num_children,
                    info.num_mappings,
                    info.committed_bytes,
                    info.populated_bytes,
                    info.committed_change_events
                ),
                Err(status) => {
                    println!("\tFailed to retrieve info for vmo with key {}: {status}", vmo.key());
                    return;
                }
            }
        }
    }
}

/// Default rights of new objects, from `zircon/system/public/zircon/rights.h`.
pub mod rights {
    use zx::sys::*;

    pub const ZX_RIGHTS_BASIC: zx_rights_t =
        ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE | ZX_RIGHT_WAIT | ZX_RIGHT_INSPECT;
    pub const ZX_RIGHTS_IO: zx_rights_t = ZX_RIGHT_READ | ZX_RIGHT_WRITE;
    pub const ZX_RIGHTS_PROPERTY: zx_rights_t = ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY;
    pub const ZX_DEFAULT_PORT_RIGHTS: zx_rights_t =
        (ZX_RIGHTS_BASIC & !ZX_RIGHT_WAIT) | ZX_RIGHTS_IO;
    pub const ZX_DEFAULT_VMO_RIGHTS: zx_rights_t =
        ZX_RIGHTS_BASIC | ZX_RIGHTS_IO | ZX_RIGHTS_PROPERTY | ZX_RIGHT_MAP | ZX_RIGHT_SIGNAL;
    pub const ZX_DEFAULT_PAGER_RIGHTS: zx_rights_t =
        ZX_RIGHT_INSPECT | ZX_RIGHT_TRANSFER | ZX_RIGHT_ATTACH_VMO | ZX_RIGHT_MANAGE_VMO;
}
