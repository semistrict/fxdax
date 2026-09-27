// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The pager tests of the `zx` crate, copied from `sdk/rust/zx/src/pager.rs`, which cannot build
//! its own tests outside Fuchsia.

use std::sync::Arc;

const KEY: u64 = 5;

#[test]
fn create_vmo() {
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmo = pager.create_vmo(zx::VmoOptions::RESIZABLE, &port, KEY, /*size=*/ 100).unwrap();
    let vmo_info = vmo.info().unwrap();
    assert!(vmo_info.flags.contains(zx::VmoInfoFlags::PAGER_BACKED));
    assert!(vmo_info.flags.contains(zx::VmoInfoFlags::RESIZABLE));
}

#[test]
fn detach_vmo() {
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, KEY, /*size=*/ 100).unwrap();
    pager.detach_vmo(&vmo).unwrap();

    // If the vmo was not detached then the test will time out waiting for the page to be
    // supplied.
    let mut data = [0u8; 100];
    let e = vmo.read(&mut data, 0).expect_err("A detached vmo should fail reads");
    assert_eq!(e, zx::Status::BAD_STATE);
}

#[test]
fn supply_pages() {
    let page_size: u64 = zx::system_get_page_size().into();
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmo = Arc::new(pager.create_vmo(zx::VmoOptions::empty(), &port, KEY, page_size).unwrap());

    let read_thread = std::thread::spawn({
        let vmo = vmo.clone();
        move || {
            let mut data = [0u8; 100];
            vmo.read(&mut data, 0).unwrap();
        }
    });

    let packet = port.wait(zx::MonotonicInstant::INFINITE).unwrap();
    assert_eq!(packet.key(), KEY);
    match packet.contents() {
        zx::PacketContents::Pager(request) => {
            assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ);
            assert_eq!(request.range(), 0..page_size);
            let aux_vmo = zx::Vmo::create(page_size).unwrap();
            pager.supply_pages(vmo.as_ref(), request.range(), &aux_vmo, 0).unwrap();
        }
        packet => panic!("Unexpected packet: {:?}", packet),
    }
    read_thread.join().unwrap();
}

#[test]
fn fail_page_request() {
    let page_size: u64 = zx::system_get_page_size().into();
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmo = Arc::new(pager.create_vmo(zx::VmoOptions::empty(), &port, KEY, page_size).unwrap());

    let read_thread = std::thread::spawn({
        let vmo = vmo.clone();
        move || {
            let mut data = [0u8; 100];
            let e = vmo.read(&mut data, 0).expect_err("Request should have failed");
            assert_eq!(e, zx::Status::NO_SPACE);
        }
    });

    let packet = port.wait(zx::MonotonicInstant::INFINITE).unwrap();
    assert_eq!(packet.key(), KEY);
    match packet.contents() {
        zx::PacketContents::Pager(request) => {
            assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ);
            assert_eq!(request.range(), 0..page_size);
            pager
                .op_range(zx::PagerOp::Fail(zx::Status::NO_SPACE), vmo.as_ref(), request.range())
                .unwrap();
        }
        packet => panic!("Unexpected packet: {:?}", packet),
    }
    read_thread.join().unwrap();
}

#[test]
fn pager_writeback() {
    let page_size: u64 = zx::system_get_page_size().into();
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmo =
        Arc::new(pager.create_vmo(zx::VmoOptions::TRAP_DIRTY, &port, KEY, page_size).unwrap());

    let write_thread = std::thread::spawn({
        let vmo = vmo.clone();
        move || {
            let data = [0u8; 100];
            vmo.write(&data, 0).unwrap();
        }
    });

    let packet = port.wait(zx::MonotonicInstant::INFINITE).unwrap();
    assert_eq!(packet.key(), KEY);
    match packet.contents() {
        zx::PacketContents::Pager(request) => {
            assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ);
            assert_eq!(request.range(), 0..page_size);
            let aux_vmo = zx::Vmo::create(page_size).unwrap();
            pager.supply_pages(vmo.as_ref(), request.range(), &aux_vmo, 0).unwrap();
        }
        packet => panic!("Unexpected packet: {:?}", packet),
    }

    let packet = port.wait(zx::MonotonicInstant::INFINITE).unwrap();
    assert_eq!(packet.key(), KEY);
    match packet.contents() {
        zx::PacketContents::Pager(request) => {
            assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_DIRTY);
            assert_eq!(request.range(), 0..page_size);
            pager.op_range(zx::PagerOp::Dirty, vmo.as_ref(), request.range()).unwrap();
        }
        packet => panic!("Unexpected packet: {:?}", packet),
    }

    write_thread.join().unwrap();

    // TODO(https://fxbug.dev/42142550) Verify that the first page is dirty with `query_dirty_ranges`.
    pager
        .op_range(
            zx::PagerOp::WritebackBegin(zx::PagerWritebackBeginOptions::empty()),
            vmo.as_ref(),
            0..page_size,
        )
        .unwrap();
    pager.op_range(zx::PagerOp::WritebackEnd, vmo.as_ref(), 0..page_size).unwrap();
    // TODO(https://fxbug.dev/42142550) Verify that the first page is now clean with `query_dirty_ranges`.
}

#[test]
fn pager_writeback_begin_with_dirtied_zero_range() {
    let page_size: u64 = zx::system_get_page_size().into();
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmo = pager.create_vmo(zx::VmoOptions::RESIZABLE, &port, KEY, page_size).unwrap();
    let aux_vmo = zx::Vmo::create(page_size).unwrap();
    pager.supply_pages(&vmo, 0..page_size, &aux_vmo, 0).unwrap();
    vmo.set_size(page_size * 3).unwrap();

    // TODO(https://fxbug.dev/42142550) Verify that pages 2, and 3 are dirty and zero with
    // `query_dirty_ranges`.

    vmo.write(&[0, 1, 2, 3], page_size * 2).unwrap();
    pager
        .op_range(
            zx::PagerOp::WritebackBegin(zx::PagerWritebackBeginOptions::DIRTY_RANGE_IS_ZERO),
            &vmo,
            page_size..(page_size * 2),
        )
        .unwrap();
    pager.op_range(zx::PagerOp::WritebackEnd, &vmo, page_size..(page_size * 2)).unwrap();

    // TODO(https://fxbug.dev/42142550) Verify that page 2 is still dirty and not zero with
    // `query_dirty_ranges`.
}
