// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! guest/sim's model of Fxfs, through fxfs.h's calls as dax.c and data.c make them, over the
//! shim's store (shim/shim.c). The model is C with globals of its own, so there is one at a time
//! (`Model::new`), on one thread. A failure of the model's, a `sim_panic`, panics here.
//!
//! Its scheduling points (`sim_point`) call what `Model::at_points` sets, which may run more of the
//! model there, as its locks allow: none of those calls reaches it again.

use fxfs_posix::{Attr, SetAttr};
use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};

/// `struct fxfs`, the model's.
#[repr(C)]
struct Fs {
    _opaque: [u8; 0],
}

/// `struct fxfs_write_part`.
#[repr(C)]
struct WritePart {
    offset: u64,
    data: *const c_void,
    len: usize,
}

/// `struct fxfs_dax_extent`: a piece of a file's data, `len` bytes at `offset`, at `extent_offset`
/// in the extent `sequence`, open (1) or sealed (0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Extent {
    pub offset: u64,
    pub len: u64,
    pub branch: u64,
    pub sequence: u64,
    pub extent_offset: u64,
    pub access: u32,
    pub reserved: u32,
    pub extent_size: u64,
}

/// `FXFS_DAX_OPEN`.
pub const OPEN: u32 = 1;

unsafe extern "C-unwind" {
    fn fxfs_sim_new() -> *mut Fs;
    fn fxfs_sim_free(fs: *mut Fs);
    fn sim_device_reset();
    fn fxfs_sim_new_file(fs: *mut Fs) -> u64;
    fn fxfs_sim_size(fs: *mut Fs, ino: u64) -> u64;
    fn fxfs_sim_read(fs: *mut Fs, ino: u64, offset: u64, buf: *mut c_void, len: usize);
    fn fxfs_sim_reserved(fs: *mut Fs, ino: u64) -> u64;
    fn fxfs_write_many(
        fs: *mut Fs,
        ino: u64,
        parts: *const WritePart,
        count: usize,
        dirty: u64,
    ) -> c_int;
    fn fxfs_write_inode(fs: *mut Fs, ino: u64, set: *const SetAttr) -> c_int;
    fn fxfs_clone_range(
        fs: *mut Fs,
        src: u64,
        offset: u64,
        len: u64,
        dst: u64,
        dst_offset: u64,
        size: u64,
        attr: *mut Attr,
    ) -> c_int;
    fn fxfs_punch(fs: *mut Fs, ino: u64, offset: u64, len: u64) -> c_int;
    fn fxfs_fsync(fs: *mut Fs) -> c_int;
    fn fxfs_reserve_dirty(fs: *mut Fs, ino: u64, bytes: u64) -> c_int;
    fn fxfs_dax_extents(
        fs: *mut Fs,
        ino: u64,
        offset: u64,
        len: u64,
        out: *mut Extent,
        max: usize,
    ) -> isize;
    fn fxfs_set_fail_commits(count: u64);
}

/// A `sim_panic` of the model's, or of the shim's: unwinds to the case that ran it.
#[unsafe(no_mangle)]
extern "C-unwind" fn fxfs_sim_diff_panic(message: *const c_char) -> ! {
    // SAFETY: shim.c's sim_panic passes a string it formatted.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    panic!("the model: {message}");
}

type Points = Box<dyn FnMut(&Model, &str)>;

thread_local! {
    /// What runs at the model's scheduling points, and the model's: taken while it runs, so that
    /// what it runs of the model finds none.
    static POINTS: RefCell<Option<(*mut Fs, Points)>> = const { RefCell::new(None) };
}

#[unsafe(no_mangle)]
extern "C-unwind" fn fxfs_sim_diff_point(what: *const c_char) {
    // SAFETY: the model's points are string literals.
    let what = unsafe { CStr::from_ptr(what) }.to_str().expect("a point's name");
    let Some((fs, mut points)) = POINTS.with(|points| points.borrow_mut().take()) else { return };
    // The model's, which its owner drops.
    let model = ManuallyDrop::new(Model { fs });
    points(&model, what);
    POINTS.with(|slot| *slot.borrow_mut() = Some((fs, points)));
}

/// Whether a model exists, of which there is one at a time.
static EXISTS: AtomicBool = AtomicBool::new(false);

/// The model, a filesystem of its own.
pub struct Model {
    fs: *mut Fs,
}

/// A failure of one of the model's calls: its errno.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Errno(pub i32);

fn result(status: c_int) -> Result<(), Errno> {
    if status == 0 { Ok(()) } else { Err(Errno(-status)) }
}

impl Model {
    /// A new model, empty, with nothing at its points. Panics if another exists.
    pub fn new() -> Self {
        assert!(!EXISTS.swap(true, Ordering::SeqCst), "two models at once");
        // SAFETY: the shim's store is the model's alone, and no model exists.
        unsafe {
            sim_device_reset();
            fxfs_set_fail_commits(0);
            Self { fs: fxfs_sim_new() }
        }
    }

