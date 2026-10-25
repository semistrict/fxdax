// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The filesystem the guest kernel's fs/fxfs calls (guest/linux/fs/fxfs/fxfs.h): Fxfs, on the
//! extent-store device a mount names (`device`), serving the volume fxdax's images keep their files
//! in (`fs`).
//!
//! Each call blocks the calling thread until Fxfs is done with it, while Fxfs's own tasks run on
//! the kernel's workers.

mod device;
mod follow;
mod fs;

use anyhow::Error;
use core::ffi::{c_char, c_int, c_void};
use device::ExtentDevice;
use fs::{Attr, DaxExtent, Fxfs, ReaddirStart, SetAttr, StatFs};
use fxfs::errors::FxfsError;
use fxfs::object_store::SetExtendedAttributeMode;
use linux_rt::block_on;
use log::LevelFilter;

// The kernel's errnos (asm-generic/errno-base.h, asm-generic/errno.h).
const ENOENT: c_int = 2;
const EIO: c_int = 5;
const EBUSY: c_int = 16;
const EACCES: c_int = 13;
const EEXIST: c_int = 17;
const ENOTDIR: c_int = 20;
const EISDIR: c_int = 21;
const EINVAL: c_int = 22;
const EFBIG: c_int = 27;
const ENOSPC: c_int = 28;
const EROFS: c_int = 30;
const ERANGE: c_int = 34;
const ENOTEMPTY: c_int = 39;
const E2BIG: c_int = 7;
const ENODATA: c_int = 61;
const EOPNOTSUPP: c_int = 95;

/// The errno of `error`, which is logged unless it is one a filesystem's callers expect.
fn errno(error: &Error) -> c_int {
    let errno = match error.chain().find_map(|cause| cause.downcast_ref::<FxfsError>()) {
        Some(FxfsError::NotFound | FxfsError::Deleted) => return ENOENT,
        Some(FxfsError::AlreadyExists) => return EEXIST,
        Some(FxfsError::NotDir) => return ENOTDIR,
        Some(FxfsError::NotFile) => return EISDIR,
        Some(FxfsError::NotEmpty) => return ENOTEMPTY,
        Some(FxfsError::NoSpace) => return ENOSPC,
        Some(FxfsError::ReadOnlyFilesystem) => EROFS,
        Some(FxfsError::InvalidArgs | FxfsError::BadPath | FxfsError::WrongType) => EINVAL,
        Some(FxfsError::TooBig) => EFBIG,
        Some(FxfsError::OutOfRange) => ERANGE,
        Some(FxfsError::AccessDenied) => EACCES,
        Some(FxfsError::NotSupported) => EOPNOTSUPP,
        Some(FxfsError::Unavailable) => EBUSY,
        _ => EIO,
    };
    log::error!("{error:#}");
    errno
}

/// The errno of `error`, an extended attribute call's: ENODATA for one that doesn't exist, as
/// getxattr(2) has it, and E2BIG for one too large.
fn xattr_errno(error: &Error) -> c_int {
    match error.chain().find_map(|cause| cause.downcast_ref::<FxfsError>()) {
        Some(FxfsError::NotFound) => ENODATA,
        Some(FxfsError::TooBig) => E2BIG,
        _ => errno(error),
    }
}

/// Copies `data` to `buf`, which has room for `size` bytes, and returns its length: or only its
/// length if `size` is 0, or -ERANGE if it doesn't fit.
///
/// # Safety
///
/// `buf` has room for `size` bytes.
unsafe fn copy_out(data: &[u8], buf: *mut u8, size: usize) -> isize {
    if size == 0 {
        return data.len() as isize;
    }
    if data.len() > size {
        return -(ERANGE as isize);
    }
    // SAFETY: As the caller promises; `data` fits.
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len()) };
    data.len() as isize
}

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, and `buf` has room for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_getxattr(
    fs: *const Fxfs,
    ino: u64,
    name: *const c_char,
    len: usize,
    buf: *mut u8,
    size: usize,
) -> isize {
    // SAFETY: As the caller promises.
    let (fs, name) = unsafe { (&*fs, bytes(name, len)) };
    match block_on(fs.reading(|| fs.get_xattr(ino, name))) {
        // SAFETY: As the caller promises.
        Ok(value) => unsafe { copy_out(&value, buf, size) },
        Err(error) => -(xattr_errno(&error) as isize),
    }
}

