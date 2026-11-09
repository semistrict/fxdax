// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A volume as a SQLite VFS ([`register`]): its databases' files are the volume's, by the names
//! SQLite opens them by, and their journals and temporary files are memory (see `volume`).
//!
//! With `PRAGMA mmap_size`, SQLite reads pages in place, in the volume's window, if it has one,
//! with nothing copied (`xFetch`); otherwise, or for pages changed since the last commit, it reads
//! them as it would without.
//!
//! A transaction is committed as SQLite syncs the database's file at its end, or, if told not to
//! sync (`synchronous=OFF`), as it unlocks it. WAL mode is refused: its log would be memory, which
//! a crash loses, and SQLite then keeps to the rollback journal; nor does the VFS give SQLite the
//! shared memory WAL mode needs otherwise. Randomness, sleeping and the time are the default VFS's.

use crate::locks::Level;
use crate::volume::{Failed, MemoryFile, NotFound, Volume};
use anyhow::{Error, bail};
use rusqlite::ffi;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Arc, Mutex};

/// The longest name a file of a volume has.
const MAX_PATHNAME: c_int = 512;

/// What the VFS's functions find in it.
struct App {
    volume: Arc<Volume>,
    /// The default VFS, which randomness, sleeping and the time are of.
    default: *mut ffi::sqlite3_vfs,
}

/// A volume registered as a SQLite VFS, until dropped, which only a volume none of whose files is
/// open may be.
pub struct Registration {
    vfs: Box<ffi::sqlite3_vfs>,
    _name: CString,
    app: Box<App>,
}

// SAFETY: SQLite's VFS functions may be called from any thread; what they share is the volume's,
// behind its lock.
unsafe impl Send for Registration {}
unsafe impl Sync for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        let opens = self.app.volume.opens();
        assert_eq!(opens, 0, "unregistering a volume whose files are open");
        // SAFETY: registered, and no connection uses it.
        unsafe { ffi::sqlite3_vfs_unregister(&mut *self.vfs) };
    }
}

/// Registers `volume` as the SQLite VFS `name`, which no other is, not as the default: a
/// connection opens databases on it as `rusqlite::Connection::open_with_flags_and_vfs` does.
pub fn register(name: &str, volume: Arc<Volume>) -> Result<Registration, Error> {
    let name = CString::new(name)?;
    // SAFETY: what SQLite asks before registering.
    let initialized = unsafe { ffi::sqlite3_initialize() };
    if initialized != ffi::SQLITE_OK {
        bail!("initializing SQLite: {initialized}");
    }
    // SAFETY: a lookup; the default's, and `name`'s, which must not be.
    let default = unsafe { ffi::sqlite3_vfs_find(std::ptr::null()) };
    if unsafe { !ffi::sqlite3_vfs_find(name.as_ptr()).is_null() } {
        bail!("a SQLite VFS named {name:?} is registered already");
    }
    let mut app = Box::new(App { volume, default });
    let mut vfs = Box::new(ffi::sqlite3_vfs {
        iVersion: 2,
        szOsFile: size_of::<Handle>() as c_int,
        mxPathname: MAX_PATHNAME,
        pNext: std::ptr::null_mut(),
        zName: name.as_ptr(),
        pAppData: &mut *app as *mut App as *mut c_void,
        xOpen: Some(x_open),
        xDelete: Some(x_delete),
        xAccess: Some(x_access),
        xFullPathname: Some(x_full_pathname),
        xDlOpen: None,
        xDlError: None,
        xDlSym: None,
        xDlClose: None,
        xRandomness: Some(x_randomness),
        xSleep: Some(x_sleep),
        xCurrentTime: Some(x_current_time),
        xGetLastError: Some(x_get_last_error),
        xCurrentTimeInt64: Some(x_current_time_int64),
        xSetSystemCall: None,
        xGetSystemCall: None,
        xNextSystemCall: None,
    });
    // SAFETY: `vfs`, its name and its app live until it is unregistered, as the registration
    // holds them.
    let registered = unsafe { ffi::sqlite3_vfs_register(&mut *vfs, 0) };
    if registered != ffi::SQLITE_OK {
        bail!("registering the SQLite VFS {name:?}: {registered}");
    }
    Ok(Registration { vfs, _name: name, app })
}

