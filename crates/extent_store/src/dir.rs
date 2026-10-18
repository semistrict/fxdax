// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The file system a store lives in: a directory of the host's ([`HostDir`]), or of memory
//! ([`MemDir`](crate::MemDir)), which can simulate crashes.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileExt, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// A file of a store.
pub trait File: Send + Sync + 'static {
    /// Reads `buf.len()` bytes at `offset`. Bytes past the file's end read as zeros.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;

    /// Writes `buf` at `offset`.
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<()>;

    /// The file's size.
    fn size(&self) -> io::Result<u64>;

    /// Makes what was written to the file durable.
    fn sync(&self) -> io::Result<()>;
}

/// The directory a store lives in, with what the store needs of its file system. Paths are
/// relative to it: a file's name, or a subdirectory's and a file's, joined by `/`. `""` is the
/// directory itself.
pub trait Dir: Send + Sync + 'static {
    type File: File;

    /// Creates the file at `path`, which must not exist, for reading and writing. `data` says
    /// whether it holds extents, whose reads and writes are block-aligned, and which the host may
    /// keep out of its page cache.
    fn create(&self, path: &str, data: bool) -> io::Result<Self::File>;

    /// Opens the file at `path` for reading and writing; `data` as for `create`.
    fn open(&self, path: &str, data: bool) -> io::Result<Self::File>;

    /// Removes the file at `path`.
    fn remove(&self, path: &str) -> io::Result<()>;

    /// Creates the directory at `path`.
    fn create_dir(&self, path: &str) -> io::Result<()>;

    /// The names of the files in the directory at `path`.
    fn list(&self, path: &str) -> io::Result<Vec<String>>;

    /// Makes the directory's entries durable: the files created and removed in it.
    fn sync_dir(&self, path: &str) -> io::Result<()>;

    /// Makes everything synced durable, past any cache the host's storage keeps.
    fn barrier(&self) -> io::Result<()>;

    /// Takes the directory for this process, against other processes' takings of it: what a
    /// process holds while it has branches of the store open, or opens or deletes them, so that
    /// two processes never change one store at once. It waits a while for another process to let
    /// it go, as one whose last branch is closing does. A directory no other process sees takes
    /// nothing.
    fn take(&self) -> io::Result<()> {
        Ok(())
    }

    /// Lets the directory go, once taken ([`Dir::take`]).
    fn let_go(&self) {}
}

/// How long [`Dir::take`] waits for another process to let a directory of the host's go.
const TAKE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A directory of the host's.
///
/// Extents' files are read bypassing the host's page cache where its file system allows
/// (`O_DIRECT` on Linux, `F_NOCACHE` on macOS), so their reads must be in buffers aligned to
/// blocks. fxdax: on Linux, they are written through it, so that a write doesn't wait for the
/// host's disk, which a commit doesn't need (`Store::flush` makes it durable), and its pages leave
/// the cache once synced: written directly, every `WRITE`, and every commit of an extent written
/// through a DAX window, waited half a millisecond for a network disk. A read bypassing the cache
/// writes back what it reads first. Syncing a file is `fdatasync` on Linux; on macOS it is
/// `fsync`, which leaves the drive's cache to `barrier`, an `F_FULLFSYNC`.
pub struct HostDir {
    path: PathBuf,
    /// Whether extents' files are to bypass the page cache.
    direct: bool,
    /// Whether the last extents' file opened bypasses it.
    bypassing: AtomicBool,
    /// The directory, open, for `barrier`.
    handle: std::fs::File,
}

impl HostDir {
    /// The directory at `path`, which exists. Its extents' files bypass the host's page cache if
    /// `direct` and the host's file system allows.
    pub fn new(path: &Path, direct: bool) -> io::Result<Self> {
        let handle = std::fs::File::open(path)?;
        Ok(Self { path: path.to_owned(), direct, bypassing: AtomicBool::new(false), handle })
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether extents' files bypass the host's page cache, as the last one opened does.
    pub fn is_direct(&self) -> bool {
        self.bypassing.load(Ordering::Relaxed)
    }

    fn file(&self, file: std::fs::File, data: bool) -> io::Result<HostFile> {
        if !(data && self.direct) {
            return Ok(HostFile { file, written: None });
        }
        // The same file again, through the page cache, for writes.
        let written = file.try_clone()?;
        // A file descriptor of its own, whose flags the clone doesn't share. On macOS, the clone
        // shares `F_NOCACHE`: its writes bypass the cache too.
        let path = format!("/proc/self/fd/{}", written.as_raw_fd());
        let written = if cfg!(target_os = "linux") {
            OpenOptions::new().write(true).custom_flags(libc::O_CLOEXEC).open(path)?
        } else {
            written
        };
        let bypassing = bypass_page_cache(file.as_raw_fd());
        self.bypassing.store(bypassing, Ordering::Relaxed);
        Ok(HostFile { file, written: bypassing.then_some(written) })
    }
}

impl Dir for HostDir {
    type File = HostFile;

