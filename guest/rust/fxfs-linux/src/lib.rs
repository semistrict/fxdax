//! The filesystem the guest kernel's fs/fxfs calls (guest/linux/fs/fxfs/fxfs.h).
//!
//! For now a stand-in for Fxfs, which shows that Rust built outside Kbuild runs in the kernel
//! (TASK-26.1): a root directory holding one file, whose contents a task on the kernel's workers
//! produces after a timer, each time the file is read.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_void};
use core::time::Duration;
use linux_rt::{Timer, block_on, spawn};
use log::LevelFilter;

const ROOT_INO: u64 = 1;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const ENOENT: c_int = 2;
const ENOTDIR: c_int = 20;
const EISDIR: c_int = 21;

/// The one file, and what it holds.
const HELLO: (u64, &str) = (2, "hello");
const HELLO_CONTENTS: &[u8] = b"Hello from Rust in the kernel\n";

/// An inode's attributes: `struct fxfs_attr`.
#[repr(C)]
pub struct Attr {
    ino: u64,
    size: u64,
    mode: u32,
    nlink: u32,
}

/// A mounted filesystem: `struct fxfs`.
pub struct Fxfs {
    _private: (),
}

impl Fxfs {
    fn getattr(&self, ino: u64) -> Result<Attr, c_int> {
        match ino {
            ROOT_INO => Ok(Attr { ino, size: 0, mode: S_IFDIR | 0o755, nlink: 2 }),
            ino if ino == HELLO.0 => {
                Ok(Attr { ino, size: HELLO_CONTENTS.len() as u64, mode: S_IFREG | 0o444, nlink: 1 })
            }
            _ => Err(ENOENT),
        }
    }

    fn lookup(&self, dir: u64, name: &[u8]) -> Result<Attr, c_int> {
        if dir != ROOT_INO {
            return Err(ENOTDIR);
        }
        if name != HELLO.1.as_bytes() {
            return Err(ENOENT);
        }
        self.getattr(HELLO.0)
    }

    /// The file's contents, which a spawned task produces once a timer fires.
    async fn contents(&self, ino: u64) -> Result<Vec<u8>, c_int> {
        if ino != HELLO.0 {
            return Err(EISDIR);
        }
        Ok(spawn(async {
            Timer::new(Duration::from_millis(1)).await;
            HELLO_CONTENTS.to_vec()
        })
        .await)
    }
}

/// Readies the runtime.
#[unsafe(no_mangle)]
pub extern "C" fn fxfs_init() {
    linux_rt::init_logger(LevelFilter::Info);
    log::info!("Rust runtime ready");
}

#[unsafe(no_mangle)]
pub extern "C" fn fxfs_mount() -> *mut Fxfs {
    Box::into_raw(Box::new(Fxfs { _private: () }))
}

/// # Safety
///
/// `fs` came from `fxfs_mount`, and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_unmount(fs: *mut Fxfs) {
    // SAFETY: As the caller promises.
    drop(unsafe { Box::from_raw(fs) });
}

/// # Safety
///
/// `fs` is mounted, and `attr` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fxfs_getattr(fs: *const Fxfs, ino: u64, attr: *mut Attr) -> c_int {
    // SAFETY: As the caller promises.
    let fs = unsafe { &*fs };
    match fs.getattr(ino) {
        // SAFETY: As the caller promises.
        Ok(found) => unsafe { attr.write(found) },
        Err(errno) => return -errno,
    }
    0
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
    match fs.lookup(dir, name) {
        // SAFETY: As the caller promises.
        Ok(found) => unsafe { attr.write(found) },
        Err(errno) => return -errno,
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
    if dir != ROOT_INO {
        return -ENOTDIR;
    }
    for (ino, name) in [HELLO].into_iter().skip(index as usize) {
        let mode = fs.getattr(ino).map_or(0, |attr| attr.mode);
        if !emit(ctx, name.as_ptr().cast(), name.len(), ino, mode) {
            break;
        }
    }
    0
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
    let fs = unsafe { &*fs };
    let contents = match block_on(fs.contents(ino)) {
        Ok(contents) => contents,
        Err(errno) => return -(errno as isize),
    };
    let start = usize::try_from(offset).unwrap_or(usize::MAX).min(contents.len());
    let read = &contents[start..(start + len).min(contents.len())];
    // SAFETY: As the caller promises; `read` is no longer than `len`.
    unsafe { core::ptr::copy_nonoverlapping(read.as_ptr(), buf, read.len()) };
    read.len() as isize
}
