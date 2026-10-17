// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ported from `zircon/system/utest/core/stream/stream.cc`.
//!
//! Offsets upstream writes as 4 KiB values (4090, 4096, 4100) are written relative to the page
//! size, so that the tests hold for other page sizes.
//!
//! Upstream's `ASSERT_TRUE(a || b)` checks of racing outcomes are `assert!(a || b)` here, as either
//! outcome is correct.
//!
//! Where upstream serves pager requests on a thread of its own, the test thread serves them and a
//! thread of its own makes the stream calls, as the pager is the test thread's.
//!
//! Not ported, as the kernel lacks what they need:
//! - There is no system resource, so no contiguous or physical VMOs:
//!   `NoStreamFromContiguousOrPhysicalVmo`.
//! - No mappings fault beyond the stream size: `FaultBeyondStreamSizeResizeDownRace`.

use std::sync::Arc;
use zircon_core_tests::userpager::{UserPager, page_size, root_vmar};
use zx::sys::{
    ZX_ERR_ACCESS_DENIED, ZX_ERR_BAD_HANDLE, ZX_ERR_FILE_BIG, ZX_ERR_INVALID_ARGS,
    ZX_ERR_NOT_FOUND, ZX_ERR_OUT_OF_RANGE, ZX_ERR_WRONG_TYPE, ZX_HANDLE_INVALID, ZX_INFO_STREAM,
    ZX_OK, ZX_PROP_STREAM_MODE_APPEND, ZX_PROP_VMO_CONTENT_SIZE, ZX_RIGHT_DUPLICATE,
    ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_INSPECT, ZX_RIGHT_READ, ZX_RIGHT_SET_PROPERTY, ZX_RIGHT_SIGNAL,
    ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT, ZX_RIGHT_WRITE, ZX_STREAM_APPEND, ZX_STREAM_MODE_APPEND,
    ZX_STREAM_MODE_READ, ZX_STREAM_MODE_WRITE, ZX_STREAM_SEEK_ORIGIN_CURRENT,
    ZX_STREAM_SEEK_ORIGIN_END, ZX_STREAM_SEEK_ORIGIN_START, zx_handle_t, zx_info_stream_t,
    zx_iovec_t, zx_off_t, zx_rights_t, zx_status_t,
};

/// This value corresponds to the kernel's VmObjectPaged::ReadWriteInternal write batch limit
/// (kMaxWriteWaitPages).
const MAX_PAGES_BATCH: u64 = 256;

const DEFAULT_STREAM_RIGHTS: zx_rights_t = ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_TRANSFER
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_GET_PROPERTY
    | ZX_RIGHT_SET_PROPERTY
    | ZX_RIGHT_SIGNAL;

const ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz";

fn ps() -> usize {
    page_size() as usize
}

fn vmo_create(size: usize, options: zx::VmoOptions) -> zx::Vmo {
    zx::Vmo::create_with_opts(options, size as u64).unwrap()
}

fn set_content_size(vmo: &zx::Vmo, content_size: u64) -> zx_status_t {
    // SAFETY: The value is a u64.
    unsafe {
        zx::sys::zx_object_set_property(
            vmo.raw_handle(),
            ZX_PROP_VMO_CONTENT_SIZE,
            (&raw const content_size).cast(),
            size_of::<u64>(),
        )
    }
}

fn get_content_size(vmo: &zx::Vmo) -> u64 {
    let mut content_size = 45684651u64;
    // SAFETY: The value is a u64.
    let status = unsafe {
        zx::sys::zx_object_get_property(
            vmo.raw_handle(),
            ZX_PROP_VMO_CONTENT_SIZE,
            (&raw mut content_size).cast(),
            size_of::<u64>(),
        )
    };
    assert_eq!(status, ZX_OK);
    content_size
}

/// The VMO's first page, up to the first zero byte, as `GetData` returns.
fn get_data(vmo: &zx::Vmo) -> String {
    let mut buffer = vec![0u8; ps()];
    vmo.read(&mut buffer, 0).unwrap();
    c_str(&buffer)
}

/// `bytes` up to the first zero byte.
fn c_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec()).unwrap()
}

fn stream_create(options: u32, vmo: zx_handle_t, seek: u64) -> Result<zx::Stream, zx_status_t> {
    let mut raw = ZX_HANDLE_INVALID;
    // SAFETY: `raw` is a handle to write.
    let status = unsafe { zx::sys::zx_stream_create(options, vmo, seek, &mut raw) };
    if status != ZX_OK {
        return Err(status);
    }
    // SAFETY: `raw` is a new handle this owns.
    Ok(unsafe { zx::NullableHandle::from_raw(raw) }.into())
}

fn create_stream(options: u32, vmo: &zx::Vmo, seek: u64) -> Result<zx::Stream, zx_status_t> {
    stream_create(options, vmo.raw_handle(), seek)
}

fn check_rights(stream: &zx::Stream, expected_rights: zx_rights_t, message: &str) {
    assert_eq!(stream.basic_info().unwrap().rights.bits(), expected_rights, "{message}");
}

fn iovec(buffer: *const u8, capacity: usize) -> zx_iovec_t {
    zx_iovec_t { buffer, capacity }
}

fn readv(
    stream: &zx::Stream,
    options: u32,
    vectors: *mut zx_iovec_t,
    count: usize,
    actual: *mut usize,
) -> zx_status_t {
    // SAFETY: The pointers are the test's.
    unsafe { zx::sys::zx_stream_readv(stream.raw_handle(), options, vectors, count, actual) }
}

fn readv_at(
    stream: &zx::Stream,
    options: u32,
    offset: zx_off_t,
    vectors: *mut zx_iovec_t,
    count: usize,
    actual: *mut usize,
) -> zx_status_t {
    // SAFETY: The pointers are the test's.
    unsafe {
        zx::sys::zx_stream_readv_at(stream.raw_handle(), options, offset, vectors, count, actual)
    }
}

fn writev(
    stream: &zx::Stream,
    options: u32,
    vectors: *const zx_iovec_t,
    count: usize,
    actual: *mut usize,
) -> zx_status_t {
    // SAFETY: The pointers are the test's.
    unsafe { zx::sys::zx_stream_writev(stream.raw_handle(), options, vectors, count, actual) }
}

fn writev_at(
    stream: &zx::Stream,
    options: u32,
    offset: zx_off_t,
    vectors: *const zx_iovec_t,
    count: usize,
    actual: *mut usize,
) -> zx_status_t {
    // SAFETY: The pointers are the test's.
    unsafe {
        zx::sys::zx_stream_writev_at(stream.raw_handle(), options, offset, vectors, count, actual)
    }
}

fn stream_seek(
    stream: &zx::Stream,
    whence: u32,
    offset: i64,
    result: *mut zx_off_t,
) -> zx_status_t {
    // SAFETY: `result` is the test's.
    unsafe { zx::sys::zx_stream_seek(stream.raw_handle(), whence, offset, result) }
}

