//! Cloning images on the host.
//!
//! A clone is a new image with the image's extents' files, and a branch of its own. Where the host
//! filesystem can, each file shares the image's file's blocks, copying each only as either is
//! written: `clonefile` on APFS, `FICLONE` on btrfs and XFS. Elsewhere it is a copy, of the file's
//! data only, so that its holes stay holes: `copy_file_range` on Linux, which shares blocks where
//! it can too, and reads and writes on macOS.

use super::image::{Description, extents_dir, read_root, replace_file};
use anyhow::Context as _;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

/// How a clone was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloneKind {
    /// It shares the image's blocks with it, each of which is copied only once either writes it.
    Shared,
    /// It is a copy of the image's data, at least in part.
    Copied,
}

/// Clones the image at `src` to a new image at `dst`, which must not exist, whose branch is
/// `branch`. The image must not change meanwhile.
pub(super) fn clone_image(src: &Path, dst: &Path, branch: u64) -> anyhow::Result<CloneKind> {
    let description = Description::read(src)?;
    std::fs::create_dir(dst).with_context(|| format!("creating {}", dst.display()))?;
    let cloned = || -> anyhow::Result<CloneKind> {
        std::fs::create_dir(dst.join("extents"))?;
        let mut kind = CloneKind::Shared;
        for low_byte in 0..=u8::MAX {
            let (from, to) = (extents_dir(src, low_byte), extents_dir(dst, low_byte));
            std::fs::create_dir(&to)?;
            for entry in std::fs::read_dir(&from)? {
                let entry = entry?;
                let file = File::open(entry.path())?;
                if clone_file(&file, &to.join(entry.file_name()))? == CloneKind::Copied {
                    kind = CloneKind::Copied;
                }
            }
            File::open(&to)?.sync_all()?;
        }
        File::open(dst.join("extents"))?.sync_all()?;
        let root = read_root(src)?;
        if !root.is_empty() {
            replace_file(dst, "root", &root)?;
        }
        // The description is last: until it is written, the clone is no image.
        Description { branch, ..description }.write(dst)?;
        // The clone's directory entry is as durable as the clone.
        let parent = dst.parent().filter(|parent| !parent.as_os_str().is_empty());
        File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(kind)
    };
    cloned().inspect_err(|_| {
        // Leave nothing half cloned behind.
        let _ = std::fs::remove_dir_all(dst);
    })
}

/// Clones the file `src` to a new file at `dst`, which must not exist.
fn clone_file(src: &File, dst: &Path) -> io::Result<CloneKind> {
    if share(src, dst)? {
        return Ok(CloneKind::Shared);
    }
    let copy = File::options().write(true).create_new(true).open(dst)?;
    if let Err(err) = copy_data(src, &copy) {
        let _ = std::fs::remove_file(dst);
        return Err(err);
    }
    Ok(CloneKind::Copied)
}

/// Whether the error is the host filesystem's refusal to share blocks.
fn cannot_share(err: &io::Error) -> bool {
    super::unsupported(err)
        || matches!(err.raw_os_error(), Some(libc::EXDEV | libc::EINVAL | libc::ENOTTY))
}

/// Clones `src` to `dst` sharing its blocks, returning whether the host filesystem could.
#[cfg(target_os = "macos")]
fn share(src: &File, dst: &Path) -> io::Result<bool> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let dst = CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: Clones an open file to a NUL-terminated path.
    if unsafe { libc::fclonefileat(src.as_raw_fd(), libc::AT_FDCWD, dst.as_ptr(), 0) } == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if cannot_share(&err) { Ok(false) } else { Err(err) }
}

/// Clones `src` to `dst` sharing its blocks, returning whether the host filesystem could.
#[cfg(target_os = "linux")]
fn share(src: &File, dst: &Path) -> io::Result<bool> {
    let file = File::options().read(true).write(true).create_new(true).open(dst)?;
    // SAFETY: Clones an open file into another.
    if unsafe { libc::ioctl(file.as_raw_fd(), libc::FICLONE, src.as_raw_fd()) } == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    let _ = std::fs::remove_file(dst);
    if cannot_share(&err) { Ok(false) } else { Err(err) }
}

