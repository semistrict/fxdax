//! The filesystem the guest kernel's fs/fxfs calls (guest/linux/fs/fxfs/fxfs.h): Fxfs, on the
//! extent-store device a mount names (`device`), serving the volume fxdax's images keep their files
//! in (`fs`).
//!
//! Each call blocks the calling thread until Fxfs is done with it, while Fxfs's own tasks run on
//! the kernel's workers.

mod device;
mod fs;

use anyhow::Error;
use core::ffi::{c_char, c_int, c_void};
use device::ExtentDevice;
use fs::{Attr, Fxfs};
use fxfs::errors::FxfsError;
use linux_rt::block_on;
use log::LevelFilter;

// The kernel's errnos (asm-generic/errno-base.h, asm-generic/errno.h).
const ENOENT: c_int = 2;
const EIO: c_int = 5;
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
const EOPNOTSUPP: c_int = 95;

/// The errno of `error`, which is logged unless it is one a filesystem's callers expect.
fn errno(error: &Error) -> c_int {
    let errno = match error.root_cause().downcast_ref::<FxfsError>() {
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
        _ => EIO,
    };
    log::error!("{error:#}");
    errno
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
/// `source` holds `len` bytes, and `fs` and `root` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_mount(
    source: *const c_char,
    len: usize,
    fs: *mut *mut Fxfs,
    root: *mut Attr,
) -> c_int {
    // SAFETY: As the caller promises.
    let tag = unsafe { core::slice::from_raw_parts(source.cast::<u8>(), len) };
    let mounted = block_on(async {
        let mounted = Fxfs::mount(ExtentDevice::find(tag).await?).await?;
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
    if let Err(error) = block_on(fs.unmount()) {
        log::error!("failed to unmount: {error:#}");
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
    let (fs, name) = unsafe { (&*fs, core::slice::from_raw_parts(name.cast::<u8>(), len)) };
    match block_on(fs.lookup(dir, name)) {
        // SAFETY: As the caller promises.
        Ok(found) => unsafe { attr.write(found) },
        Err(error) => return -errno(&error),
    }
    0
}

/// What `fxfs_readdir` calls for each entry, until it returns false.
type Emit =
    extern "C" fn(ctx: *mut c_void, name: *const c_char, len: usize, ino: u64, mode: u32) -> bool;

/// # Safety
///
/// `fs` is mounted, and `emit` may be called with `ctx`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_readdir(
    fs: *const Fxfs,
    dir: u64,
    index: u64,
    emit: Emit,
    ctx: *mut c_void,
) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    let emit = |name: &str, ino, mode| emit(ctx, name.as_ptr().cast(), name.len(), ino, mode);
    match block_on(fs.readdir(dir, index, emit)) {
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
