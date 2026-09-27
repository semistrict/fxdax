// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The VMO tests of the `zx` crate, copied from `sdk/rust/zx/src/vmo.rs`, which cannot build its
//! own tests outside Fuchsia.
//!
//! Not copied, as the kernel lacks what they need: `vmo_create_contiguous` (BTIs and IOMMUs),
//! `vmo_get_child_info`, `vmo_child_snapshot`, `vmo_child_snapshot_at_least_on_write`,
//! `vmo_child_no_write` and `vmo_create_child` (VMO children), and `vmo_replace_as_executeable`
//! (resources).
//!
//! Deviation: `vmo_set_size` and `vmo_set_size_fails_on_non_resizable` use the system page size
//! rather than assuming 4 KiB pages.

// The tests are kept as they were written.
#![allow(clippy::unnecessary_cast, clippy::useless_vec)]

use test_case::test_case;
use zerocopy::{FromBytes, KnownLayout};
use zx::{Status, Vmo, VmoInfoFlags, VmoOp, VmoOptions};

#[test]
fn vmo_get_size() {
    let size = 16 * 1024 * 1024;
    let vmo = Vmo::create(size).unwrap();
    assert_eq!(size, vmo.get_size().unwrap());
}

#[test]
fn vmo_set_size() {
    // Use a multiple of page size to match VMOs page aligned size
    let page_size = u64::from(zx::system_get_page_size());
    let start_size = page_size;
    let vmo = Vmo::create_with_opts(VmoOptions::RESIZABLE, start_size).unwrap();
    assert_eq!(start_size, vmo.get_size().unwrap());

    // Change the size and make sure the new size is reported
    let new_size = 2 * page_size;
    assert!(vmo.set_size(new_size).is_ok());
    assert_eq!(new_size, vmo.get_size().unwrap());
}

#[test]
fn vmo_get_info_default() {
    let size = 4096;
    let vmo = Vmo::create(size).unwrap();
    let info = vmo.info().unwrap();
    assert!(!info.flags.contains(VmoInfoFlags::PAGER_BACKED));
    assert!(info.flags.contains(VmoInfoFlags::PAGED));
}

#[test]
fn vmo_set_size_fails_on_non_resizable() {
    let page_size = u64::from(zx::system_get_page_size());
    let size = page_size;
    let vmo = Vmo::create(size).unwrap();
    assert_eq!(size, vmo.get_size().unwrap());

    let new_size = 2 * page_size;
    assert_eq!(Err(Status::UNAVAILABLE), vmo.set_size(new_size));
    assert_eq!(size, vmo.get_size().unwrap());
}

#[test_case(0)]
#[test_case(1)]
fn vmo_read_to_array(read_offset: usize) {
    const ACTUAL_SIZE: usize = 5;
    const ACTUAL: [u8; ACTUAL_SIZE] = [1, 2, 3, 4, 5];
    let vmo = Vmo::create(ACTUAL.len() as u64).unwrap();
    vmo.write(&ACTUAL, 0).unwrap();
    let read_len = ACTUAL_SIZE - read_offset;
    assert_eq!(
        &vmo.read_to_array::<u8, ACTUAL_SIZE>(read_offset as u64).unwrap()[..read_len],
        &ACTUAL[read_offset..]
    );
}

#[test_case(0)]
#[test_case(1)]
fn vmo_read_to_vec(read_offset: usize) {
    const ACTUAL_SIZE: usize = 4;
    const ACTUAL: [u8; ACTUAL_SIZE] = [6, 7, 8, 9];
    let vmo = Vmo::create(ACTUAL.len() as u64).unwrap();
    vmo.write(&ACTUAL, 0).unwrap();
    let read_len = ACTUAL_SIZE - read_offset;
    assert_eq!(
        &vmo.read_to_vec::<u8>(read_offset as u64, read_len as u64).unwrap(),
        &ACTUAL[read_offset..]
    );
}