/// `setxattr(2)`'s flags.
const XATTR_CREATE: c_int = 1;
const XATTR_REPLACE: c_int = 2;

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, and `value` `value_len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_setxattr(
    fs: *const Fxfs,
    ino: u64,
    name: *const c_char,
    len: usize,
    value: *const u8,
    value_len: usize,
    flags: c_int,
) -> c_int {
    let mode = match flags {
        XATTR_CREATE => SetExtendedAttributeMode::Create,
        XATTR_REPLACE => SetExtendedAttributeMode::Replace,
        _ => SetExtendedAttributeMode::Set,
    };
    // SAFETY: As the caller promises.
    let set = unsafe {
        let value = core::slice::from_raw_parts(value, value_len);
        block_on((*fs).set_xattr(ino, bytes(name, len), value, mode))
    };
    match set {
        Ok(()) => 0,
        Err(error) => -xattr_errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, and `buf` has room for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_listxattr(
    fs: *const Fxfs,
    ino: u64,
    buf: *mut u8,
    size: usize,
) -> isize {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match block_on(fs.reading(|| fs.list_xattrs(ino))) {
        Ok(names) => {
            // Each name, and a NUL after it, as listxattr(2) has them.
            let mut list = Vec::new();
            for name in names {
                list.extend_from_slice(&name);
                list.push(0);
            }
            // SAFETY: As the caller promises.
            unsafe { copy_out(&list, buf, size) }
        }
        Err(error) => -(errno(&error) as isize),
    }
}

/// # Safety
///
/// `fs` is mounted, and `name` holds `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_removexattr(
    fs: *const Fxfs,
    ino: u64,
    name: *const c_char,
    len: usize,
) -> c_int {
    // SAFETY: As the caller promises.
    match unsafe { block_on((*fs).remove_xattr(ino, bytes(name, len))) } {
        Ok(()) => 0,
        Err(error) => -xattr_errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, `new_name` `new_len`, and `attr`, `new_attr`,
/// `dir_attr` and `new_dir_attr` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_exchange(
    fs: *const Fxfs,
    dir: u64,
    name: *const c_char,
    len: usize,
    new_dir: u64,
    new_name: *const c_char,
    new_len: usize,
    attr: *mut Attr,
    new_attr: *mut Attr,
    dir_attr: *mut Attr,
    new_dir_attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    unsafe {
        let exchanged =
            block_on((*fs).exchange(dir, bytes(name, len), new_dir, bytes(new_name, new_len)));
        write_attrs(exchanged.map(|attrs| (0, attrs)), [attr, new_attr, dir_attr, new_dir_attr])
    }
}

/// getrandom's backend, which the sysroot's flags select (scripts/build-guest-sysroot.sh): the
/// kernel's generator. getrandom 0.3 calls it too, with an error of the same layout.
///
/// # Safety
///
/// `dest` is writable for `len` bytes.
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    // SAFETY: As the caller promises.
    linux_rt::fill_random(unsafe { core::slice::from_raw_parts_mut(dest, len) });
    Ok(())
}

/// Readies the runtime.
#[unsafe(no_mangle)]
pub extern "C" fn fxfs_init() {
    linux_rt::init_logger(LevelFilter::Info);
    log::info!("Rust runtime ready");
}

