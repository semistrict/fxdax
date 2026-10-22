// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A pager with many VMOs, as the extent cache's has one for each extent it caches.

const INFINITE_PAST: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE_PAST;

/// fxdax (TASK-92.34): a pager lets go of each of its VMOs' proxies as their COMPLETE packets are
/// read, on the thread reading its port, which delivers every page request of all its VMOs: each
/// found at once, in any order. The last made goes first here, which a scan of the pager's proxies
/// for each made quadratic, about four seconds for 50,000 (macOS, debug).
#[test]
fn many_pager_vmos_are_let_go_of_in_any_order() {
    const VMOS: u64 = 50_000;
    let page_size = zx::system_get_page_size() as u64;
    let port = zx::Port::create();
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let vmos: Vec<zx::Vmo> = (0..VMOS)
        .map(|key| pager.create_vmo(zx::VmoOptions::empty(), &port, key, page_size).unwrap())
        .collect();
    for vmo in vmos.into_iter().rev() {
        drop(vmo);
    }
    for key in (0..VMOS).rev() {
        let packet = port.wait(INFINITE_PAST).unwrap();
        assert_eq!(packet.key(), key);
        let zx::PacketContents::Pager(request) = packet.contents() else {
            panic!("expected a page request, got {:?}", packet.contents());
        };
        assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_COMPLETE);
    }
    assert_eq!(port.wait(INFINITE_PAST), Err(zx::Status::TIMED_OUT));
}
