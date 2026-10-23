// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of memory that is never out (`OomBehavior::Evict`, TASK-50.3), which Zircon lacks, through
//! the system calls: what nothing makes room for fails, rather than wait or ask again for ever.
//!
//! Each test configures a small kernel before it first runs, which works as nextest runs each
//! test in its own process.

use std::sync::Arc;
use zircon::boot_options::{BootOptions, OomBehavior};
use zircon::vm::physmap::CHUNK_SIZE;
use zircon::vm::pmm;
use zircon_core_tests::userpager::page_size;

const INFINITE: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE;
/// The key of the pager's VMO's page requests.
const PAGER_KEY: u64 = 1;
/// The key of the packet the writer queues once its write returns.
const WRITTEN_KEY: u64 = 2;

/// Boots a kernel of 16 chunks of memory, which is never out, as by default.
fn boot() {
    let options = BootOptions { memory_bytes: (16 * CHUNK_SIZE) as u64, ..Default::default() };
    assert_eq!(options.oom_behavior, OomBehavior::Evict);
    BootOptions::set(options).expect("the kernel has not started");
}

/// Takes every free page with an anonymous VMO's pages, which eviction cannot free, and returns
/// the VMO.
fn fill_with_anonymous_pages() -> zx::Vmo {
    let len = pmm::count_free_pages() * page_size();
    let vmo = zx::Vmo::create(len).unwrap();
    vmo.op_range(zx::VmoOp::COMMIT, 0, len).unwrap();
    assert_eq!(pmm::count_free_pages(), 0);
    vmo
}

/// Writes a page of a trap-dirty pager-backed VMO whose content is zero, which takes a page to
/// dirty, once every free page is taken by pages nothing evicts, and returns what the write
/// returned. The pager is asked to dirty the page, and does so (`ZX_PAGER_OP_DIRTY`), which fails
/// for want of memory: the write must then fail too, as the kernel failed it, without asking again.
/// The pager failing the page with another error, as the test does next, fails a write left
/// waiting, and so does the kernel's failure of a write asking again, rather than hang the test.
fn write_with_memory_full() -> Result<(), zx::Status> {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = Arc::new(zx::Port::create());
    let page = 0..page_size();
    let vmo = pager.create_vmo(zx::VmoOptions::TRAP_DIRTY, &port, PAGER_KEY, page_size()).unwrap();
    let vmo = Arc::new(vmo);
    // Supplied from an empty VMO, the page is a zero page marker, which takes a page to dirty.
    pager.supply_pages(&vmo, page.clone(), &zx::Vmo::create(page_size()).unwrap(), 0).unwrap();
    let _anonymous = fill_with_anonymous_pages();

    let writer = std::thread::spawn({
        let (vmo, port) = (vmo.clone(), port.clone());
        move || {
            let written = vmo.write(&[0xaa], 0);
            let user = zx::UserPacket::from_u8_array([0; 32]);
            port.queue(&zx::Packet::from_user_packet(WRITTEN_KEY, 0, user)).unwrap();
            written
        }
    });
    let request = port.wait(INFINITE).unwrap();
    let zx::PacketContents::Pager(contents) = request.contents() else {
        panic!("not a page request: {request:?}");
    };
    assert_eq!(request.key(), PAGER_KEY);
    assert_eq!(contents.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_DIRTY);
    assert_eq!(contents.range(), page);

    let dirtied = pager.op_range(zx::PagerOp::Dirty, &vmo, page.clone());
    assert_eq!(dirtied, Err(zx::Status::NO_MEMORY));
    pager.op_range(zx::PagerOp::Fail(zx::Status::IO), &vmo, page).unwrap();
    let next = port.wait(INFINITE).unwrap();
    assert_eq!(next.key(), WRITTEN_KEY, "the write asked again: {next:?}");
    writer.join().unwrap()
}

/// A write that takes a page to dirty, with memory full of pages nothing evicts, fails with
/// `NO_MEMORY`. Its pager's DIRTY request, failing for want of memory, was resolved for the write
/// to ask again, and the write asked again for ever, as Fxfs's writes to its page cache did.
#[test]
fn writes_fail_with_memory_full_of_pages_nothing_evicts() {
    boot();
    assert_eq!(write_with_memory_full(), Err(zx::Status::NO_MEMORY));
}

/// With relief, as the extent daemon's, whose evictor frees nothing here, the pager's DIRTY
/// request waits for relief's pass, and then fails, its arena exhausted; and the write fails with
/// `NO_MEMORY`. Nothing failed the write's DIRTY request, nor resolved it, and the write waited
/// for ever (the pager's failure fails it here, with `IO`).
#[test]
fn relieved_writes_fail_with_memory_full_of_pages_nothing_evicts() {
    boot();
    pmm::relieve_with(|_, _| 0).unwrap();
    assert_eq!(write_with_memory_full(), Err(zx::Status::NO_MEMORY));
}