/// What a connection has open.
enum Open {
    Database { volume: Arc<Volume>, name: String, lock: Level },
    Memory(MemoryFile),
}

/// A file SQLite opened on the VFS: its `sqlite3_file`, then what it is.
#[repr(C)]
struct Handle {
    base: ffi::sqlite3_file,
    open: Open,
}

static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    // 3, for memory-mapped reads (`xFetch`); without shared memory, which WAL mode would need.
    iVersion: 3,
    xClose: Some(x_close),
    xRead: Some(x_read),
    xWrite: Some(x_write),
    xTruncate: Some(x_truncate),
    xSync: Some(x_sync),
    xFileSize: Some(x_file_size),
    xLock: Some(x_lock),
    xUnlock: Some(x_unlock),
    xCheckReservedLock: Some(x_check_reserved_lock),
    xFileControl: Some(x_file_control),
    xSectorSize: Some(x_sector_size),
    xDeviceCharacteristics: Some(x_device_characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: Some(x_fetch),
    xUnfetch: Some(x_unfetch),
};

/// The VFS's app.
///
/// # Safety
///
/// `vfs` must be one [`register`] made.
unsafe fn app<'a>(vfs: *mut ffi::sqlite3_vfs) -> &'a App {
    unsafe { &*((*vfs).pAppData as *const App) }
}

/// What `file` has open.
///
/// # Safety
///
/// `file` must be one `x_open` opened, not closed.
unsafe fn open<'a>(file: *mut ffi::sqlite3_file) -> &'a mut Open {
    unsafe { &mut (*(file as *mut Handle)).open }
}

/// `name`, a C string SQLite passes.
///
/// # Safety
///
/// `name` must be a C string.
unsafe fn name_of(name: *const c_char) -> Option<String> {
    match name.is_null() {
        true => None,
        false => Some(unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned()),
    }
}

/// The SQLite result of `result`: `code` if it failed, logged.
fn code_of(result: Result<(), Error>, code: c_int) -> c_int {
    match result {
        Ok(()) => ffi::SQLITE_OK,
        Err(error) => {
            log::error!("sqlite_extents: {error:#}");
            match error.is::<NotFound>() {
                true => ffi::SQLITE_CANTOPEN,
                false if error.is::<Failed>() => ffi::SQLITE_IOERR,
                false => code,
            }
        }
    }
}

unsafe extern "C" fn x_open(
    vfs: *mut ffi::sqlite3_vfs,
    name: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    // SAFETY: as SQLite calls it.
    let (app, name) = unsafe { (app(vfs), name_of(name)) };
    unsafe { (*file).pMethods = std::ptr::null() };
    let opened = match name {
        _ if flags & ffi::SQLITE_OPEN_WAL != 0 => {
            log::warn!("sqlite_extents: WAL mode is refused: its log would be lost in a crash");
            return ffi::SQLITE_CANTOPEN;
        }
        Some(name) if flags & ffi::SQLITE_OPEN_MAIN_DB != 0 => {
            let create = flags & ffi::SQLITE_OPEN_CREATE != 0;
            let volume = app.volume.clone();
            volume.open_file(&name, create).map(|()| Open::Database {
                volume,
                name,
                lock: Level::None,
            })
        }
        Some(name) if flags & ffi::SQLITE_OPEN_DELETEONCLOSE == 0 => {
            let create = flags & ffi::SQLITE_OPEN_CREATE != 0;
            app.volume.memory_file(&name, create).map(Open::Memory)
        }
        _ => Ok(Open::Memory(Arc::new(Mutex::new(Vec::new())))),
    };
    match opened {
        Ok(opened) => {
            // SAFETY: SQLite gives `szOsFile` bytes, a handle's.
            unsafe {
                std::ptr::write(
                    file as *mut Handle,
                    Handle { base: ffi::sqlite3_file { pMethods: &METHODS }, open: opened },
                );
                if !out_flags.is_null() {
                    *out_flags = flags;
                }
            }
            ffi::SQLITE_OK
        }
        Err(error) => code_of(Err(error), ffi::SQLITE_CANTOPEN),
    }
}