fn stream_info(stream: &zx::Stream) -> zx_info_stream_t {
    let mut info = zx_info_stream_t::default();
    // SAFETY: `info` is a `zx_info_stream_t`.
    let status = unsafe {
        zx::sys::zx_object_get_info(
            stream.raw_handle(),
            ZX_INFO_STREAM,
            (&raw mut info).cast(),
            size_of::<zx_info_stream_t>(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, ZX_OK);
    info
}

fn get_prop_mode_append(stream: &zx::Stream) -> u8 {
    let mut mode_append = 0u8;
    // SAFETY: The value is a u8.
    let status = unsafe {
        zx::sys::zx_object_get_property(
            stream.raw_handle(),
            ZX_PROP_STREAM_MODE_APPEND,
            (&raw mut mode_append).cast(),
            size_of::<u8>(),
        )
    };
    assert_eq!(status, ZX_OK);
    mode_append
}

fn set_prop_mode_append(stream: &zx::Stream, value: bool) -> zx_status_t {
    let mode_append = value as u8;
    // SAFETY: The value is a u8.
    unsafe {
        zx::sys::zx_object_set_property(
            stream.raw_handle(),
            ZX_PROP_STREAM_MODE_APPEND,
            (&raw const mode_append).cast(),
            size_of::<u8>(),
        )
    }
}

/// A pointer to share with a test thread, as C++ lambdas capture them.
#[derive(Clone, Copy)]
struct SendPtr<T>(*mut T);
// SAFETY: The tests share them as upstream does, racing on purpose where upstream does.
unsafe impl<T> Send for SendPtr<T> {}
// SAFETY: As for `Send`.
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> SendPtr<T> {
    /// The pointer. Closures call this rather than taking the field, to capture the `SendPtr`.
    fn get(self) -> *mut T {
        self.0
    }
}

#[test]
fn create() {
    assert_eq!(stream_create(0, ZX_HANDLE_INVALID, 0).unwrap_err(), ZX_ERR_BAD_HANDLE);

    let event = zx::Event::create();
    assert_eq!(stream_create(0, event.raw_handle(), 0).unwrap_err(), ZX_ERR_WRONG_TYPE);

    let vmo = vmo_create(ps() * 4, zx::VmoOptions::empty());
    assert_eq!(set_content_size(&vmo, 0), ZX_OK);

    const _: () = assert!(DEFAULT_STREAM_RIGHTS & ZX_RIGHT_WRITE == 0);
    const _: () = assert!(DEFAULT_STREAM_RIGHTS & ZX_RIGHT_READ == 0);

    assert_eq!(create_stream(-42i32 as u32, &vmo, 0).unwrap_err(), ZX_ERR_INVALID_ARGS);

    let stream = create_stream(0, &vmo, 0).unwrap();
    check_rights(&stream, DEFAULT_STREAM_RIGHTS, "Default");

    let stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 0).unwrap();
    check_rights(&stream, DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ, "ZX_STREAM_MODE_READ");

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    check_rights(&stream, DEFAULT_STREAM_RIGHTS | ZX_RIGHT_WRITE, "ZX_STREAM_MODE_WRITE");

    let stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    check_rights(
        &stream,
        DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ | ZX_RIGHT_WRITE,
        "ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE",
    );

    let stream = create_stream(ZX_STREAM_MODE_APPEND, &vmo, 0).unwrap();
    check_rights(&stream, DEFAULT_STREAM_RIGHTS, "ZX_STREAM_MODE_APPEND");

    let stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_APPEND, &vmo, 0).unwrap();
    check_rights(
        &stream,
        DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ,
        "ZX_STREAM_MODE_READ | ZX_STREAM_MODE_APPEND",
    );

    let stream = create_stream(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, &vmo, 0).unwrap();
    check_rights(
        &stream,
        DEFAULT_STREAM_RIGHTS | ZX_RIGHT_WRITE,
        "ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND",
    );

    let stream =
        create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, &vmo, 0)
            .unwrap();
    check_rights(
        &stream,
        DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ | ZX_RIGHT_WRITE,
        "ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND",
    );

    {
        let read_only = vmo.duplicate_handle(zx::Rights::READ).unwrap();

        let stream = create_stream(0, &read_only, 0).unwrap();
        check_rights(&stream, DEFAULT_STREAM_RIGHTS, "read_only: Default");

        let stream = create_stream(ZX_STREAM_MODE_READ, &read_only, 0).unwrap();
        check_rights(
            &stream,
            DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ,
            "read_only: ZX_STREAM_MODE_READ",
        );

        assert_eq!(
            create_stream(ZX_STREAM_MODE_WRITE, &read_only, 0).unwrap_err(),
            ZX_ERR_ACCESS_DENIED
        );
        assert_eq!(
            create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &read_only, 0).unwrap_err(),
            ZX_ERR_ACCESS_DENIED
        );
    }

    {
        let write_only = vmo.duplicate_handle(zx::Rights::WRITE).unwrap();

        let stream = create_stream(0, &write_only, 0).unwrap();
        check_rights(&stream, DEFAULT_STREAM_RIGHTS, "write_only: Default");

        assert_eq!(
            create_stream(ZX_STREAM_MODE_READ, &write_only, 0).unwrap_err(),
            ZX_ERR_ACCESS_DENIED
        );

        let stream = create_stream(ZX_STREAM_MODE_WRITE, &write_only, 0).unwrap();
        check_rights(
            &stream,
            DEFAULT_STREAM_RIGHTS | ZX_RIGHT_WRITE,
            "write_only: ZX_STREAM_MODE_WRITE",
        );

        assert_eq!(
            create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &write_only, 0).unwrap_err(),
            ZX_ERR_ACCESS_DENIED
        );
    }

    {
        let none = vmo.duplicate_handle(zx::Rights::NONE).unwrap();

        let stream = create_stream(0, &none, 0).unwrap();
        check_rights(&stream, DEFAULT_STREAM_RIGHTS, "none: Default");

        assert_eq!(create_stream(ZX_STREAM_MODE_READ, &none, 0).unwrap_err(), ZX_ERR_ACCESS_DENIED);
        assert_eq!(
            create_stream(ZX_STREAM_MODE_WRITE, &none, 0).unwrap_err(),
            ZX_ERR_ACCESS_DENIED
        );
        assert_eq!(
            create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &none, 0).unwrap_err(),
            ZX_ERR_ACCESS_DENIED
        );
    }
}

#[test]
fn seek() {
    let vmo = vmo_create(ps() * 4, zx::VmoOptions::empty());
    let content_size = 42u64;
    assert_eq!(set_content_size(&vmo, content_size), ZX_OK);

    let mut result: zx_off_t = 81;

    let stream = create_stream(0, &vmo, 0).unwrap();
    assert_eq!(
        stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 0, &mut result),
        ZX_ERR_ACCESS_DENIED
    );

    let stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 9).unwrap();
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &mut result), ZX_OK);
    assert_eq!(result, 9);

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 518).unwrap();
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &mut result), ZX_OK);
    assert_eq!(result, 518);

    let stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

    assert_eq!(stream_seek(&stream, 34893, 12, &mut result), ZX_ERR_INVALID_ARGS);
    assert_eq!(
        stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, -10, &mut result),
        ZX_ERR_INVALID_ARGS
    );
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 10, &mut result), ZX_OK);
    assert_eq!(result, 10);
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 12, &mut result), ZX_OK);
    assert_eq!(result, 12);

    assert_eq!(
        stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, -21, &mut result),
        ZX_ERR_INVALID_ARGS
    );
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 3, &mut result), ZX_OK);
    assert_eq!(result, 15);
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, -15, &mut result), ZX_OK);
    assert_eq!(result, 0);
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, i64::MAX, &mut result), ZX_OK);
    assert_eq!(result, i64::MAX as zx_off_t);

    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 1038, &mut result), ZX_OK);
    assert_eq!(result, i64::MAX as zx_off_t + 1038);
    assert_eq!(
        stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, i64::MAX, &mut result),
        ZX_ERR_INVALID_ARGS
    );

    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_END, 0, &mut result), ZX_OK);
    assert_eq!(result, content_size);
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_END, -11, &mut result), ZX_OK);
    assert_eq!(result, 31);
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_END, -13, &mut result), ZX_OK);
    assert_eq!(result, 29);
    assert_eq!(
        stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_END, -(content_size as i64), &mut result),
        ZX_OK
    );
    assert_eq!(result, 0);
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_END, 24, &mut result), ZX_OK);
    assert_eq!(result, 66);
    assert_eq!(
        stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_END, -1238, &mut result),
        ZX_ERR_INVALID_ARGS
    );

    assert_eq!(set_content_size(&vmo, u64::MAX), ZX_ERR_OUT_OF_RANGE);

    assert_eq!(
        stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_END, i64::MIN, &mut result),
        ZX_ERR_INVALID_ARGS
    );

    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 0, std::ptr::null_mut()), ZX_OK);
}