/// Copies the data of `src` into `dst`, which is empty, keeping `src`'s size and holes.
fn copy_data(src: &File, dst: &File) -> io::Result<()> {
    let size = src.metadata()?.len();
    dst.set_len(size)?;
    let src = reopen_buffered(src)?;
    let mut offset = 0;
    while offset < size {
        let Some(data) = seek(&src, offset, libc::SEEK_DATA)? else {
            break;
        };
        let hole = seek(&src, data, libc::SEEK_HOLE)?.unwrap_or(size).min(size);
        copy_range(&src, dst, data, hole - data)?;
        offset = hole;
    }
    dst.sync_all()
}

/// The next offset from `offset` of `whence`'s kind, data or a hole, if there is one.
fn seek(file: &File, offset: u64, whence: libc::c_int) -> io::Result<Option<u64>> {
    // SAFETY: Seeks an open file.
    let found = unsafe { libc::lseek(file.as_raw_fd(), offset as libc::off_t, whence) };
    if found >= 0 {
        return Ok(Some(found as u64));
    }
    let err = io::Error::last_os_error();
    // No more data past `offset`.
    if err.raw_os_error() == Some(libc::ENXIO) { Ok(None) } else { Err(err) }
}

/// `file`, opened again without bypassing the page cache, so that it can be read in any sizes.
fn reopen_buffered(file: &File) -> io::Result<File> {
    #[cfg(target_os = "linux")]
    {
        File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))
    }
    #[cfg(target_os = "macos")]
    {
        // `F_NOCACHE` is only a hint, which unaligned reads do not mind.
        file.try_clone()
    }
}

/// Copies the `len` bytes at `offset` from `src` to `dst`, with `copy_file_range`, which shares
/// blocks where it can, unless the files are on different filesystems or the kernel can't, when it
/// reads and writes them.
#[cfg(target_os = "linux")]
fn copy_range(src: &File, dst: &File, offset: u64, len: u64) -> io::Result<()> {
    let (mut src_offset, mut dst_offset) = (offset as libc::loff_t, offset as libc::loff_t);
    let mut remaining = len as usize;
    while remaining > 0 {
        // SAFETY: Copies between open files at offsets it owns.
        let copied = unsafe {
            libc::copy_file_range(
                src.as_raw_fd(),
                &mut src_offset,
                dst.as_raw_fd(),
                &mut dst_offset,
                remaining,
                0,
            )
        };
        match copied {
            0 => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the image shrank")),
            copied if copied < 0 => {
                let err = io::Error::last_os_error();
                return match err.raw_os_error() {
                    Some(libc::EXDEV | libc::ENOSYS | libc::EOPNOTSUPP | libc::EINVAL) => {
                        let done = len - remaining as u64;
                        read_write_range(src, dst, offset + done, remaining as u64)
                    }
                    _ => Err(err),
                };
            }
            copied => remaining -= copied as usize,
        }
    }
    Ok(())
}

/// Copies the `len` bytes at `offset` from `src` to `dst`.
#[cfg(target_os = "macos")]
fn copy_range(src: &File, dst: &File, offset: u64, len: u64) -> io::Result<()> {
    read_write_range(src, dst, offset, len)
}

/// Copies the `len` bytes at `offset` from `src` to `dst` by reading and writing them.
fn read_write_range(src: &File, dst: &File, offset: u64, len: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    let mut buf = vec![0; (1 << 20).min(len as usize)];
    let mut done = 0;
    while done < len {
        let chunk = (buf.len() as u64).min(len - done) as usize;
        src.read_exact_at(&mut buf[..chunk], offset + done)?;
        dst.write_all_at(&buf[..chunk], offset + done)?;
        done += chunk as u64;
    }
    Ok(())
}