/// # Safety
///
/// `source` holds `len` bytes, `fs` and `root` are writable, and `vfs` is the superblock, whose
/// follower's primitives (vfs.h) outlive the filesystem.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_mount(
    source: *const c_char,
    len: usize,
    memory: u64,
    read_only: bool,
    vfs: *mut c_void,
    fs: *mut *mut Fxfs,
    root: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    let tag = unsafe { core::slice::from_raw_parts(source.cast::<u8>(), len) };
    let mounted = block_on(async {
        let device = ExtentDevice::find(tag).await?;
        let mounted = Fxfs::mount(device, memory, read_only, vfs).await?;
        match mounted.root().await {
            Ok(attr) => Ok((mounted, attr)),
            Err(error) => {
                if let Err(unmount_error) = mounted.unmount().await {
                    log::error!("failed to unmount: {unmount_error:#}");
                }
                Err(error)
            }
        }
    });
    match mounted {
        Ok((mounted, attr)) => {
            // SAFETY: As the caller promises.
            unsafe {
                fs.write(Box::into_raw(Box::new(mounted)));
                root.write(attr);
            }
            0
        }
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` came from `fxfs_mount`, and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_unmount(fs: *mut Fxfs) {
    // SAFETY: As the caller promises.
    let fs = unsafe { Box::from_raw(fs) };
    // Its background worker refers to it where it is, boxed: stopped before it moves.
    if let Err(error) = block_on(fs.stop_delayed()) {
        log::error!("queued unlinks failed as the filesystem was unmounted: {error:#}");
    }
    if let Err(error) = block_on(fs.unmount()) {
        log::error!("failed to unmount: {error:#}");
    }
}

/// fxdax: whether `fs` is mounted read-only, and whether it follows its device's writer.
///
/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_read_only(fs: *const Fxfs) -> bool {
    // SAFETY: As the caller promises.
    unsafe { (*fs).read_only() }
}

/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_follows(fs: *const Fxfs) -> bool {
    // SAFETY: As the caller promises.
    unsafe { (*fs).follows() }
}

/// fxdax: a follower's batch (`Fxfs::follow`): 0, or a -errno.
///
/// # Safety
///
/// `fs` is mounted, and follows its device's writer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_follow(fs: *const Fxfs) -> c_int {
    // SAFETY: As the caller promises.
    match block_on(unsafe { (*fs).follow() }) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// fxdax: holds the file `ino`, which the kernel opens, on a follower (`Fxfs::hold`): 0, or a
/// -errno (-ENOENT if a batch deleted it).
///
/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_hold(fs: *const Fxfs, ino: u64) -> c_int {
    // SAFETY: As the caller promises.
    match block_on(unsafe { (*fs).hold(ino) }) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// fxdax: lets go of the file `ino`, which the kernel closes.
///
/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_release(fs: *const Fxfs, ino: u64) {
    // SAFETY: As the caller promises.
    unsafe { (*fs).release(ino) }
}

/// The device `fs` is on, as device.h has it.
///
/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_dax_device(fs: *const Fxfs) -> *mut c_void {
    // SAFETY: As the caller promises.
    unsafe { (*fs).device() }
}

/// Copies `extents` to `out`, which has room for `max`, and returns how many: or -E2BIG if they
/// don't fit, or `result`'s error's -errno.
///
/// # Safety
///
/// `out` has room for `max` extents.
unsafe fn copy_extents(
    result: Result<Vec<DaxExtent>, Error>,
    out: *mut DaxExtent,
    max: usize,
) -> isize {
    match result {
        Ok(extents) if extents.len() > max => -(E2BIG as isize),
        // SAFETY: As the caller promises; they fit.
        Ok(extents) => unsafe {
            core::ptr::copy_nonoverlapping(extents.as_ptr(), out, extents.len());
            extents.len() as isize
        },
        Err(error) => -(errno(&error) as isize),
    }
}

/// # Safety
///
/// `fs` is mounted, and `out` has room for `max` extents.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_dax_extents(
    fs: *const Fxfs,
    ino: u64,
    offset: u64,
    len: u64,
    out: *mut DaxExtent,
    max: usize,
) -> isize {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    let range = offset..offset.saturating_add(len);
    let result = block_on(fs.reading(|| fs.dax_extents(ino, range.clone())));
    // SAFETY: As the caller promises.
    unsafe { copy_extents(result, out, max) }
}

/// # Safety
///
/// `fs` is mounted, and `out` has room for `max` extents.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_dax_map_write(
    fs: *const Fxfs,
    ino: u64,
    offset: u64,
    len: u64,
    size: u64,
    keep: bool,
    out: *mut DaxExtent,
    max: usize,
) -> isize {
    // SAFETY: As the caller promises.
    unsafe {
        let range = offset..offset.saturating_add(len);
        let result = block_on((*fs).dax_map_write(ino, range, size, keep));
        copy_extents(result, out, max)
    }
}

/// Records an append already written through the DAX window (`Fxfs::dax_update_size`).
///
/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_dax_update_size(fs: *const Fxfs, ino: u64, size: u64) -> c_int {
    // SAFETY: As the caller promises.
    match unsafe { block_on((*fs).dax_update_size(ino, size)) } {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, and `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_dax_restage(fs: *const Fxfs, out: *mut DaxExtent) -> bool {
    // SAFETY: As the caller promises.
    unsafe {
        let Some(extent) = (*fs).restage() else { return false };
        out.write(extent);
        true
    }
}

/// Writes `result`'s attributes to `attr`: 0, or its error's -errno.
///
/// # Safety
///
/// `attr` is writable.
unsafe fn write_attr(result: Result<Attr, Error>, attr: *mut Attr) -> c_int {
    // SAFETY: As the caller promises.
    unsafe { write_attrs(result.map(|found| (0, [found])), [attr]) }
}

/// Writes `result`'s attributes to `out`, in order: its value, or its error's -errno.
///
/// # Safety
///
/// `out`'s pointers are writable.
unsafe fn write_attrs<const N: usize>(
    result: Result<(c_int, [Attr; N]), Error>,
    out: [*mut Attr; N],
) -> c_int {
    match result {
        Ok((value, attrs)) => {
            for (attr, out) in attrs.into_iter().zip(out) {
                // SAFETY: As the caller promises.
                unsafe { out.write(attr) };
            }
            value
        }
        Err(error) => -errno(&error),
    }
}

/// The `len` bytes at `ptr`.
///
/// # Safety
///
/// `ptr` holds `len` bytes, which live for `'a`.
unsafe fn bytes<'a>(ptr: *const c_char, len: usize) -> &'a [u8] {
    // SAFETY: As the caller promises.
    unsafe { core::slice::from_raw_parts(ptr.cast::<u8>(), len) }
}

/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_sync(fs: *const Fxfs) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match block_on(fs.sync()) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// fxdax: freezes the filesystem (`Fxfs::freeze`).
///
/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_freeze(fs: *const Fxfs) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match block_on(fs.freeze()) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// fxdax: thaws the filesystem (`Fxfs::thaw`).
///
/// # Safety
///
/// `fs` is mounted, and frozen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_thaw(fs: *const Fxfs) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match block_on(fs.thaw()) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// fxdax: takes the size the host resized the store to (`Fxfs::resize`).
///
/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_resize(fs: *const Fxfs) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match block_on(fs.resize()) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_fsync(fs: *const Fxfs) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match block_on(fs.fsync()) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_punch(fs: *const Fxfs, ino: u64, offset: u64, len: u64) -> c_int {
    // SAFETY: As the caller promises.
    match block_on(unsafe { &*fs }.punch(ino, offset..offset.saturating_add(len))) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, and `attr` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_getattr(fs: *const Fxfs, ino: u64, attr: *mut Attr) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    // SAFETY: As the caller promises.
    unsafe { write_attr(block_on(fs.reading(|| fs.attr_by_handle(ino))), attr) }
}

/// # Safety
///
/// `fs` is mounted, and `statfs` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_statfs(fs: *const Fxfs, statfs: *mut StatFs) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match block_on(fs.statfs()) {
        // SAFETY: As the caller promises.
        Ok(found) => unsafe { statfs.write(found) },
        Err(error) => return -errno(&error),
    }
    0
}