#[test]
fn read_v() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
    assert_eq!(set_content_size(&vmo, 26), ZX_OK);

    let mut buffer = [0u8; 16];
    let mut vec = iovec(buffer.as_mut_ptr(), buffer.len());
    let mut actual = 42usize;

    let stream = create_stream(0, &vmo, 0).unwrap();
    assert_eq!(readv(&stream, 0, &mut vec, 1, &mut actual), ZX_ERR_ACCESS_DENIED);

    let stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 0).unwrap();
    vec.capacity = 7;
    assert_eq!(readv(&stream, 0, &mut vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 7);
    assert_eq!(c_str(&buffer), "abcdefg");
    buffer.fill(0);

    assert_eq!(readv(&stream, 24098, &mut vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(readv(&stream, 0, std::ptr::null_mut(), 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(readv(&stream, 0, std::ptr::null_mut(), 0, &mut actual), ZX_ERR_INVALID_ARGS);

    vec.capacity = 3;
    assert_eq!(readv(&stream, 0, &mut vec, 1, std::ptr::null_mut()), ZX_OK);
    assert_eq!(c_str(&buffer), "hij");
    buffer.fill(0);

    vec.buffer = std::ptr::null();
    vec.capacity = 7;
    assert_eq!(readv(&stream, 0, &mut vec, 1, &mut actual), ZX_ERR_NOT_FOUND);
    vec.buffer = buffer.as_mut_ptr();

    const VECTOR_COUNT: usize = 7;
    let mut multivec = [iovec(buffer.as_mut_ptr(), i64::MAX as usize); VECTOR_COUNT];

    assert_eq!(
        readv(&stream, 0, multivec.as_mut_ptr(), VECTOR_COUNT, &mut actual),
        ZX_ERR_INVALID_ARGS
    );

    vec.capacity = buffer.len();
    assert_eq!(readv(&stream, 0, &mut vec, 1, &mut actual), ZX_OK);
    buffer.fill(0);

    assert_eq!(set_content_size(&vmo, 6), ZX_OK);

    vec.capacity = 3;
    actual = 42;
    assert_eq!(readv(&stream, 0, &mut vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 0);
    buffer.fill(0);

    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 0, std::ptr::null_mut()), ZX_OK);
    vec.capacity = 12;
    actual = 42;
    assert_eq!(readv(&stream, 0, &mut vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 6);
    assert_eq!(c_str(&buffer), "abcdef");
    buffer.fill(0);

    assert_eq!(set_content_size(&vmo, 26), ZX_OK);

    for (i, vec) in multivec.iter_mut().enumerate() {
        // SAFETY: `i` is within `buffer`.
        vec.buffer = unsafe { buffer.as_mut_ptr().add(i) };
        vec.capacity = 1;
    }

    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 0, std::ptr::null_mut()), ZX_OK);
    assert_eq!(readv(&stream, 0, multivec.as_mut_ptr(), VECTOR_COUNT, &mut actual), ZX_OK);
    assert_eq!(actual, VECTOR_COUNT);
    assert_eq!(c_str(&buffer), "abcdef");
}

#[test]
fn write_v() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
    assert_eq!(set_content_size(&vmo, 26), ZX_OK);

    let buffer = *b"0123456789ABCDEF\0";
    let mut vec = iovec(buffer.as_ptr(), buffer.len());
    let mut actual = 42usize;

    let stream = create_stream(0, &vmo, 0).unwrap();
    assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_ERR_ACCESS_DENIED);

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    vec.capacity = 7;
    assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 7);
    assert_eq!(get_data(&vmo), "0123456hijklmnopqrstuvwxyz");
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();

    assert_eq!(writev(&stream, 24098, &vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev(&stream, 0, std::ptr::null(), 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev(&stream, 0, std::ptr::null(), 0, &mut actual), ZX_ERR_INVALID_ARGS);

    vec.capacity = 3;
    assert_eq!(writev(&stream, 0, &vec, 1, std::ptr::null_mut()), ZX_OK);
    assert_eq!(get_data(&vmo), "abcdefg012klmnopqrstuvwxyz");
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();

    vec.buffer = std::ptr::null();
    vec.capacity = 7;
    assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_ERR_NOT_FOUND);

    const VECTOR_COUNT: usize = 7;
    let mut multivec = [iovec(buffer.as_ptr(), i64::MAX as usize); VECTOR_COUNT];

    assert_eq!(
        writev(&stream, 0, multivec.as_ptr(), VECTOR_COUNT, &mut actual),
        ZX_ERR_INVALID_ARGS
    );

    for i in 0..VECTOR_COUNT {
        multivec[VECTOR_COUNT - i - 1] = iovec(buffer[i..].as_ptr(), 1);
    }

    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 0, std::ptr::null_mut()), ZX_OK);
    assert_eq!(writev(&stream, 0, multivec.as_ptr(), VECTOR_COUNT, &mut actual), ZX_OK);
    assert_eq!(actual, VECTOR_COUNT);
    assert_eq!(get_data(&vmo), "6543210hijklmnopqrstuvwxyz");
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
}

/// Writes 10 bytes at a time from the start of a one page VMO until less than 10 bytes remain,
/// then writes the rest, as `WriteExtendsContentSize` and `WriteDoesntExtendVMOSize` do with 4 KiB
/// pages (409 writes, then 6 bytes).
fn write_page_in_tens(stream: &zx::Stream, vmo: &zx::Vmo, vec: &zx_iovec_t) -> usize {
    let mut actual = 42usize;
    let mut i = 1;
    while i * 10 < ps() {
        assert_eq!(writev(stream, 0, vec, 1, &mut actual), ZX_OK);
        assert_eq!(actual, 10);
        i += 1;
    }
    let full = (i - 1) * 10;
    assert_eq!(get_content_size(vmo), full as u64);

    actual = 9823;
    assert_eq!(writev(stream, 0, vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, ps() - full);
    assert_eq!(get_content_size(vmo), ps() as u64);
    full
}

#[test]
fn write_extends_content_size() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
    assert_eq!(set_content_size(&vmo, 3), ZX_OK);

    let buffer = *b"0123456789ABCDEF\0";
    let mut vec = iovec(buffer.as_ptr(), buffer.len());
    let mut actual = 42usize;

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    vec.capacity = 7;
    assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 7);
    assert_eq!(get_data(&vmo), "0123456");
    assert_eq!(get_content_size(&vmo), 7);
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();

    vec.capacity = 2;
    assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 2);
    assert_eq!(get_data(&vmo), "abcdefg01jklmnopqrstuvwxyz");
    assert_eq!(get_content_size(&vmo), 9);
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();

    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 0, std::ptr::null_mut()), ZX_OK);

    vec.capacity = 10;
    let full = write_page_in_tens(&stream, &vmo, &vec);

    let mut scratch = [0u8; 17];
    vmo.read(&mut scratch[..ps() - full], full as u64).unwrap();
    assert_eq!(c_str(&scratch), &"0123456789"[..ps() - full]);

    actual = 9823;
    assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_ERR_OUT_OF_RANGE);
    assert_eq!(get_content_size(&vmo), ps() as u64);
}

#[test]
fn write_doesnt_extend_vmo_size() {
    let vmo = vmo_create(ps(), zx::VmoOptions::RESIZABLE);
    assert_eq!(set_content_size(&vmo, 0), ZX_OK);

    let buffer = *b"0123456789ABCDEF\0";
    let mut vec = iovec(buffer.as_ptr(), buffer.len());

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    vec.capacity = 10;
    write_page_in_tens(&stream, &vmo, &vec);

    // Assert that there is no remaining capacity in the underlying VMO by trying
    // to write one byte.
    vec.capacity = 1;
    assert_eq!(writev(&stream, 0, &vec, 1, std::ptr::null_mut()), ZX_ERR_OUT_OF_RANGE);

    assert_eq!(vmo.get_size().unwrap(), ps() as u64);

    vec.capacity = u64::MAX as usize;
    let mut actual = 5423usize;
    assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_ERR_FILE_BIG);

    assert_eq!(vmo.get_size().unwrap(), ps() as u64);
}

#[test]
fn read_v_at() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
    assert_eq!(set_content_size(&vmo, 26), ZX_OK);

    let mut buffer = [0u8; 16];
    let mut vec = iovec(buffer.as_mut_ptr(), buffer.len());
    let mut actual = 42usize;

    let stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 0).unwrap();
    vec.capacity = 7;
    assert_eq!(readv_at(&stream, 0, 24, &mut vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 2);
    assert_eq!(c_str(&buffer), "yz");
    buffer.fill(0);

    let mut seek_result: zx_off_t = 39;
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &mut seek_result), ZX_OK);
    assert_eq!(seek_result, 0);

    assert_eq!(readv_at(&stream, 0, 36, &mut vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 0);
    assert_eq!(c_str(&buffer), "");
    buffer.fill(0);

    assert_eq!(readv_at(&stream, 0, 3645651, &mut vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 0);
    assert_eq!(c_str(&buffer), "");
}

#[test]
fn write_v_at() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
    assert_eq!(set_content_size(&vmo, 26), ZX_OK);

    let buffer = *b"0123456789ABCDEF\0";
    let mut vec = iovec(buffer.as_ptr(), buffer.len());
    let mut actual = 42usize;

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    vec.capacity = 3;
    assert_eq!(writev_at(&stream, 0, 7, &vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 3);
    assert_eq!(get_data(&vmo), "abcdefg012klmnopqrstuvwxyz");
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();

    let mut seek_result: zx_off_t = 39;
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &mut seek_result), ZX_OK);
    assert_eq!(seek_result, 0);

    vec.capacity = 10;
    actual = 9823;
    assert_eq!(writev_at(&stream, 0, ps() as u64 + 4, &vec, 1, &mut actual), ZX_ERR_OUT_OF_RANGE);

    let vmo = vmo_create(ps(), zx::VmoOptions::RESIZABLE);
    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

    vec.capacity = 10;
    actual = 9823;
    assert_eq!(writev_at(&stream, 0, ps() as u64 - 6, &vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 6);
    assert_eq!(get_content_size(&vmo), ps() as u64);

    assert_eq!(vmo.get_size().unwrap(), ps() as u64);

    vec.capacity = u64::MAX as usize;
    actual = 5423;
    assert_eq!(writev_at(&stream, 0, 5414, &vec, 1, &mut actual), ZX_ERR_FILE_BIG);

    // did not change the size of the VMO
    assert_eq!(vmo.get_size().unwrap(), ps() as u64);
    assert_eq!(get_content_size(&vmo), ps() as u64);

    let bad_vec = iovec(std::ptr::null(), 42);

    actual = 5423;
    assert_ne!(writev_at(&stream, 0, 5000, &bad_vec, 1, &mut actual), ZX_OK);
    assert_eq!(get_content_size(&vmo), ps() as u64);
}