    /// Has `points` run at each of the model's scheduling points, with the model and the point's
    /// name, until the model goes.
    pub fn at_points(&self, points: impl FnMut(&Self, &str) + 'static) {
        POINTS.with(|slot| *slot.borrow_mut() = Some((self.fs, Box::new(points))));
    }

    pub fn new_file(&self) -> u64 {
        // SAFETY: `fs` is the model's, on this thread, as every call's.
        unsafe { fxfs_sim_new_file(self.fs) }
    }

    pub fn size(&self, ino: u64) -> u64 {
        // SAFETY: As `new_file`'s.
        unsafe { fxfs_sim_size(self.fs, ino) }
    }

    /// `len` bytes of `ino` at `offset`, from its blocks, whole: past its size too, as a DAX
    /// window maps them.
    pub fn read(&self, ino: u64, offset: u64, len: usize) -> Vec<u8> {
        let mut buf = vec![0; len];
        // SAFETY: As `new_file`'s; `buf` holds `len` bytes.
        unsafe { fxfs_sim_read(self.fs, ino, offset, buf.as_mut_ptr().cast(), len) };
        buf
    }

    /// The room for dirty pages `ino` holds.
    pub fn reserved(&self, ino: u64) -> u64 {
        // SAFETY: As `new_file`'s.
        unsafe { fxfs_sim_reserved(self.fs, ino) }
    }

    pub fn reserve_dirty(&self, ino: u64, bytes: u64) -> Result<(), Errno> {
        // SAFETY: As `new_file`'s.
        result(unsafe { fxfs_reserve_dirty(self.fs, ino, bytes) })
    }

    /// fxfs_write_many: `writes`, bytes at offsets, paid with `dirty` bytes of room reserved.
    pub fn write_many(&self, ino: u64, writes: &[(u64, &[u8])], dirty: u64) -> Result<(), Errno> {
        let parts: Vec<_> = writes
            .iter()
            .map(|(offset, data)| WritePart {
                offset: *offset,
                data: data.as_ptr().cast(),
                len: data.len(),
            })
            .collect();
        // SAFETY: As `new_file`'s; each part's data lives through the call.
        result(unsafe { fxfs_write_many(self.fs, ino, parts.as_ptr(), parts.len(), dirty) })
    }

    pub fn write_inode(&self, ino: u64, set: &SetAttr) -> Result<(), Errno> {
        // SAFETY: As `new_file`'s.
        result(unsafe { fxfs_write_inode(self.fs, ino, set) })
    }

    pub fn clone_range(
        &self,
        src: u64,
        offset: u64,
        len: u64,
        dst: u64,
        dst_offset: u64,
        size: u64,
    ) -> Result<Attr, Errno> {
        let mut attr = Attr::default();
        // SAFETY: As `new_file`'s.
        let status = unsafe {
            fxfs_clone_range(self.fs, src, offset, len, dst, dst_offset, size, &mut attr)
        };
        result(status).map(|()| attr)
    }

    pub fn punch(&self, ino: u64, offset: u64, len: u64) -> Result<(), Errno> {
        // SAFETY: As `new_file`'s.
        result(unsafe { fxfs_punch(self.fs, ino, offset, len) })
    }

    /// A seal: fxfs_fsync.
    pub fn fsync(&self) -> Result<(), Errno> {
        // SAFETY: As `new_file`'s.
        result(unsafe { fxfs_fsync(self.fs) })
    }

    /// Has the next `count` writebacks fail between their bytes and their commits.
    pub fn fail_commits(&self, count: u64) {
        // SAFETY: A global of the model's, which is on this thread.
        unsafe { fxfs_set_fail_commits(count) }
    }

    /// The pieces of `ino`'s data, all of them.
    pub fn extents(&self, ino: u64) -> Vec<Extent> {
        let mut out = vec![Extent::default(); 64];
        loop {
            // SAFETY: As `new_file`'s; `out` holds as many as it says.
            let count = unsafe {
                fxfs_dax_extents(self.fs, ino, 0, u64::MAX >> 1, out.as_mut_ptr(), out.len())
            };
            match usize::try_from(count) {
                Ok(count) => {
                    out.truncate(count);
                    return out;
                }
                Err(_) if count == -7 => out.resize(out.len() * 2, Extent::default()),
                Err(_) => panic!("the model's extents: errno {}", -count),
            }
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        POINTS.with(|slot| slot.borrow_mut().take());
        // A model a panic left mid-call is left as it is, its memory too: only its store goes.
        if !std::thread::panicking() {
            // SAFETY: `fs` is the model's, and nothing uses it again.
            unsafe { fxfs_sim_free(self.fs) };
        }
        // SAFETY: The store is the model's, which nothing uses again.
        unsafe { sim_device_reset() };
        EXISTS.store(false, Ordering::SeqCst);
    }
}