unsafe extern "C" fn x_delete(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    _sync_dir: c_int,
) -> c_int {
    // SAFETY: as SQLite calls it.
    let (app, name) = unsafe { (app(vfs), name_of(name).unwrap_or_default()) };
    match app.volume.delete(&name) {
        Ok(true) => ffi::SQLITE_OK,
        Ok(false) => ffi::SQLITE_IOERR_DELETE_NOENT,
        Err(error) => code_of(Err(error), ffi::SQLITE_IOERR_DELETE),
    }
}

unsafe extern "C" fn x_access(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    _flags: c_int,
    out: *mut c_int,
) -> c_int {
    // SAFETY: as SQLite calls it.
    let (app, name) = unsafe { (app(vfs), name_of(name).unwrap_or_default()) };
    match app.volume.exists(&name) {
        Ok(exists) => {
            unsafe { *out = exists as c_int };
            ffi::SQLITE_OK
        }
        Err(error) => code_of(Err(error), ffi::SQLITE_IOERR_ACCESS),
    }
}

unsafe extern "C" fn x_full_pathname(
    _vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    size: c_int,
    out: *mut c_char,
) -> c_int {
    // SAFETY: as SQLite calls it: `out` holds `size` bytes.
    let name = unsafe { CStr::from_ptr(name) }.to_bytes_with_nul();
    if name.len() > size as usize {
        return ffi::SQLITE_CANTOPEN;
    }
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr() as *const c_char, out, name.len()) };
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_randomness(
    vfs: *mut ffi::sqlite3_vfs,
    len: c_int,
    out: *mut c_char,
) -> c_int {
    // SAFETY: the default VFS's, as SQLite calls it.
    unsafe {
        let default = app(vfs).default;
        (*default).xRandomness.unwrap()(default, len, out)
    }
}

unsafe extern "C" fn x_sleep(vfs: *mut ffi::sqlite3_vfs, micros: c_int) -> c_int {
    // SAFETY: the default VFS's, as SQLite calls it.
    unsafe {
        let default = app(vfs).default;
        (*default).xSleep.unwrap()(default, micros)
    }
}

unsafe extern "C" fn x_current_time(vfs: *mut ffi::sqlite3_vfs, out: *mut f64) -> c_int {
    // SAFETY: the default VFS's, as SQLite calls it.
    unsafe {
        let default = app(vfs).default;
        (*default).xCurrentTime.unwrap()(default, out)
    }
}

unsafe extern "C" fn x_current_time_int64(
    vfs: *mut ffi::sqlite3_vfs,
    out: *mut ffi::sqlite3_int64,
) -> c_int {
    // SAFETY: the default VFS's, as SQLite calls it.
    unsafe {
        let default = app(vfs).default;
        (*default).xCurrentTimeInt64.unwrap()(default, out)
    }
}

unsafe extern "C" fn x_get_last_error(
    vfs: *mut ffi::sqlite3_vfs,
    len: c_int,
    out: *mut c_char,
) -> c_int {
    // SAFETY: the default VFS's, as SQLite calls it.
    unsafe {
        let default = app(vfs).default;
        (*default).xGetLastError.unwrap()(default, len, out)
    }
}