#[test]
fn read_vector_alias() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
    assert_eq!(set_content_size(&vmo, 26), ZX_OK);

    const VECTOR_COUNT: usize = 7;
    let mut multivec = [iovec(std::ptr::null(), 0); VECTOR_COUNT];
    let alias = multivec.as_mut_ptr().cast::<u8>();
    for vec in multivec.iter_mut() {
        // Notice the alias.
        *vec = iovec(alias, size_of::<[zx_iovec_t; VECTOR_COUNT]>());
    }

    let stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 0).unwrap();
    let mut actual = 42usize;
    assert_eq!(readv(&stream, 0, multivec.as_mut_ptr(), VECTOR_COUNT, &mut actual), ZX_OK);
    assert_eq!(actual, 26);
}

/// Checks the stream's info, as the `zx_info_stream_t` blocks in upstream's tests do.
fn check_info(stream: &zx::Stream, options: u32, seek: u64, content_size: u64) {
    let info = stream_info(stream);
    assert_eq!(info.options, options);
    assert_eq!(info.seek, seek);
    assert_eq!(info.content_size, content_size);
}

/// Appends with `append` until the page is full, as `Append` and
/// `WriteVectorWithStreamInAppendMode` do.
fn append_until_full(
    stream: &zx::Stream,
    vmo: &zx::Vmo,
    mode: u32,
    append: impl Fn(&zx_iovec_t, *mut usize) -> zx_status_t,
) {
    let buffer = *b"0123456789ABCDEF\0";
    let mut vec = iovec(buffer.as_ptr(), buffer.len());

    check_info(stream, mode, 0, 26);

    vec.capacity = 7;
    let mut actual = 42usize;
    assert_eq!(append(&vec, &mut actual), ZX_OK);
    assert_eq!(actual, 7);
    assert_eq!(get_data(vmo), "abcdefghijklmnopqrstuvwxyz0123456");

    {
        check_info(stream, mode, 33, 33);

        vec.capacity = 26;
        let mut size = 33;
        while size + vec.capacity < ps() {
            assert_eq!(append(&vec, &mut actual), ZX_OK);
            assert_eq!(actual, vec.capacity);
            size += vec.capacity;
        }
    }

    {
        let info = stream_info(stream);
        assert!(info.content_size < ps() as u64);

        assert_eq!(append(&vec, &mut actual), ZX_OK);
        assert_eq!(actual as u64, ps() as u64 - info.content_size);
    }

    assert_eq!(append(&vec, &mut actual), ZX_ERR_OUT_OF_RANGE);

    vec.capacity = u64::MAX as usize;
    assert_eq!(append(&vec, &mut actual), ZX_ERR_OUT_OF_RANGE);
}

#[test]
fn append() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
    assert_eq!(set_content_size(&vmo, 26), ZX_OK);

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    append_until_full(&stream, &vmo, ZX_STREAM_MODE_WRITE, |vec, actual| {
        writev(&stream, ZX_STREAM_APPEND, vec, 1, actual)
    });
}

#[test]
fn write_vector_with_stream_in_append_mode() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    vmo.write(ALPHABET.as_bytes(), 0).unwrap();
    assert_eq!(set_content_size(&vmo, 26), ZX_OK);

    let stream = create_stream(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, &vmo, 0).unwrap();
    append_until_full(
        &stream,
        &vmo,
        ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND,
        |vec, actual| writev(&stream, 0, vec, 1, actual),
    );
}

#[test]
fn property_mode_append() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    assert_eq!(set_content_size(&vmo, 0), ZX_OK);

    let buffer = *b"0123456789ABCDEF\0";
    let vec = iovec(buffer.as_ptr(), 16);

    // Create the stream not in append mode.
    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    assert_eq!(writev(&stream, 0, &vec, 1, std::ptr::null_mut()), ZX_OK);

    {
        let info = stream_info(&stream);
        assert_eq!(info.options & ZX_STREAM_MODE_APPEND, 0);
        assert_eq!(info.seek, 16);
        assert_eq!(info.content_size, 16);
        assert_eq!(get_prop_mode_append(&stream), 0);
    }

    // Switch the stream to append mode.
    assert_eq!(set_prop_mode_append(&stream, true), ZX_OK);
    {
        let info = stream_info(&stream);
        assert_ne!(info.options & ZX_STREAM_MODE_APPEND, 0);
        assert_eq!(get_prop_mode_append(&stream), 1);
    }
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 10, std::ptr::null_mut()), ZX_OK);
    assert_eq!(writev(&stream, 0, &vec, 1, std::ptr::null_mut()), ZX_OK);
    assert_eq!(get_data(&vmo), "0123456789ABCDEF0123456789ABCDEF");

    // Take the stream out of append mode.
    assert_eq!(set_prop_mode_append(&stream, false), ZX_OK);
    {
        let info = stream_info(&stream);
        assert_eq!(info.options & ZX_STREAM_MODE_APPEND, 0);
        // The previous write appended to the stream despite the seek offset not being at the end
        // of the stream.
        assert_eq!(info.seek, 32);
        assert_eq!(info.content_size, 32);
        assert_eq!(get_prop_mode_append(&stream), 0);
    }
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_START, 10, std::ptr::null_mut()), ZX_OK);
    assert_eq!(writev(&stream, 0, &vec, 1, std::ptr::null_mut()), ZX_OK);
    assert_eq!(get_data(&vmo), "01234567890123456789ABCDEFABCDEF");
}

#[test]
fn append_with_multiple_threads() {
    // THREAD_COUNT threads collectively write the numbers 0 to BUFFER_SIZE-1 to the vmo.
    const THREAD_COUNT: usize = 4;
    const BUFFER_SIZE: usize = 256;
    const ITERATION_COUNT: usize = BUFFER_SIZE / THREAD_COUNT;
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    assert_eq!(set_content_size(&vmo, 0), ZX_OK);

    let buffer: Vec<u8> = (0..BUFFER_SIZE).map(|i| i as u8).collect();

    std::thread::scope(|scope| {
        for thread in 0..THREAD_COUNT {
            let vmo = &vmo;
            let buffer = &buffer;
            scope.spawn(move || {
                let stream =
                    create_stream(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, vmo, 0).unwrap();
                for i in 0..ITERATION_COUNT {
                    let vec = iovec(&buffer[thread * ITERATION_COUNT + i], 1);
                    assert_eq!(writev(&stream, 0, &vec, 1, std::ptr::null_mut()), ZX_OK);
                }
            });
        }
    });

    // With several threads simultaneously appending, the data is likely out of order but none of
    // the appends should have overwritten each other.
    let mut vmo_data = vec![0u8; BUFFER_SIZE];
    vmo.read(&mut vmo_data, 0).unwrap();
    vmo_data.sort();
    assert_eq!(vmo_data, buffer);
}