/// # Safety
///
/// `fs` is mounted, and `set` readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_setattr(fs: *const Fxfs, ino: u64, set: *const SetAttr) -> c_int {
    // SAFETY: As the caller promises.
    match unsafe { block_on((*fs).setattr(ino, &*set)) } {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, and `set` readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_write_inode(fs: *const Fxfs, ino: u64, set: *const SetAttr) -> c_int {
    // SAFETY: As the caller promises.
    match unsafe { block_on((*fs).write_inode(ino, &*set)) } {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, `xattrs` `nr_xattrs` extended attributes, each
/// holding the bytes it says, and `attr` and `dir_attr` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_create(
    fs: *const Fxfs,
    dir: u64,
    name: *const c_char,
    len: usize,
    mode: u32,
    rdev: u64,
    uid: u32,
    gid: u32,
    xattrs: *const Xattr,
    nr_xattrs: usize,
    attr: *mut Attr,
    dir_attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    unsafe {
        let xattrs: Vec<(&[u8], &[u8])> = (0..nr_xattrs)
            .map(|i| {
                let xattr = &*xattrs.add(i);
                (bytes(xattr.name, xattr.name_len), bytes(xattr.value.cast(), xattr.value_len))
            })
            .collect();
        let created = block_on((*fs).create(dir, bytes(name, len), mode, rdev, uid, gid, &xattrs));
        write_attrs(created.map(|attrs| (0, attrs)), [attr, dir_attr])
    }
}

/// fxdax: an extended attribute a new object is made with (`fxfs_create`), as C has it
/// (`struct fxfs_xattr`).
#[repr(C)]
pub struct Xattr {
    name: *const c_char,
    name_len: usize,
    value: *const core::ffi::c_void,
    value_len: usize,
}

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, `target` `target_len`, and `attr` and `dir_attr`
/// are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_symlink(
    fs: *const Fxfs,
    dir: u64,
    name: *const c_char,
    len: usize,
    target: *const c_char,
    target_len: usize,
    uid: u32,
    gid: u32,
    attr: *mut Attr,
    dir_attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    unsafe {
        let (name, target) = (bytes(name, len), bytes(target, target_len));
        let created = block_on((*fs).symlink(dir, name, target, uid, gid));
        write_attrs(created.map(|attrs| (0, attrs)), [attr, dir_attr])
    }
}

/// # Safety
///
/// `fs` is mounted, and `buf` has room for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_readlink(
    fs: *const Fxfs,
    ino: u64,
    buf: *mut c_char,
    len: usize,
) -> isize {
    // SAFETY: As the caller promises.
    let (fs, buf) = unsafe { (&*fs, core::slice::from_raw_parts_mut(buf.cast::<u8>(), len)) };
    match block_on(fs.reading(|| fs.readlink(ino))) {
        Ok(target) => {
            let copied = target.len().min(len);
            buf[..copied].copy_from_slice(&target[..copied]);
            target.len() as isize
        }
        Err(error) => -(errno(&error) as isize),
    }
}

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, and `attr` and `dir_attr` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_link(
    fs: *const Fxfs,
    ino: u64,
    dir: u64,
    name: *const c_char,
    len: usize,
    unnamed: bool,
    attr: *mut Attr,
    dir_attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    unsafe {
        let linked = block_on((*fs).link(ino, dir, bytes(name, len), unnamed));
        write_attrs(linked.map(|attrs| (0, attrs)), [attr, dir_attr])
    }
}