    fn create(&self, path: &str, data: bool) -> io::Result<HostFile> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(self.path.join(path))?;
        self.file(file, data)
    }

    fn open(&self, path: &str, data: bool) -> io::Result<HostFile> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(self.path.join(path))?;
        self.file(file, data)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        std::fs::remove_file(self.path.join(path))
    }

    fn create_dir(&self, path: &str) -> io::Result<()> {
        std::fs::create_dir(self.path.join(path))
    }

    fn list(&self, path: &str) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(self.path.join(path))? {
            let name = entry?.file_name();
            names.push(name.into_string().map_err(|name| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{name:?} isn't UTF-8"))
            })?);
        }
        Ok(names)
    }

    fn sync_dir(&self, path: &str) -> io::Result<()> {
        std::fs::File::open(self.path.join(path))?.sync_all()
    }

    /// Takes the directory with an exclusive `flock` of it, which the open directory holds until
    /// it is let go, or closed, as when the process exits.
    fn take(&self) -> io::Result<()> {
        let deadline = std::time::Instant::now() + TAKE_WAIT;
        loop {
            // SAFETY: Plain system call on the directory's descriptor.
            if unsafe { libc::flock(self.handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(());
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::WouldBlock {
                return Err(err);
            }
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another process has the store's branches open",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn let_go(&self) {
        // SAFETY: Plain system call on the directory's descriptor.
        if unsafe { libc::flock(self.handle.as_raw_fd(), libc::LOCK_UN) } != 0 {
            let err = io::Error::last_os_error();
            log::warn!("extent store: unlocking {}: {err}", self.path.display());
        }
    }

    fn barrier(&self) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: Flushes an open file descriptor's file, and the drive's cache.
            if unsafe { libc::fcntl(self.handle.as_raw_fd(), libc::F_FULLFSYNC) } < 0 {
                let err = io::Error::last_os_error();
                // Not every file system supports it, where syncing is all there is.
                if !matches!(err.raw_os_error(), Some(libc::ENOTSUP | libc::ENOTTY | libc::EINVAL))
                {
                    return Err(err);
                }
                self.handle.sync_all()?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = &self.handle;
        Ok(())
    }
}

/// A file of a [`HostDir`]: read through `file`, which bypasses the page cache if `written` is
/// given, through which it is written.
pub struct HostFile {
    file: std::fs::File,
    written: Option<std::fs::File>,
}

impl HostFile {
    fn written(&self) -> &std::fs::File {
        self.written.as_ref().unwrap_or(&self.file)
    }
}

impl AsRawFd for HostFile {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

impl File for HostFile {
    fn read_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        while !buf.is_empty() {
            match self.file.read_at(buf, offset) {
                Ok(0) => {
                    buf.fill(0);
                    return Ok(());
                }
                Ok(read) => {
                    buf = &mut buf[read..];
                    offset += read as u64;
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<()> {
        self.written().write_all_at(buf, offset)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    fn sync(&self) -> io::Result<()> {
        let file = self.written();
        if cfg!(target_os = "macos") { file.sync_all() } else { file.sync_data() }?;
        // What was written through the page cache leaves it, now clean: the store's pages are
        // its users' to cache.
        #[cfg(target_os = "linux")]
        if self.written.is_some() {
            // SAFETY: Advises the kernel about an open file descriptor's file.
            unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        }
        Ok(())
    }
}

/// Makes I/O on `fd`'s file bypass the host's page cache, if its file system allows, and returns
/// whether it does.
pub fn bypass_page_cache(fd: RawFd) -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: Reads and sets the status flags of an open file descriptor.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_DIRECT) == 0
        }
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: Sets an option of an open file descriptor.
        unsafe { libc::fcntl(fd, libc::F_NOCACHE, 1) == 0 }
    }
}