#[test]
fn extend_fills_with_zeros() {
    const PAGE_COUNT: usize = 6;
    let vmo = vmo_create(ps() * PAGE_COUNT, zx::VmoOptions::empty());
    assert_eq!(set_content_size(&vmo, 0), ZX_OK);

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

    let mut scratch = vec![b'x'; ps()];

    for i in 0..PAGE_COUNT {
        vmo.write(&scratch, (ps() * i) as u64).unwrap();
    }

    let buffer = *b"0123456789ABCDEF\0";
    let vec = iovec(buffer.as_ptr(), 4);

    let mut actual = 0usize;
    assert_eq!(writev_at(&stream, 0, (ps() * 2 - 2) as u64, &vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 4);

    scratch.fill(b'a');
    vmo.read(&mut scratch, 0).unwrap();

    for (i, &b) in scratch.iter().enumerate() {
        assert_eq!(b, 0, "The {i} byte should be zero.");
    }

    scratch.fill(b'a');
    vmo.read(&mut scratch, ps() as u64).unwrap();

    for (i, &b) in scratch[..ps() - 2].iter().enumerate() {
        assert_eq!(b, 0, "The {i} byte of the second page should be zero.");
    }

    assert_eq!(scratch[ps() - 2], b'0');
    assert_eq!(scratch[ps() - 1], b'1');

    scratch.fill(b'a');
    vmo.read(&mut scratch, (ps() * 2) as u64).unwrap();

    assert_eq!(scratch[0], b'2');
    assert_eq!(scratch[1], b'3');
    assert_eq!(scratch[2], b'x');
    assert_eq!(scratch[3], b'x');

    assert_eq!(
        stream_seek(
            &stream,
            ZX_STREAM_SEEK_ORIGIN_START,
            (ps() * 5 - 2) as i64,
            std::ptr::null_mut()
        ),
        ZX_OK
    );

    actual = 0;
    assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 4);

    scratch.fill(b'a');
    vmo.read(&mut scratch, (ps() * 2) as u64).unwrap();

    assert_eq!(scratch[0], b'2');
    assert_eq!(scratch[1], b'3');
    assert_eq!(scratch[2], 0);
    assert_eq!(scratch[3], 0);

    scratch.fill(b'a');
    vmo.read(&mut scratch, (ps() * 3) as u64).unwrap();

    for (i, &b) in scratch.iter().enumerate() {
        assert_eq!(b, 0, "The {i} byte of the third page should be zero.");
    }

    scratch.fill(b'a');
    vmo.read(&mut scratch, (ps() * 4) as u64).unwrap();

    for (i, &b) in scratch[..ps() - 2].iter().enumerate() {
        assert_eq!(b, 0, "The {i} byte of the fourth page should be zero.");
    }

    assert_eq!(scratch[ps() - 2], b'0');
    assert_eq!(scratch[ps() - 1], b'1');

    scratch.fill(b'a');
    vmo.read(&mut scratch, (ps() * 5) as u64).unwrap();

    assert_eq!(scratch[0], b'2');
    assert_eq!(scratch[1], b'3');
    assert_eq!(scratch[2], b'x');
    assert_eq!(scratch[3], b'x');
}

#[test]
fn read_shrink_race() {
    const NUM_ITERATIONS: usize = 10;

    const INITIAL_VMO_SIZE: usize = 80;
    const TRUNCATE_TO_SIZE: u64 = 0;

    let initial_vmo_num_pages = INITIAL_VMO_SIZE.div_ceil(ps()) as u64;

    for _ in 0..NUM_ITERATIONS {
        let mut pager = UserPager::initialized();

        let vmo = pager
            .create_vmo_with_options(initial_vmo_num_pages, zx::VmoOptions::RESIZABLE)
            .unwrap();

        let stream = create_stream(ZX_STREAM_MODE_READ, vmo.vmo(), 0).unwrap();

        std::thread::scope(|scope| {
            // Create a read that intersects with the truncate.
            let read_thread = scope.spawn(|| {
                let mut buffer = [0u8; 16];
                let mut vec = iovec(buffer.as_mut_ptr(), buffer.len());

                let mut actual = 42usize;
                assert_eq!(readv(&stream, 0, &mut vec, 1, &mut actual), ZX_OK);

                // The read should have happened either before or after the set size, so either
                // nothing or everything should've been read.
                assert!(actual == 0 || actual == buffer.len(), "actual: {actual}");
            });

            let set_size_thread = scope.spawn(|| {
                vmo.vmo().set_size(TRUNCATE_TO_SIZE).unwrap();
            });

            // Start a page fault handler to respond to read requests.
            vmo.set_page_fault_supply_limit(1);
            assert!(pager.start_tagged_page_fault_handler());

            set_size_thread.join().unwrap();
            read_thread.join().unwrap();
        });

        // The set size must now be complete.
        assert_eq!(get_content_size(vmo.vmo()), TRUNCATE_TO_SIZE);

        // Reads should be okay and return nothing.
        let mut buffer = [0u8; 16];
        let mut vec = iovec(buffer.as_mut_ptr(), buffer.len());
        let mut actual = 42usize;
        assert_eq!(readv(&stream, 0, &mut vec, 1, &mut actual), ZX_OK);
        assert_eq!(actual, 0);
    }
}

#[test]
fn write_shrink_race() {
    const NUM_ITERATIONS: usize = 50;

    let initial_vmo_size = ps() + 8;
    let initial_vmo_num_pages = initial_vmo_size.div_ceil(ps()) as u64;
    let truncate_to_size = ps() as u64;

    for _ in 0..NUM_ITERATIONS {
        let mut pager = UserPager::initialized();

        let vmo = pager.create_unbounded_vmo(0, zx::VmoOptions::empty()).unwrap();

        let stream = create_stream(ZX_STREAM_MODE_WRITE, vmo.vmo(), 0).unwrap();

        assert!(pager.supply_pages(&vmo, 0, initial_vmo_num_pages));

        std::thread::scope(|scope| {
            // Create a write that intersects with the truncate.
            let boundary_write_thread = scope.spawn(|| {
                let buffer = [0u8; 16];
                assert!(buffer.len() <= initial_vmo_size);
                let vec = iovec(buffer.as_ptr(), buffer.len());

                // Attempt to write the last |buffer.len()| bytes.
                let offset = (initial_vmo_size - buffer.len()) as u64;
                let mut actual = 42usize;
                assert_eq!(writev_at(&stream, 0, offset, &vec, 1, &mut actual), ZX_OK);
                assert_eq!(actual, buffer.len());
            });

            // Create a write that should always complete, regardless of truncation.
            let full_write_thread = scope.spawn(|| {
                let buffer = [0u8; 16];
                assert!(buffer.len() <= initial_vmo_size);
                let vec = iovec(buffer.as_ptr(), buffer.len());

                // Attempt to write the first |buffer.len()| bytes.
                let mut actual = 42usize;
                assert_eq!(writev_at(&stream, 0, 0, &vec, 1, &mut actual), ZX_OK);
                assert_eq!(actual, buffer.len());
            });

            // Simultaneously try to truncate.
            let truncate_thread = scope.spawn(|| {
                vmo.vmo().set_stream_size(truncate_to_size).unwrap();
            });

            boundary_write_thread.join().unwrap();
            full_write_thread.join().unwrap();
            truncate_thread.join().unwrap();
        });

        // The set size must now be complete.
        // The size will either be |truncate_to_size| if the truncate happened last or
        // |initial_vmo_size| if the write happened last.
        let content_size = get_content_size(vmo.vmo());
        assert!(
            content_size == initial_vmo_size as u64 || content_size == truncate_to_size,
            "content_size: {content_size}"
        );
    }
}