/// # Safety
///
/// `fs` is mounted, and `attr` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_clone_range(
    fs: *const Fxfs,
    src: u64,
    offset: u64,
    len: u64,
    dst: u64,
    dst_offset: u64,
    size: u64,
    attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    unsafe {
        let range = offset..offset.saturating_add(len);
        write_attr(block_on((*fs).clone_range(src, range, dst, dst_offset, size)), attr)
    }
}

/// # Safety
///
/// `fs` is mounted, and `attr` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_tmpfile(
    fs: *const Fxfs,
    dir: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    unsafe { write_attr(block_on((*fs).tmpfile(dir, mode, uid, gid)), attr) }
}

/// # Safety
///
/// `fs` is mounted, and `name` holds `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_unlink_later(
    fs: *const Fxfs,
    dir: u64,
    name: *const c_char,
    len: usize,
    ino: u64,
) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match block_on(fs.unlink_later(dir, unsafe { bytes(name, len) }, ino)) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, and `dir_attr` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_unlink(
    fs: *const Fxfs,
    dir: u64,
    name: *const c_char,
    len: usize,
    ino: u64,
    dir_attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    unsafe {
        let unlinked = block_on((*fs).unlink(dir, bytes(name, len), ino));
        write_attrs(unlinked.map(|(replaced, attr)| (replaced as c_int, [attr])), [dir_attr])
    }
}

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, `new_name` `new_len`, and `attr`, `dir_attr` and
/// `new_dir_attr` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_rename(
    fs: *const Fxfs,
    dir: u64,
    name: *const c_char,
    len: usize,
    ino: u64,
    new_dir: u64,
    new_name: *const c_char,
    new_len: usize,
    new_ino: u64,
    attr: *mut Attr,
    dir_attr: *mut Attr,
    new_dir_attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    unsafe {
        let (name, new_name) = (bytes(name, len), bytes(new_name, new_len));
        // No inode is 0, Fxfs's INVALID_OBJECT_ID.
        let new_ino = (new_ino != 0).then_some(new_ino);
        let renamed = block_on((*fs).rename(dir, name, ino, new_dir, new_name, new_ino));
        write_attrs(
            renamed.map(|(replaced, attrs)| (replaced as c_int, attrs)),
            [attr, dir_attr, new_dir_attr],
        )
    }
}

