// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Mappings of VMOs into the process, faulted in as they are accessed.

use std::sync::Arc;

unsafe extern "C" {
    /// From `zircon/process.h`.
    fn zx_vmar_root_self() -> zx::sys::zx_handle_t;
}

fn root_vmar() -> zx::Unowned<'static, zx::Vmar> {
    // SAFETY: The root VMAR handle lives as long as the process.
    unsafe { zx::Unowned::from_raw_handle(zx_vmar_root_self()) }
}

fn page_size() -> usize {
    zx::system_get_page_size() as usize
}

const RW: zx::VmarFlags = zx::VmarFlags::PERM_READ.union(zx::VmarFlags::PERM_WRITE);

/// A mapping, unmapped when dropped.
struct Mapping {
    addr: usize,
    len: usize,
}

impl Mapping {
    fn new(vmo: &zx::Vmo, len: usize, flags: zx::VmarFlags) -> Self {
        let addr = root_vmar().map(0, vmo, 0, len, flags).unwrap();
        Self { addr, len }
    }

    fn ptr(&self, offset: usize) -> *mut u8 {
        assert!(offset < self.len);
        (self.addr + offset) as *mut u8
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: Nothing refers to the mapping any more.
        unsafe { root_vmar().unmap(self.addr, self.len) }.unwrap();
    }
}

#[test]
fn root_vmar_info() {
    let info = root_vmar().info().unwrap();
    assert_eq!(info.len, 1 << 36);
    assert_eq!(info.base % page_size(), 0);
}

#[test]
fn writes_through_a_mapping_reach_the_vmo() {
    let len = 4 * page_size();
    let vmo = zx::Vmo::create(len as u64).unwrap();
    let mapping = Mapping::new(&vmo, len, RW);
    // SAFETY: The mapping is readable and writable.
    unsafe {
        assert_eq!(mapping.ptr(page_size()).read_volatile(), 0);
        mapping.ptr(page_size() + 1).write_volatile(0xab);
        mapping.ptr(3 * page_size()).write_volatile(0xcd);
    }
    let mut buf = [0u8; 2];
    vmo.read(&mut buf, page_size() as u64).unwrap();
    assert_eq!(buf, [0, 0xab]);
    vmo.read(&mut buf[..1], 3 * page_size() as u64).unwrap();
    assert_eq!(buf[0], 0xcd);

    vmo.write(&[0x12], 2 * page_size() as u64).unwrap();
    // SAFETY: As above.
    assert_eq!(unsafe { mapping.ptr(2 * page_size()).read_volatile() }, 0x12);
}

#[test]
fn decommit_unmaps_pages() {
    let len = page_size();
    let vmo = zx::Vmo::create(len as u64).unwrap();
    let mapping = Mapping::new(&vmo, len, RW);
    // SAFETY: The mapping is readable and writable.
    unsafe { mapping.ptr(0).write_volatile(7) };
    vmo.op_range(zx::VmoOp::DECOMMIT, 0, len as u64).unwrap();
    // SAFETY: As above.
    assert_eq!(unsafe { mapping.ptr(0).read_volatile() }, 0);
}

#[test]
fn unmap_part_of_a_mapping_splits_it() {
    let len = 3 * page_size();
    let vmo = zx::Vmo::create(len as u64).unwrap();
    let addr = root_vmar().map(0, &vmo, 0, len, RW).unwrap();
    // SAFETY: The middle page is not accessed after it is unmapped.
    unsafe {
        root_vmar().unmap(addr + page_size(), page_size()).unwrap();
        ((addr + 2 * page_size()) as *mut u8).write_volatile(9);
        (addr as *mut u8).write_volatile(8);
    }
    let mut buf = [0u8; 1];
    vmo.read(&mut buf, 2 * page_size() as u64).unwrap();
    assert_eq!(buf, [9]);
    vmo.read(&mut buf, 0).unwrap();
    assert_eq!(buf, [8]);
    // Unmapping the whole range again is fine, even though part of it is unmapped already.
    // SAFETY: Nothing refers to the mapping any more.
    unsafe { root_vmar().unmap(addr, len) }.unwrap();
}