#[test]
fn read_write_shrink_race() {
    const NUM_ITERATIONS: usize = 500;

    let initial_vmo_size = 8 * ps() + 8;
    let initial_vmo_num_pages = initial_vmo_size.div_ceil(ps()) as u64;
    let truncate_to_size = ps() as u64;

    for _ in 0..NUM_ITERATIONS {
        let mut pager = UserPager::initialized();

        let vmo = pager
            .create_vmo_with_options(initial_vmo_num_pages, zx::VmoOptions::RESIZABLE)
            .unwrap();

        let stream =
            create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo.vmo(), 0).unwrap();

        assert!(pager.supply_pages(&vmo, 0, initial_vmo_num_pages));

        std::thread::scope(|scope| {
            // Create a write that intersects with the truncate.
            let write_thread = scope.spawn(|| {
                let buffer = [0u8; 16];
                assert!(buffer.len() <= initial_vmo_size);
                let vec = iovec(buffer.as_ptr(), buffer.len());

                // Attempt to write the last |buffer.len()| bytes.
                let offset = (initial_vmo_size - buffer.len()) as u64;
                let mut actual = 42usize;
                assert_eq!(writev_at(&stream, 0, offset, &vec, 1, &mut actual), ZX_OK);
                assert_eq!(actual, buffer.len());
            });

            // Simultaneously try to truncate.
            let truncate_thread = scope.spawn(|| {
                vmo.vmo().set_stream_size(truncate_to_size).unwrap();
            });

            // Create a read that intersects with the truncate.
            let read_thread = scope.spawn(|| {
                let mut buffer = vec![0u8; initial_vmo_size];
                let mut vec = iovec(buffer.as_mut_ptr(), buffer.len());

                let mut actual = 42usize;
                assert_eq!(readv_at(&stream, 0, 0, &mut vec, 1, &mut actual), ZX_OK);
                // If the write happens after the truncate, the read may see a content size in the
                // range [truncate_to_size, initial_vmo_size] because of a partial expanding write
                // updating content size as it progresses.
                assert!(
                    actual as u64 >= truncate_to_size || actual <= initial_vmo_size,
                    "actual: {actual}"
                );
            });

            write_thread.join().unwrap();
            truncate_thread.join().unwrap();
            read_thread.join().unwrap();
        });

        // The set size must now be complete.
        // The size will either be |truncate_to_size| if the truncate happened last or
        // |initial_vmo_size| if the write happened last.
        let content_size = get_content_size(vmo.vmo());
        assert!(
            content_size == initial_vmo_size as u64 || content_size == truncate_to_size,
            "content_size: {content_size}"
        );
    }
}

// Regression test for https://fxbug.dev/42176351. Writing to an offset that requires expansion
// should not result in an overflow when computing the new required VMO size.
#[test]
fn expand_overflow() {
    let vmo = vmo_create(ps(), zx::VmoOptions::RESIZABLE);

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

    let buffer = *b"AAAA";
    let vec = iovec(buffer.as_ptr(), 4);

    let mut actual = 0usize;
    // This write will require a content size of 0xfffffffffffffffc, which when rounded up to the
    // page boundary to compute the VMO size will overflow. So content expansion should fail.
    assert_eq!(
        writev_at(&stream, 0, 0xfffffffffffffff8, &vec, 1, &mut actual),
        ZX_ERR_OUT_OF_RANGE
    );
    assert_eq!(actual, 0);

    // Verify the VMO and content sizes.
    assert_eq!(vmo.get_size().unwrap(), ps() as u64);
    assert_eq!(get_content_size(&vmo), ps() as u64);

    // Verify that a subsequent resize succeeds.
    vmo.set_size(2 * ps() as u64).unwrap();
    assert_eq!(vmo.get_size().unwrap(), 2 * ps() as u64);
    assert_eq!(get_content_size(&vmo), 2 * ps() as u64);
}

// Tests that content size is updated as soon as bytes are committed to the VMO.
#[test]
fn content_size_updated_on_partial_write() {
    const NUM_PAGES_TO_WRITE: u64 = MAX_PAGES_BATCH * 3;

    let mut pager = UserPager::initialized();

    let vmo = pager.create_unbounded_vmo(0, zx::VmoOptions::TRAP_DIRTY).unwrap();

    let stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo.vmo(), 0).unwrap();

    std::thread::scope(|scope| {
        let write_thread = scope.spawn(|| {
            let buffer = vec![b'a'; NUM_PAGES_TO_WRITE as usize * ps()];
            let vec = iovec(buffer.as_ptr(), buffer.len());
            let mut actual = 0usize;
            assert_eq!(writev(&stream, 0, &vec, 1, &mut actual), ZX_OK);

            assert_eq!(actual, buffer.len());
        });

        let mut page_num = 0;
        while page_num < NUM_PAGES_TO_WRITE {
            let num_pages_to_dirty = MAX_PAGES_BATCH.min(NUM_PAGES_TO_WRITE - page_num);

            assert!(pager.wait_for_page_dirty(
                &vmo,
                page_num,
                num_pages_to_dirty,
                zx::MonotonicInstant::INFINITE
            ));
            assert_eq!(get_content_size(vmo.vmo()), page_num * ps() as u64);
            assert!(pager.dirty_pages(&vmo, page_num, num_pages_to_dirty));
            page_num += MAX_PAGES_BATCH;
        }

        write_thread.join().unwrap();
    });
}

/// Races a vector operation with a thread changing the vector's capacity, as the
/// `Race*ResizeVec*` tests do.
fn race_resize_vec(
    initial_capacity: usize,
    resize_capacity: usize,
    buffer: *const u8,
    op: impl Fn(*mut zx_iovec_t) -> usize + Sync,
    check: impl Fn(usize) + Sync,
) {
    let mut vec = iovec(buffer, initial_capacity);
    let vec_ptr = SendPtr(&raw mut vec);
    std::thread::scope(|scope| {
        let op_thread = scope.spawn(|| check(op(vec_ptr.get())));

        let resize_thread = scope.spawn(move || {
            // SAFETY: The race on the capacity is the test, as upstream.
            unsafe {
                std::ptr::write_volatile(&raw mut (*vec_ptr.get()).capacity, resize_capacity)
            };
        });

        op_thread.join().unwrap();
        resize_thread.join().unwrap();
    });
}

// Tests that resizing a `zx_iovec_t` capacity smaller while a read is using it does not fail.
#[test]
fn race_read_resize_vec_smaller() {
    const NUM_ITERATIONS: usize = 50;
    const INITIAL_VEC_SIZE: usize = 26;
    const RESIZE_VEC_SIZE: usize = 10;
    const INITIAL_BUFFER_CHAR: u8 = b'!';

    for _ in 0..NUM_ITERATIONS {
        let vmo = vmo_create(ps(), zx::VmoOptions::empty());
        vmo.write(ALPHABET.as_bytes(), 0).unwrap();

        let mut buffer = vec![INITIAL_BUFFER_CHAR; INITIAL_VEC_SIZE];
        let buffer_ptr = SendPtr(buffer.as_mut_ptr());

        let stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 0).unwrap();

        race_resize_vec(
            INITIAL_VEC_SIZE,
            RESIZE_VEC_SIZE,
            buffer.as_ptr(),
            |vec| {
                let mut actual = 42usize;
                assert_eq!(readv(&stream, 0, vec, 1, &mut actual), ZX_OK);
                actual
            },
            |actual| {
                assert!(
                    actual == INITIAL_VEC_SIZE || actual == RESIZE_VEC_SIZE,
                    "actual: {actual}"
                );
                // SAFETY: The read is done.
                let buffer =
                    unsafe { std::slice::from_raw_parts(buffer_ptr.get(), INITIAL_VEC_SIZE) };

                if actual == RESIZE_VEC_SIZE {
                    let spliced = format!(
                        "{}{}",
                        &ALPHABET[..RESIZE_VEC_SIZE],
                        "!".repeat(INITIAL_VEC_SIZE - RESIZE_VEC_SIZE)
                    );

                    assert_eq!(c_str(buffer), spliced);
                } else {
                    assert_eq!(get_data(&vmo), ALPHABET);
                }
            },
        );
    }
}

// Tests that resizing a `zx_iovec_t` capacity smaller while a write is using it does not fail.
#[test]
fn race_write_resize_vec_smaller() {
    const NUM_ITERATIONS: usize = 50;
    const RESIZE_VEC_SIZE: usize = 10;

    for _ in 0..NUM_ITERATIONS {
        let vmo = vmo_create(ps(), zx::VmoOptions::empty());
        vmo.write(ALPHABET.as_bytes(), 0).unwrap();

        let buffer = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

        let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

        race_resize_vec(
            buffer.len(),
            RESIZE_VEC_SIZE,
            buffer.as_ptr(),
            |vec| {
                let mut actual = 42usize;
                assert_eq!(writev(&stream, 0, vec, 1, &mut actual), ZX_OK);
                actual
            },
            |actual| {
                assert!(actual == buffer.len() || actual == RESIZE_VEC_SIZE, "actual: {actual}");

                if actual == RESIZE_VEC_SIZE {
                    let spliced =
                        format!("{}{}", &buffer[..RESIZE_VEC_SIZE], &ALPHABET[RESIZE_VEC_SIZE..]);

                    assert_eq!(get_data(&vmo), spliced);
                } else {
                    assert_eq!(get_data(&vmo), buffer);
                }
            },
        );
    }
}

