// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Ported from `zircon/kernel/lib/user_copy/tests/kernel.rs` and
//! `zircon/kernel/lib/user_copy/user_copy_tests.cc`.
//!
//! Not ported: the iovec and string view tests, as those are not ported; the test of copies
//! holding a spinlock, as there is no `CopyContext`; and the tests of addresses outside the user
//! address space other than null, as the host process's memory surrounds its address spaces.

use crate::unittest::user_memory::UserMemory;
use crate::unittest::{assert_nonnull, assert_null, assert_true, unwrap_ok};
use crate::user_copy::{UserInOutPtr, UserInPtr, UserOutPtr};
use crate::vm::fault::flag;
use std::mem::MaybeUninit;
use zx_status::Status;

const PAGE_SIZE: usize = page::SIZE;

const TEST_VALUE: u32 = 0xDEADBEEF;

/// Test UserPtr offsets.
#[test]
fn offsets() {
    let base = 0x1000 as *mut u32;

    // UserInPtr offset tests
    let in_ptr = UserInPtr::new(base as *const u32);
    assert_nonnull!(in_ptr);
    assert_eq!(in_ptr.byte_offset(8).as_ptr(), 0x1008 as *const u32);
    assert_eq!(in_ptr.byte_offset(-4).as_ptr(), 0x0ffc as *const u32);
    assert_eq!(in_ptr.element_offset(3).as_ptr(), 0x100c as *const u32);

    let null_in = UserInPtr::<u32>::new(core::ptr::null());
    assert_null!(null_in.byte_offset(8));
    assert_null!(null_in.element_offset(3));

    let def_in = UserInPtr::<u32>::default();
    assert_null!(def_in);

    // UserOutPtr offset tests
    let out_ptr = UserOutPtr::new(base);
    assert_nonnull!(out_ptr);
    assert_eq!(out_ptr.byte_offset(8).as_ptr(), 0x1008 as *mut u32);
    assert_eq!(out_ptr.byte_offset(-4).as_ptr(), 0x0ffc as *mut u32);
    assert_eq!(out_ptr.element_offset(3).as_ptr(), 0x100c as *mut u32);

    let null_out = UserOutPtr::<u32>::new(core::ptr::null_mut());
    assert_null!(null_out.byte_offset(8));
    assert_null!(null_out.element_offset(3));

    let def_out = UserOutPtr::<u32>::default();
    assert_null!(def_out);

    // UserInOutPtr offset tests
    let inout_ptr = UserInOutPtr::new(base);
    assert_nonnull!(inout_ptr);
    assert_eq!(inout_ptr.byte_offset(8).as_ptr(), 0x1008 as *mut u32);
    assert_eq!(inout_ptr.byte_offset(-4).as_ptr(), 0x0ffc as *mut u32);
    assert_eq!(inout_ptr.element_offset(3).as_ptr(), 0x100c as *mut u32);

    let null_inout = UserInOutPtr::<u32>::new(core::ptr::null_mut());
    assert_null!(null_inout.byte_offset(8));
    assert_null!(null_inout.element_offset(3));

    let def_inout = UserInOutPtr::<u32>::default();
    assert_null!(def_inout);
}

/// Test UserPtr default without trait bounds on T.
#[test]
fn default_unconstrained() {
    #[allow(dead_code)]
    struct NoTraits(u8);
    let in_ptr = UserInPtr::<NoTraits>::default();
    assert_null!(in_ptr);
    let out_ptr = UserOutPtr::<NoTraits>::default();
    assert_null!(out_ptr);
    let inout_ptr = UserInOutPtr::<NoTraits>::default();
    assert_null!(inout_ptr);
}

fn test_copy_out(pre_map: bool) {
    let user = UserMemory::create(PAGE_SIZE).unwrap();

    if pre_map {
        unwrap_ok!(user.commit_and_map(0..PAGE_SIZE));
    }

    unwrap_ok!(user.user_out::<u32>().copy_to_user(&TEST_VALUE));

    let mut temp = [0u8; 4];
    unwrap_ok!(user.vmo_read(&mut temp, 0));
    assert_eq!(u32::from_ne_bytes(temp), TEST_VALUE);
}

fn test_copy_in(pre_map: bool) {
    let user = UserMemory::create(PAGE_SIZE).unwrap();

    if pre_map {
        unwrap_ok!(user.commit_and_map(0..PAGE_SIZE));
    }

    unwrap_ok!(user.vmo_write(&TEST_VALUE.to_ne_bytes(), 0));

    let temp = unwrap_ok!(user.user_in::<u32>().read());
    assert_eq!(temp, TEST_VALUE);
}

#[test]
fn pre_map_copy_out() {
    test_copy_out(true)
}

#[test]
fn fault_copy_out() {
    test_copy_out(false)
}

#[test]
fn pre_map_copy_in() {
    test_copy_in(true)
}

#[test]
fn fault_copy_in() {
    test_copy_in(false)
}