unsafe extern "C" fn x_close(file: *mut ffi::sqlite3_file) -> c_int {
    // SAFETY: SQLite closes what it opened once; its memory is SQLite's after.
    let handle = unsafe { std::ptr::read(file as *mut Handle) };
    if let Open::Database { volume, name, lock } = handle.open {
        let mut lock = lock;
        let unlocked = volume.unlock_file(&name, &mut lock, Level::None);
        volume.close_file(&name);
        return code_of(unlocked, ffi::SQLITE_IOERR_CLOSE);
    }
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_read(
    file: *mut ffi::sqlite3_file,
    buf: *mut c_void,
    len: c_int,
    offset: ffi::sqlite3_int64,
) -> c_int {
    // SAFETY: as SQLite calls it: `buf` holds `len` bytes.
    let buf = unsafe { std::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
    let read = match unsafe { open(file) } {
        Open::Database { volume, name, .. } => volume.read(name, offset as u64, buf),
        Open::Memory(data) => {
            let data = data.lock().unwrap();
            let start = (offset as usize).min(data.len());
            let read = (data.len() - start).min(buf.len());
            buf[..read].copy_from_slice(&data[start..start + read]);
            Ok(read)
        }
    };
    match read {
        Ok(read) if read == buf.len() => ffi::SQLITE_OK,
        Ok(read) => {
            buf[read..].fill(0);
            ffi::SQLITE_IOERR_SHORT_READ
        }
        Err(error) => code_of(Err(error), ffi::SQLITE_IOERR_READ),
    }
}

unsafe extern "C" fn x_write(
    file: *mut ffi::sqlite3_file,
    data: *const c_void,
    len: c_int,
    offset: ffi::sqlite3_int64,
) -> c_int {
    // SAFETY: as SQLite calls it: `data` holds `len` bytes.
    let data = unsafe { std::slice::from_raw_parts(data as *const u8, len as usize) };
    let written = match unsafe { open(file) } {
        Open::Database { volume, name, .. } => volume.write(name, offset as u64, data),
        Open::Memory(file) => {
            let mut file = file.lock().unwrap();
            let end = offset as usize + data.len();
            if file.len() < end {
                file.resize(end, 0);
            }
            file[offset as usize..end].copy_from_slice(data);
            Ok(())
        }
    };
    code_of(written, ffi::SQLITE_IOERR_WRITE)
}

unsafe extern "C" fn x_truncate(file: *mut ffi::sqlite3_file, size: ffi::sqlite3_int64) -> c_int {
    // SAFETY: as SQLite calls it.
    let truncated = match unsafe { open(file) } {
        Open::Database { volume, name, .. } => volume.truncate(name, size as u64),
        Open::Memory(file) => {
            file.lock().unwrap().truncate(size as usize);
            Ok(())
        }
    };
    code_of(truncated, ffi::SQLITE_IOERR_TRUNCATE)
}

unsafe extern "C" fn x_sync(file: *mut ffi::sqlite3_file, _flags: c_int) -> c_int {
    // SAFETY: as SQLite calls it.
    match unsafe { open(file) } {
        Open::Database { volume, name, .. } => code_of(volume.sync(name), ffi::SQLITE_IOERR_FSYNC),
        Open::Memory(_) => ffi::SQLITE_OK,
    }
}

unsafe extern "C" fn x_file_size(
    file: *mut ffi::sqlite3_file,
    out: *mut ffi::sqlite3_int64,
) -> c_int {
    // SAFETY: as SQLite calls it.
    let size = match unsafe { open(file) } {
        Open::Database { volume, name, .. } => volume.size(name),
        Open::Memory(file) => Ok(file.lock().unwrap().len() as u64),
    };
    match size {
        Ok(size) => {
            unsafe { *out = size as ffi::sqlite3_int64 };
            ffi::SQLITE_OK
        }
        Err(error) => code_of(Err(error), ffi::SQLITE_IOERR_FSTAT),
    }
}

unsafe extern "C" fn x_lock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    let want = Level::from_sqlite(level).expect("a lock level");
    // SAFETY: as SQLite calls it.
    match unsafe { open(file) } {
        Open::Database { volume, name, lock } => match volume.lock_file(name, lock, want) {
            Ok(true) => ffi::SQLITE_OK,
            Ok(false) => ffi::SQLITE_BUSY,
            Err(error) => code_of(Err(error), ffi::SQLITE_IOERR_LOCK),
        },
        Open::Memory(_) => ffi::SQLITE_OK,
    }
}

unsafe extern "C" fn x_unlock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    let want = Level::from_sqlite(level).expect("a lock level");
    // SAFETY: as SQLite calls it.
    match unsafe { open(file) } {
        Open::Database { volume, name, lock } => {
            code_of(volume.unlock_file(name, lock, want), ffi::SQLITE_IOERR_UNLOCK)
        }
        Open::Memory(_) => ffi::SQLITE_OK,
    }
}