#[test]
fn allocate_and_destroy_a_child_vmar() {
    let len = 2 * page_size();
    let (vmar, base) = root_vmar()
        .allocate(
            0,
            len,
            zx::VmarFlags::CAN_MAP_READ
                | zx::VmarFlags::CAN_MAP_WRITE
                | zx::VmarFlags::CAN_MAP_SPECIFIC,
        )
        .unwrap();
    let vmo = zx::Vmo::create(len as u64).unwrap();
    let addr = vmar.map(page_size(), &vmo, 0, page_size(), RW | zx::VmarFlags::SPECIFIC).unwrap();
    assert_eq!(addr, base + page_size());
    // SAFETY: The page is mapped.
    unsafe { (addr as *mut u8).write_volatile(3) };
    // SAFETY: Nothing refers to the mappings any more.
    unsafe { vmar.destroy() }.unwrap();
    let mut buf = [0u8; 1];
    vmo.read(&mut buf, 0).unwrap();
    assert_eq!(buf, [3]);
}

const KEY: u64 = 5;

fn page_request(packet: &zx::Packet) -> zx::PagerPacket {
    assert_eq!(packet.key(), KEY);
    match packet.contents() {
        zx::PacketContents::Pager(request) => request,
        packet => panic!("Unexpected packet: {:?}", packet),
    }
}

/// Reading a mapped page of a pager-backed VMO blocks the reading thread until the pager supplies
/// the page.
#[test]
fn fault_on_a_pager_backed_mapping_waits_for_supply() {
    let len = page_size();
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, KEY, len as u64).unwrap();
    let mapping = Arc::new(Mapping::new(&vmo, len, RW));

    let reader = std::thread::spawn({
        let mapping = mapping.clone();
        // SAFETY: The mapping is readable.
        move || unsafe { mapping.ptr(10).read_volatile() }
    });

    let request = page_request(&port.wait(zx::MonotonicInstant::INFINITE).unwrap());
    assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ);
    assert_eq!(request.range(), 0..len as u64);
    let aux = zx::Vmo::create(len as u64).unwrap();
    aux.write(&[42], 10).unwrap();
    pager.supply_pages(&vmo, request.range(), &aux, 0).unwrap();

    assert_eq!(reader.join().unwrap(), 42);
}

/// With dirty transitions trapped, the first write through a mapping blocks until the pager
/// dirties the page, and after writeback begins the next write is trapped again.
#[test]
fn writes_to_a_pager_backed_mapping_are_trapped_until_dirtied() {
    let len = page_size();
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmo = pager.create_vmo(zx::VmoOptions::TRAP_DIRTY, &port, KEY, len as u64).unwrap();
    let aux = zx::Vmo::create(len as u64).unwrap();
    pager.supply_pages(&vmo, 0..len as u64, &aux, 0).unwrap();
    let mapping = Arc::new(Mapping::new(&vmo, len, RW));

    for value in [1u8, 2] {
        let writer = std::thread::spawn({
            let mapping = mapping.clone();
            // SAFETY: The mapping is writable.
            move || unsafe { mapping.ptr(0).write_volatile(value) }
        });
        let request = page_request(&port.wait(zx::MonotonicInstant::INFINITE).unwrap());
        assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_DIRTY);
        assert_eq!(request.range(), 0..len as u64);
        pager.op_range(zx::PagerOp::Dirty, &vmo, request.range()).unwrap();
        writer.join().unwrap();

        let mut buf = [0u8; 1];
        vmo.read(&mut buf, 0).unwrap();
        assert_eq!(buf, [value]);
        pager
            .op_range(
                zx::PagerOp::WritebackBegin(zx::PagerWritebackBeginOptions::empty()),
                &vmo,
                0..len as u64,
            )
            .unwrap();
        pager.op_range(zx::PagerOp::WritebackEnd, &vmo, 0..len as u64).unwrap();
    }
}