// Tests that resizing a `zx_iovec_t` capacity larger while a read is using it does not fail.
#[test]
fn race_read_resize_vec_larger() {
    const NUM_ITERATIONS: usize = 50;
    const INITIAL_VEC_SIZE: usize = 10;
    const RESIZE_VEC_SIZE: usize = 26;
    const INITIAL_BUFFER_CHAR: u8 = b'!';

    for _ in 0..NUM_ITERATIONS {
        let vmo = vmo_create(ps(), zx::VmoOptions::empty());
        vmo.write(ALPHABET.as_bytes(), 0).unwrap();

        let mut buffer = vec![INITIAL_BUFFER_CHAR; RESIZE_VEC_SIZE];
        let buffer_ptr = SendPtr(buffer.as_mut_ptr());

        let stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 0).unwrap();

        race_resize_vec(
            INITIAL_VEC_SIZE,
            RESIZE_VEC_SIZE,
            buffer.as_ptr(),
            |vec| {
                let mut actual = 42usize;
                assert_eq!(readv(&stream, 0, vec, 1, &mut actual), ZX_OK);
                actual
            },
            |actual| {
                assert!(
                    actual == INITIAL_VEC_SIZE || actual == RESIZE_VEC_SIZE,
                    "actual: {actual}"
                );
                // SAFETY: The read is done.
                let buffer =
                    unsafe { std::slice::from_raw_parts(buffer_ptr.get(), RESIZE_VEC_SIZE) };

                if actual == RESIZE_VEC_SIZE {
                    assert_eq!(c_str(buffer), ALPHABET);
                } else {
                    let spliced = format!(
                        "{}{}",
                        &ALPHABET[..INITIAL_VEC_SIZE],
                        "!".repeat(RESIZE_VEC_SIZE - INITIAL_VEC_SIZE)
                    );

                    assert_eq!(c_str(buffer), spliced);
                }
            },
        );
    }
}

// Tests that resizing a `zx_iovec_t` capacity larger while a write is using it does not fail.
#[test]
fn race_write_resize_vec_larger() {
    const NUM_ITERATIONS: usize = 50;
    const INITIAL_VEC_SIZE: usize = 10;

    for _ in 0..NUM_ITERATIONS {
        let vmo = vmo_create(ps(), zx::VmoOptions::empty());
        vmo.write(ALPHABET.as_bytes(), 0).unwrap();

        let buffer = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

        let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

        race_resize_vec(
            INITIAL_VEC_SIZE,
            buffer.len(),
            buffer.as_ptr(),
            |vec| {
                let mut actual = 42usize;
                assert_eq!(writev(&stream, 0, vec, 1, &mut actual), ZX_OK);
                actual
            },
            |actual| {
                assert!(actual == INITIAL_VEC_SIZE || actual == buffer.len(), "actual: {actual}");

                if actual == INITIAL_VEC_SIZE {
                    let spliced =
                        format!("{}{}", &buffer[..INITIAL_VEC_SIZE], &ALPHABET[INITIAL_VEC_SIZE..]);

                    assert_eq!(get_data(&vmo), spliced);
                } else {
                    assert_eq!(get_data(&vmo), buffer);
                }
            },
        );
    }
}

#[test]
fn partial_vmo_dirty() {
    // The created VMO has to have at least 1 page because it gets mapped. The 2nd and 3rd pages are
    // written to by the stream. The pager allows the 2nd page to be dirtied but fails the 3rd page
    // with ZX_ERR_NO_SPACE. The stream write call should succeed with 1 page being written.
    let mut pager = UserPager::initialized();
    let vmo = pager.create_unbounded_vmo(0, zx::VmoOptions::TRAP_DIRTY).unwrap();
    let stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo.vmo(), 0).unwrap();

    std::thread::scope(|scope| {
        let write_thread = scope.spawn(|| {
            let buffer = vec![0u8; ps() * 2];
            let iovec = iovec(buffer.as_ptr(), ps() * 2);
            let mut bytes_written = 0usize;
            assert_eq!(writev_at(&stream, 0, ps() as u64, &iovec, 1, &mut bytes_written), ZX_OK);
            assert_eq!(bytes_written, ps());
        });

        assert!(pager.wait_for_page_dirty(&vmo, 1, 2, zx::MonotonicInstant::INFINITE));
        assert!(pager.dirty_pages(&vmo, 1, 1));
        assert!(pager.fail_pages(&vmo, 2, 1, zx::Status::NO_SPACE));
        assert!(pager.wait_for_page_dirty(&vmo, 2, 1, zx::MonotonicInstant::INFINITE));
        assert!(pager.fail_pages(&vmo, 2, 1, zx::Status::NO_SPACE));

        write_thread.join().unwrap();
    });
}

#[test]
fn append_supplies_zeroes() {
    let ps = ps();
    let mut pager = UserPager::initialized();
    let vmo: Arc<_> = pager.create_unbounded_vmo(0, zx::VmoOptions::TRAP_DIRTY).unwrap();
    let stream = create_stream(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, vmo.vmo(), 0).unwrap();

    let appends = [ps / 2, ps, ps * 2, ps / 2, ps * 2, ps / 4, ps / 4, ps / 4, ps];

    std::thread::scope(|scope| {
        let append_thread = scope.spawn(|| {
            let buffer = vec![0u8; ps * 2];
            for size in appends {
                let iovec = iovec(buffer.as_ptr(), size);
                let mut bytes_written = 0usize;
                assert_eq!(writev(&stream, ZX_STREAM_APPEND, &iovec, 1, &mut bytes_written), ZX_OK);
                assert_eq!(bytes_written, size);
            }
        });

        let infinite = zx::MonotonicInstant::INFINITE;
        assert!(pager.wait_for_page_dirty(&vmo, 0, 1, infinite));
        assert!(pager.dirty_pages(&vmo, 0, 1));
        assert!(pager.wait_for_page_dirty(&vmo, 1, 1, infinite));
        assert!(pager.dirty_pages(&vmo, 1, 1));
        assert!(pager.wait_for_page_dirty(&vmo, 2, 2, infinite));
        assert!(pager.dirty_pages(&vmo, 2, 2));
        assert!(pager.wait_for_page_dirty(&vmo, 4, 2, infinite));
        assert!(pager.dirty_pages(&vmo, 4, 2));
        assert!(pager.wait_for_page_dirty(&vmo, 6, 1, infinite));
        assert!(pager.dirty_pages(&vmo, 6, 1));
        assert!(pager.wait_for_page_dirty(&vmo, 7, 1, infinite));
        assert!(pager.dirty_pages(&vmo, 7, 1));

        append_thread.join().unwrap();
        // No more requests.
        assert_eq!(pager.get_page_dirty_request(&vmo, zx::MonotonicInstant::INFINITE_PAST), None);
        // If a read request had been generated this test would have hung, but validate there are
        // none anyway.
        assert_eq!(pager.get_page_read_request(&vmo, zx::MonotonicInstant::INFINITE_PAST), None);
    });
}