unsafe extern "C" fn x_check_reserved_lock(file: *mut ffi::sqlite3_file, out: *mut c_int) -> c_int {
    // SAFETY: as SQLite calls it.
    let reserved = match unsafe { open(file) } {
        Open::Database { volume, name, .. } => volume.reserved(name),
        Open::Memory(_) => Ok(false),
    };
    match reserved {
        Ok(reserved) => {
            unsafe { *out = reserved as c_int };
            ffi::SQLITE_OK
        }
        Err(error) => code_of(Err(error), ffi::SQLITE_IOERR_CHECKRESERVEDLOCK),
    }
}

unsafe extern "C" fn x_file_control(
    _file: *mut ffi::sqlite3_file,
    _op: c_int,
    _arg: *mut c_void,
) -> c_int {
    ffi::SQLITE_NOTFOUND
}

unsafe extern "C" fn x_sector_size(file: *mut ffi::sqlite3_file) -> c_int {
    // SAFETY: as SQLite calls it.
    match unsafe { open(file) } {
        Open::Database { volume, .. } => volume.device().block_size() as c_int,
        Open::Memory(_) => 512,
    }
}

unsafe extern "C" fn x_device_characteristics(_file: *mut ffi::sqlite3_file) -> c_int {
    // A write damages nothing around it, and none is durable before the next, nor reordered:
    // nothing is durable until the commit.
    ffi::SQLITE_IOCAP_POWERSAFE_OVERWRITE
        | ffi::SQLITE_IOCAP_SAFE_APPEND
        | ffi::SQLITE_IOCAP_SEQUENTIAL
}

unsafe extern "C" fn x_fetch(
    file: *mut ffi::sqlite3_file,
    offset: ffi::sqlite3_int64,
    len: c_int,
    out: *mut *mut c_void,
) -> c_int {
    // SAFETY: as SQLite calls it.
    let fetched = match unsafe { open(file) } {
        Open::Database { volume, name, .. } => volume.fetch(name, offset as u64, len as u64),
        Open::Memory(_) => Ok(None),
    };
    match fetched {
        Ok(fetched) => {
            // Read-only, as SQLite reads what it fetches, and copies what it changes.
            let at = fetched.map_or(std::ptr::null_mut(), |at| at as *mut c_void);
            unsafe { *out = at };
            ffi::SQLITE_OK
        }
        Err(error) => {
            unsafe { *out = std::ptr::null_mut() };
            code_of(Err(error), ffi::SQLITE_IOERR_MMAP)
        }
    }
}

unsafe extern "C" fn x_unfetch(
    file: *mut ffi::sqlite3_file,
    _offset: ffi::sqlite3_int64,
    page: *mut c_void,
) -> c_int {
    // SAFETY: as SQLite calls it. A page of none is SQLite asking for its mapping to go, which the
    // window keeps in step with the file whatever it holds.
    if let (Open::Database { volume, name, .. }, false) = (unsafe { open(file) }, page.is_null()) {
        volume.unfetch(name);
    }
    ffi::SQLITE_OK
}