/// Test CopyFromUser.
#[test]
fn copy_from_user() {
    let user = UserMemory::create(4096).unwrap();
    unwrap_ok!(user.commit_and_map(0..4096));
    unwrap_ok!(user.vmo_write(&0xDEADBEEF_u32.to_ne_bytes(), 0));

    let in_ptr = UserInPtr::<u32>::new(user.base() as *const u32);
    assert_nonnull!(in_ptr);
    let mut temp = MaybeUninit::uninit();
    let val_ref = unwrap_ok!(in_ptr.copy_from_user(&mut temp));
    assert_eq!(*val_ref, 0xDEADBEEF);
}

/// Test CopySliceFromUser.
#[test]
fn copy_slice_from_user() {
    let user = UserMemory::create(4096).unwrap();
    unwrap_ok!(user.commit_and_map(0..4096));
    let vals = [10u32, 20u32, 30u32];
    let mut bytes = [0u8; 12];
    for i in 0..3 {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&vals[i].to_ne_bytes());
    }
    unwrap_ok!(user.vmo_write(&bytes, 0));

    let in_ptr = UserInPtr::<u32>::new(user.base() as *const u32);
    assert_nonnull!(in_ptr);
    let mut out = [MaybeUninit::uninit(); 3];
    let out_slice = unwrap_ok!(in_ptr.copy_slice_from_user(&mut out));
    assert_eq!(out_slice[0], 10);
    assert_eq!(out_slice[1], 20);
    assert_eq!(out_slice[2], 30);
}

/// Test faults.
#[test]
fn faults() {
    let out_ptr = UserOutPtr::<u32>::new(core::ptr::null_mut());
    assert_null!(out_ptr);
    assert_true!(out_ptr.write(0xDEADBEEF).err() == Some(Status::INVALID_ARGS));

    let in_ptr = UserInPtr::<u32>::new(core::ptr::null());
    assert_null!(in_ptr);
    assert_true!(in_ptr.read().err() == Some(Status::INVALID_ARGS));

    let mut temp = MaybeUninit::uninit();
    assert_true!(in_ptr.copy_from_user(&mut temp).err() == Some(Status::INVALID_ARGS));

    let mut temp_slice = [MaybeUninit::uninit(); 1];
    assert_true!(in_ptr.copy_slice_from_user(&mut temp_slice).err() == Some(Status::INVALID_ARGS));
}

#[test]
fn capture_faults_copy_out_success() {
    let user = UserMemory::create(PAGE_SIZE).unwrap();
    unwrap_ok!(user.commit_and_map(0..PAGE_SIZE));

    unwrap_ok!(user.user_out::<u32>().copy_to_user_capture_faults(&TEST_VALUE));

    let mut temp = [0u8; 4];
    unwrap_ok!(user.vmo_read(&mut temp, 0));
    assert_eq!(u32::from_ne_bytes(temp), TEST_VALUE);
}

#[test]
fn capture_faults_copy_in_success() {
    let user = UserMemory::create(PAGE_SIZE).unwrap();
    unwrap_ok!(user.commit_and_map(0..PAGE_SIZE));

    unwrap_ok!(user.vmo_write(&TEST_VALUE.to_ne_bytes(), 0));

    let mut temp = MaybeUninit::uninit();
    let temp = unwrap_ok!(user.user_in::<u32>().copy_from_user_capture_faults(&mut temp));
    assert_eq!(*temp, TEST_VALUE);
}

#[test]
fn capture_faults_test_capture() {
    let user = UserMemory::create(PAGE_SIZE).unwrap();

    {
        let mut temp = MaybeUninit::uninit();
        let error = user.user_in::<u32>().copy_from_user_capture_faults(&mut temp).unwrap_err();
        assert_eq!(error.status, Status::INVALID_ARGS);

        let fault_info = error.fault_info.unwrap();
        assert_eq!(fault_info.pf_va, user.base());
        assert_eq!(fault_info.pf_flags, flag::NOT_PRESENT);
    }

    {
        let error = user.user_out::<u32>().copy_to_user_capture_faults(&TEST_VALUE).unwrap_err();
        assert_eq!(error.status, Status::INVALID_ARGS);

        let fault_info = error.fault_info.unwrap();
        assert_eq!(fault_info.pf_va, user.base());
        assert_eq!(fault_info.pf_flags, flag::NOT_PRESENT | flag::WRITE);
    }
}

#[test]
fn capture_faults_null() {
    let mut temp = MaybeUninit::uninit();
    let error = UserInPtr::<u32>::new(core::ptr::null())
        .copy_from_user_capture_faults(&mut temp)
        .unwrap_err();
    assert_eq!(error.status, Status::INVALID_ARGS);
    let fault_info = error.fault_info.unwrap();
    assert_eq!(fault_info.pf_va, 0);
    assert_eq!(fault_info.pf_flags, flag::NOT_PRESENT);
}