#[test]
fn syscall_invalid_args() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());

    assert_eq!(stream_create(0xFFFFFFFF, vmo.raw_handle(), 0).unwrap_err(), ZX_ERR_INVALID_ARGS);
    assert_eq!(stream_create(0, ZX_HANDLE_INVALID, 0).unwrap_err(), ZX_ERR_BAD_HANDLE);

    let event = zx::Event::create();
    assert_eq!(stream_create(0, event.raw_handle(), 0).unwrap_err(), ZX_ERR_WRONG_TYPE);

    let read_only_vmo = vmo.duplicate_handle(zx::Rights::READ).unwrap();
    assert_eq!(
        create_stream(ZX_STREAM_MODE_WRITE, &read_only_vmo, 0).unwrap_err(),
        ZX_ERR_ACCESS_DENIED
    );

    let write_only_vmo = vmo.duplicate_handle(zx::Rights::WRITE).unwrap();
    assert_eq!(
        create_stream(ZX_STREAM_MODE_READ, &write_only_vmo, 0).unwrap_err(),
        ZX_ERR_ACCESS_DENIED
    );

    let rw_stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();
    let ro_stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 0).unwrap();
    let wo_stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

    let mut buf = [0u8; 8];
    let mut vec = iovec(buf.as_mut_ptr(), buf.len());
    let mut actual = 0usize;
    let null_vec = std::ptr::null_mut::<zx_iovec_t>();
    let invalid = || -> zx::Stream { zx::NullableHandle::invalid().into() };

    // zx_stream_writev
    assert_eq!(writev(&rw_stream, 0x1234, &vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev(&rw_stream, 0, null_vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev(&invalid(), 0, &vec, 1, &mut actual), ZX_ERR_BAD_HANDLE);
    assert_eq!(writev(&ro_stream, 0, &vec, 1, &mut actual), ZX_ERR_ACCESS_DENIED);

    // zx_stream_writev_at
    assert_eq!(writev_at(&rw_stream, 1, 0, &vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev_at(&rw_stream, 0, 0, null_vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev_at(&invalid(), 0, 0, &vec, 1, &mut actual), ZX_ERR_BAD_HANDLE);
    assert_eq!(writev_at(&ro_stream, 0, 0, &vec, 1, &mut actual), ZX_ERR_ACCESS_DENIED);

    // zx_stream_readv
    assert_eq!(readv(&rw_stream, 1, &mut vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(readv(&rw_stream, 0, null_vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(readv(&invalid(), 0, &mut vec, 1, &mut actual), ZX_ERR_BAD_HANDLE);
    assert_eq!(readv(&wo_stream, 0, &mut vec, 1, &mut actual), ZX_ERR_ACCESS_DENIED);

    // zx_stream_readv_at
    assert_eq!(readv_at(&rw_stream, 1, 0, &mut vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(readv_at(&rw_stream, 0, 0, null_vec, 1, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(readv_at(&invalid(), 0, 0, &mut vec, 1, &mut actual), ZX_ERR_BAD_HANDLE);
    assert_eq!(readv_at(&wo_stream, 0, 0, &mut vec, 1, &mut actual), ZX_ERR_ACCESS_DENIED);

    // zx_stream_seek
    let mut seek_out: zx_off_t = 0;
    assert_eq!(
        stream_seek(&invalid(), ZX_STREAM_SEEK_ORIGIN_START, 0, &mut seek_out),
        ZX_ERR_BAD_HANDLE
    );

    let no_rights_stream = rw_stream.duplicate_handle(zx::Rights::NONE).unwrap();
    assert_eq!(
        stream_seek(&no_rights_stream, ZX_STREAM_SEEK_ORIGIN_START, 0, &mut seek_out),
        ZX_ERR_ACCESS_DENIED
    );
    assert_eq!(stream_seek(&rw_stream, 999, 0, &mut seek_out), ZX_ERR_INVALID_ARGS);
}

#[test]
fn zero_length_vectors() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    assert_eq!(set_content_size(&vmo, 100), ZX_OK);

    let stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &vmo, 10).unwrap();

    let mut buf = [0u8; 8];
    let mut zero_vec = iovec(buf.as_mut_ptr(), 0);
    let mut actual = 999usize;
    let null_vec = std::ptr::null_mut::<zx_iovec_t>();

    // With vector_count = 0 and nullptr vector
    assert_eq!(readv(&stream, 0, null_vec, 0, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(readv_at(&stream, 0, 0, null_vec, 0, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev(&stream, 0, null_vec, 0, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev_at(&stream, 0, 0, null_vec, 0, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev(&stream, ZX_STREAM_APPEND, null_vec, 0, &mut actual), ZX_ERR_INVALID_ARGS);

    // With vector_count = 1 and capacity = 0
    actual = 999;
    assert_eq!(readv(&stream, 0, &mut zero_vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 0);

    actual = 999;
    assert_eq!(readv_at(&stream, 0, 5, &mut zero_vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 0);

    actual = 999;
    assert_eq!(writev(&stream, 0, &zero_vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 0);

    actual = 999;
    assert_eq!(writev_at(&stream, 0, 5, &zero_vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 0);

    actual = 999;
    assert_eq!(writev(&stream, ZX_STREAM_APPEND, &zero_vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 0);

    // Verify seek position has not changed
    let mut seek_pos: zx_off_t = 0;
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &mut seek_pos), ZX_OK);
    assert_eq!(seek_pos, 10);
}

#[test]
fn get_info_read_only() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    assert_eq!(set_content_size(&vmo, 100), ZX_OK);

    let ro_stream = create_stream(ZX_STREAM_MODE_READ, &vmo, 15).unwrap();

    check_info(&ro_stream, ZX_STREAM_MODE_READ, 15, 100);
}

#[test]
fn vector_capacity_overflow() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());

    let stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

    let mut buf = [0u8; 8];
    let mut actual = 0usize;

    // Multi-vector capacity sum overflow in GetTotalCapacity (returns ZX_ERR_INVALID_ARGS)
    let mut bad_overflow_vec = [iovec(buf.as_mut_ptr(), usize::MAX), iovec(buf.as_mut_ptr(), 1)];
    let bad = bad_overflow_vec.as_mut_ptr();
    assert_eq!(readv(&stream, 0, bad, 2, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(readv_at(&stream, 0, 0, bad, 2, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev(&stream, 0, bad, 2, &mut actual), ZX_ERR_INVALID_ARGS);
    assert_eq!(writev_at(&stream, 0, 0, bad, 2, &mut actual), ZX_ERR_INVALID_ARGS);

    // Offset + capacity overflow in CreateWriteOp (returns ZX_ERR_FILE_BIG)
    let overflow_vec = iovec(buf.as_mut_ptr(), u64::MAX as usize);
    assert_eq!(writev_at(&stream, 0, 100, &overflow_vec, 1, &mut actual), ZX_ERR_FILE_BIG);
}

#[test]
fn partial_append_at_vmo_boundary() {
    let vmo = vmo_create(ps(), zx::VmoOptions::empty());
    assert_eq!(set_content_size(&vmo, ps() as u64 - 4), ZX_OK);

    let stream = create_stream(ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

    let buf = *b"0123456789\0";
    let vec = iovec(buf.as_ptr(), 10);
    let mut actual = 0usize;

    // Partial append: requested 10 bytes, but non-resizable VMO only has 4 bytes capacity left.
    // Shrinks the operation size to available capacity and commits 4 bytes.
    assert_eq!(writev(&stream, ZX_STREAM_APPEND, &vec, 1, &mut actual), ZX_OK);
    assert_eq!(actual, 4);
    assert_eq!(get_content_size(&vmo), ps() as u64);

    // Verify seek position advanced to the end of the partial append.
    let mut seek_pos: zx_off_t = 0;
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &mut seek_pos), ZX_OK);
    assert_eq!(seek_pos, ps() as u64);

    // When VMO is completely full, append returns ZX_ERR_OUT_OF_RANGE.
    assert_eq!(writev(&stream, ZX_STREAM_APPEND, &vec, 1, &mut actual), ZX_ERR_OUT_OF_RANGE);
}

#[test]
fn write_vector_unmapped_user_buffer_cancels_operation() {
    let page_size = ps();
    let vmo = vmo_create(page_size, zx::VmoOptions::empty());
    assert_eq!(set_content_size(&vmo, 0), ZX_OK);

    let stream = create_stream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, &vmo, 0).unwrap();

    let (child_vmar, _child_addr) = root_vmar()
        .allocate(
            0,
            page_size * 2,
            zx::VmarFlags::CAN_MAP_READ
                | zx::VmarFlags::CAN_MAP_WRITE
                | zx::VmarFlags::CAN_MAP_SPECIFIC,
        )
        .unwrap();

    let user_vmo = vmo_create(page_size, zx::VmoOptions::empty());

    let mapped_addr = child_vmar
        .map(
            0,
            &user_vmo,
            0,
            page_size,
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::SPECIFIC,
        )
        .unwrap();

    // Buffer starts 4 bytes before end of mapped page, crossing into unmapped page.
    let partial_user_vec = iovec((mapped_addr + page_size - 4) as *const u8, 8);
    let mut actual = 0usize;

    // Failing user copy cancels the pending stream size operation and returns ZX_ERR_NOT_FOUND.
    assert_eq!(writev(&stream, 0, &partial_user_vec, 1, &mut actual), ZX_ERR_NOT_FOUND);
    assert_eq!(writev_at(&stream, 0, 0, &partial_user_vec, 1, &mut actual), ZX_ERR_NOT_FOUND);
    assert_eq!(
        writev(&stream, ZX_STREAM_APPEND, &partial_user_vec, 1, &mut actual),
        ZX_ERR_NOT_FOUND
    );

    // Verify stream seek and content size were not modified by the cancelled operations.
    assert_eq!(get_content_size(&vmo), 0);
    let mut seek_pos: zx_off_t = 0;
    assert_eq!(stream_seek(&stream, ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &mut seek_pos), ZX_OK);
    assert_eq!(seek_pos, 0);

    // SAFETY: Nothing uses the VMAR's mappings.
    unsafe { child_vmar.destroy() }.unwrap();
}