#[test_case(0)]
#[test_case(1)]
fn vmo_read_to_object(read_offset: usize) {
    #[repr(C)]
    #[derive(Copy, Clone, Debug, Eq, KnownLayout, FromBytes, PartialEq)]
    struct Object {
        a: u8,
        b: u8,
    }

    const ACTUAL_SIZE: usize = std::mem::size_of::<Object>();
    const ACTUAL: [u8; ACTUAL_SIZE + 1] = [10, 11, 12];
    let vmo = Vmo::create(ACTUAL.len() as u64).unwrap();
    vmo.write(&ACTUAL, 0).unwrap();
    assert_eq!(
        vmo.read_to_object::<Object>(read_offset as u64).unwrap(),
        Object { a: ACTUAL[read_offset], b: ACTUAL[1 + read_offset] }
    );
}

#[test]
fn vmo_read_write() {
    let mut vec1 = vec![0; 16];
    let vmo = Vmo::create(4096 as u64).unwrap();
    assert!(vmo.write(b"abcdef", 0).is_ok());
    assert!(vmo.read(&mut vec1, 0).is_ok());
    assert_eq!(b"abcdef", &vec1[0..6]);
    assert!(vmo.write(b"123", 2).is_ok());
    assert!(vmo.read(&mut vec1, 0).is_ok());
    assert_eq!(b"ab123f", &vec1[0..6]);

    // Read one byte into the vmo.
    assert!(vmo.read(&mut vec1, 1).is_ok());
    assert_eq!(b"b123f", &vec1[0..5]);

    assert_eq!(&vmo.read_to_vec::<u8>(0, 6).expect("read_to_vec failed"), b"ab123f");
}

#[test]
fn vmo_op_range_unsupported() {
    let vmo = Vmo::create(12).unwrap();
    assert_eq!(vmo.op_range(VmoOp::LOCK, 0, 1), Err(Status::NOT_SUPPORTED));
    assert_eq!(vmo.op_range(VmoOp::UNLOCK, 0, 1), Err(Status::NOT_SUPPORTED));
}

#[test]
fn vmo_cache() {
    let vmo = Vmo::create(12).unwrap();

    // Cache operations should all succeed.
    assert_eq!(vmo.op_range(VmoOp::CACHE_SYNC, 0, 12), Ok(()));
    assert_eq!(vmo.op_range(VmoOp::CACHE_INVALIDATE, 0, 12), Ok(()));
    assert_eq!(vmo.op_range(VmoOp::CACHE_CLEAN, 0, 12), Ok(()));
    assert_eq!(vmo.op_range(VmoOp::CACHE_CLEAN_INVALIDATE, 0, 12), Ok(()));
}

#[test]
fn vmo_content_size() {
    let start_size = 1024;
    let vmo = Vmo::create_with_opts(VmoOptions::RESIZABLE, start_size).unwrap();
    assert_eq!(vmo.get_content_size().unwrap(), start_size);
    vmo.set_content_size(&0).unwrap();
    assert_eq!(vmo.get_content_size().unwrap(), 0);

    // write should not change content size.
    let content = b"abcdef";
    assert!(vmo.write(content, 0).is_ok());
    assert_eq!(vmo.get_content_size().unwrap(), 0);
}

#[test]
fn vmo_zero() {
    let vmo = Vmo::create(16).unwrap();
    let content = b"0123456789abcdef";
    assert!(vmo.write(content, 0).is_ok());
    let mut buf = vec![0u8; 16];
    assert!(vmo.read(&mut buf[..], 0).is_ok());
    assert_eq!(&buf[..], content);

    assert!(vmo.op_range(VmoOp::ZERO, 0, 16).is_ok());
    assert!(vmo.read(&mut buf[..], 0).is_ok());
    assert_eq!(&buf[..], &[0u8; 16]);
}

#[test]
fn vmo_stream_size() {
    let start_size = 1300;
    let vmo = Vmo::create_with_opts(VmoOptions::UNBOUNDED, start_size).unwrap();
    assert_eq!(vmo.get_stream_size().unwrap(), start_size);
    vmo.set_stream_size(0).unwrap();
    assert_eq!(vmo.get_stream_size().unwrap(), 0);

    // write should not change content size.
    let content = b"abcdef";
    assert!(vmo.write(content, 0).is_ok());
    assert_eq!(vmo.get_stream_size().unwrap(), 0);

    // stream size can also grow.
    let mut buf = vec![1; 6];
    vmo.set_stream_size(6).unwrap();
    assert!(vmo.read(&mut buf, 0).is_ok());
    // growing will zero new bytes.
    assert_eq!(buf, vec![0; 6]);
}