/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_cached_objects(fs: *const Fxfs) -> u64 {
    // SAFETY: As the caller promises.
    unsafe { (*fs).cached() }
}

/// fxdax: writes what the filesystem waited on, and its device's requests took
/// (`Fxfs::stats`), as text, into the `len` bytes at `buf`, as much of it as fits; returns how
/// long it is.
///
/// # Safety
///
/// `fs` is mounted, and `buf` holds `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_stats(fs: *const Fxfs, buf: *mut u8, len: usize) -> usize {
    // SAFETY: As the caller promises.
    let stats = unsafe { (*fs).stats() };
    let copied = stats.len().min(len);
    // SAFETY: As the caller promises, `buf` holds `len` bytes, at least `copied`.
    unsafe { std::ptr::copy_nonoverlapping(stats.as_ptr(), buf, copied) };
    stats.len()
}

/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_free_cached_objects(fs: *const Fxfs, count: u64) -> u64 {
    // SAFETY: As the caller promises.
    unsafe { (*fs).free_cached(count) }
}

/// # Safety
///
/// `fs` is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_evict(fs: *const Fxfs, ino: u64, unlinked: bool) {
    // SAFETY: As the caller promises.
    unsafe { (*fs).evict(ino, unlinked) }
}

/// # Safety
///
/// `fs` is mounted, and `buf` holds `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_write(
    fs: *const Fxfs,
    ino: u64,
    offset: u64,
    buf: *const u8,
    len: usize,
) -> c_int {
    // SAFETY: As the caller promises.
    let (fs, data) = unsafe { (&*fs, core::slice::from_raw_parts(buf, len)) };
    match block_on(fs.write(ino, offset, data)) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// One of `fxfs_read_into`'s buffers: `struct fxfs_read_part`.
#[repr(C)]
pub struct ReadPart {
    buf: *mut u8,
    len: usize,
}

/// # Safety
///
/// `fs` is mounted, `parts` holds `count` parts, and each part's `buf` has room for its `len`
/// bytes, none of them another's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_read_into(
    fs: *const Fxfs,
    ino: u64,
    offset: u64,
    parts: *const ReadPart,
    count: usize,
) -> isize {
    // SAFETY: As the caller promises.
    let (fs, parts) = unsafe { (&*fs, core::slice::from_raw_parts(parts, count)) };
    let mut bufs: Vec<&mut [u8]> = parts
        .iter()
        // SAFETY: As the caller promises.
        .map(|part| unsafe { core::slice::from_raw_parts_mut(part.buf, part.len) })
        .collect();
    match block_on(fs.read_into(ino, offset, &mut bufs)) {
        Ok(read) => read as isize,
        Err(error) => -(errno(&error) as isize),
    }
}

/// One of `fxfs_write_many`'s writes: `struct fxfs_write_part`.
#[repr(C)]
pub struct WritePart {
    offset: u64,
    data: *const u8,
    len: usize,
}

/// # Safety
///
/// `fs` is mounted, `parts` holds `count` parts, and each part's `data` its `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_write_many(
    fs: *const Fxfs,
    ino: u64,
    parts: *const WritePart,
    count: usize,
) -> c_int {
    // SAFETY: As the caller promises.
    let (fs, parts) = unsafe { (&*fs, core::slice::from_raw_parts(parts, count)) };
    let mut writes: Vec<(u64, &[u8])> = parts
        .iter()
        // SAFETY: As the caller promises.
        .map(|part| (part.offset, unsafe { core::slice::from_raw_parts(part.data, part.len) }))
        .collect();
    match block_on(fs.write_many(ino, &mut writes)) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, `name` holds `len` bytes, and `attr` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_lookup(
    fs: *const Fxfs,
    dir: u64,
    name: *const c_char,
    len: usize,
    attr: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    let (fs, name) = unsafe { (&*fs, bytes(name, len)) };
    // SAFETY: As the caller promises.
    unsafe { write_attr(block_on(fs.reading(|| fs.lookup(dir, name))), attr) }
}

/// What `fxfs_readdir` calls for each entry, until it returns false.
type Emit =
    extern "C" fn(ctx: *mut c_void, name: *const c_char, len: usize, ino: u64, mode: u32) -> bool;

/// # Safety
///
/// `fs` is mounted, `emit` may be called with `ctx`, and `after`, unless null, holds `after_len`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_readdir(
    fs: *const Fxfs,
    dir: u64,
    index: u64,
    after: *const c_char,
    after_len: usize,
    emit: Emit,
    ctx: *mut c_void,
) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    let start = if after.is_null() {
        ReaddirStart::Index(index)
    } else {
        // SAFETY: As the caller promises.
        ReaddirStart::After(unsafe { bytes(after, after_len) })
    };
    let emit = |name: &str, ino, mode| emit(ctx, name.as_ptr().cast(), name.len(), ino, mode);
    match block_on(fs.readdir(dir, start, emit)) {
        Ok(()) => 0,
        Err(error) => -errno(&error),
    }
}

/// # Safety
///
/// `fs` is mounted, and `buf` has room for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_read(
    fs: *const Fxfs,
    ino: u64,
    offset: u64,
    buf: *mut u8,
    len: usize,
) -> isize {
    // SAFETY: As the caller promises.
    let (fs, buf) = unsafe { (&*fs, core::slice::from_raw_parts_mut(buf, len)) };
    match block_on(fs.read(ino, offset, buf)) {
        Ok(read) => read as isize,
        Err(error) => -(errno(&error) as isize),
    }
}
