// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Errors: the errno values a guest is given, and the errors of setting a filesystem up.

use fxfs::errors::FxfsError;
use std::fmt;

/// A Linux errno value, as a guest's Linux kernel understands it.
///
/// Values are Linux's on every host, including macOS, whose own differ: an extent-store device
/// sends them to the guest as they are.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Errno(i32);

/// The result of a filesystem operation, or with `E`, of setting one up.
pub type Result<T, E = Errno> = std::result::Result<T, E>;

macro_rules! errnos {
    ($($(#[$doc:meta])* $name:ident = $value:literal,)*) => {
        impl Errno {
            $($(#[$doc])* pub const $name: Self = Self($value);)*

            /// The symbolic name of the value, such as `"ENOENT"`, if it is one of the above.
            pub fn name(self) -> Option<&'static str> {
                match self.0 {
                    $($value => Some(stringify!($name)),)*
                    _ => None,
                }
            }
        }
    };
}

// From Linux's `include/uapi/asm-generic/errno-base.h` and `errno.h`.
errnos! {
    /// Operation not permitted.
    EPERM = 1,
    /// No such file or directory.
    ENOENT = 2,
    /// I/O error.
    EIO = 5,
    /// Argument list too long: an extended attribute's value is larger than a filesystem keeps.
    E2BIG = 7,
    /// Bad file number.
    EBADF = 9,
    /// Try again.
    EAGAIN = 11,
    /// Out of memory.
    ENOMEM = 12,
    /// Permission denied.
    EACCES = 13,
    /// Device or resource busy.
    EBUSY = 16,
    /// File exists.
    EEXIST = 17,
    /// Cross-device link.
    EXDEV = 18,
    /// Not a directory.
    ENOTDIR = 20,
    /// Is a directory.
    EISDIR = 21,
    /// Invalid argument.
    EINVAL = 22,
    /// File too large.
    EFBIG = 27,
    /// No space left on device.
    ENOSPC = 28,
    /// Read-only file system.
    EROFS = 30,
    /// Broken pipe.
    EPIPE = 32,
    /// File name too long.
    ENAMETOOLONG = 36,
    /// Function not implemented.
    ENOSYS = 38,
    /// Directory not empty.
    ENOTEMPTY = 39,
    /// No data available: a node has no extended attribute of a name.
    ENODATA = 61,
    /// Protocol error.
    EPROTO = 71,
    /// Protocol not supported.
    EPROTONOSUPPORT = 93,
    /// Operation not supported.
    ENOTSUP = 95,
    /// Address already in use.
    EADDRINUSE = 98,
    /// Network is unreachable.
    ENETUNREACH = 101,
    /// Software caused connection abort.
    ECONNABORTED = 103,
    /// Connection reset by peer.
    ECONNRESET = 104,
    /// Transport endpoint is not connected: the filesystem is unmounted.
    ENOTCONN = 107,
    /// Connection timed out.
    ETIMEDOUT = 110,
    /// Connection refused.
    ECONNREFUSED = 111,
    /// Stale handle: a later placement of a store holds its branch (TASK-39.7).
    ESTALE = 116,
}

impl Errno {
    /// The errno with Linux value `raw`.
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// The Linux value.
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// The errno of `error`, an Fxfs operation's, as Linux's filesystems give it: the errno it
    /// carries, or the one of the Fxfs error causing it. Any other is an I/O error, which is
    /// logged.
    pub(crate) fn of_fxfs(error: &anyhow::Error) -> Self {
        // An errno added as context is found by downcasting the error, not its causes.
        let errno = error.downcast_ref::<Errno>();
        if let Some(errno) = errno.or_else(|| error.chain().find_map(|cause| cause.downcast_ref()))
        {
            return *errno;
        }
        let fxfs_error = error.chain().find_map(|cause| cause.downcast_ref::<FxfsError>());
        match fxfs_error {
            Some(FxfsError::NotFound | FxfsError::Deleted) => Self::ENOENT,
            Some(FxfsError::AlreadyExists) => Self::EEXIST,
            Some(FxfsError::NotDir) => Self::ENOTDIR,
            Some(FxfsError::NotFile) => Self::EISDIR,
            Some(FxfsError::NotEmpty) => Self::ENOTEMPTY,
            Some(FxfsError::NoSpace) => Self::ENOSPC,
            Some(FxfsError::ReadOnlyFilesystem) => Self::EROFS,
            Some(FxfsError::InvalidArgs | FxfsError::BadPath | FxfsError::WrongType) => {
                Self::EINVAL
            }
            Some(FxfsError::TooBig) => Self::EFBIG,
            Some(FxfsError::AccessDenied) => Self::EACCES,
            Some(FxfsError::NotSupported) => Self::ENOTSUP,
            Some(FxfsError::Unavailable | FxfsError::AlreadyBound) => Self::EBUSY,
            _ => {
                log::error!("fxdax: {error:#}");
                Self::EIO
            }
        }
    }

    /// The errno of `error`, an extended attribute's operation's: `ENODATA` for one that doesn't
    /// exist, as getxattr(2) has it, and `E2BIG` for one too large.
    pub(crate) fn of_xattr(error: &anyhow::Error) -> Self {
        match error.chain().find_map(|cause| cause.downcast_ref::<FxfsError>()) {
            Some(FxfsError::NotFound) => Self::ENODATA,
            Some(FxfsError::TooBig) => Self::E2BIG,
            _ => Self::of_fxfs(error),
        }
    }
}

impl fmt::Debug for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "Errno({name})"),
            None => write!(f, "Errno({})", self.0),
        }
    }
}

impl fmt::Display for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name} (errno {})", self.0),
            None => write!(f, "errno {}", self.0),
        }
    }
}

impl std::error::Error for Errno {}

/// An error setting up, checking or tearing down a filesystem.
pub struct Error(anyhow::Error);

impl Error {
    pub(crate) fn new(error: impl Into<anyhow::Error>) -> Self {
        Self(error.into())
    }

    pub(crate) fn msg(message: &'static str) -> Self {
        Self(anyhow::Error::msg(message))
    }

    /// The errno that caused the error, if one did.
    pub fn errno(&self) -> Option<Errno> {
        // An errno added as context is found by downcasting the error, not its causes.
        self.0
            .downcast_ref::<Errno>()
            .or_else(|| self.0.chain().find_map(|cause| cause.downcast_ref::<Errno>()))
            .copied()
    }

    /// Whether what failed waited for something busy and gave up: another process's lock of a
    /// store (`extent_store`'s, which an extent daemon serving it holds).
    #[cfg(target_os = "linux")]
    pub(crate) fn is_busy(&self) -> bool {
        self.0.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|err| err.kind() == std::io::ErrorKind::WouldBlock)
        })
    }
}

impl From<Errno> for Error {
    fn from(errno: Errno) -> Self {
        Self::new(errno)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for Error {
    /// The error, without its sources; `{:#}` includes them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fxfs's errors are the errnos Linux's filesystems give them, the guest's among them
    /// (guest/rust/fxfs-linux), and an errno an error carries is its own.
    #[test]
    fn fxfs_errors_are_linuxs_errnos() {
        let table = [
            (FxfsError::NotFound, Errno::ENOENT, Errno::ENODATA),
            (FxfsError::Deleted, Errno::ENOENT, Errno::ENOENT),
            (FxfsError::AlreadyExists, Errno::EEXIST, Errno::EEXIST),
            (FxfsError::NotDir, Errno::ENOTDIR, Errno::ENOTDIR),
            (FxfsError::NotFile, Errno::EISDIR, Errno::EISDIR),
            (FxfsError::NotEmpty, Errno::ENOTEMPTY, Errno::ENOTEMPTY),
            (FxfsError::NoSpace, Errno::ENOSPC, Errno::ENOSPC),
            (FxfsError::TooBig, Errno::EFBIG, Errno::E2BIG),
            (FxfsError::InvalidArgs, Errno::EINVAL, Errno::EINVAL),
            (FxfsError::AlreadyBound, Errno::EBUSY, Errno::EBUSY),
            (FxfsError::Inconsistent, Errno::EIO, Errno::EIO),
        ];
        for (fxfs_error, errno, xattr_errno) in table {
            let error = anyhow::Error::from(fxfs_error.clone()).context("in context");
            assert_eq!(Errno::of_fxfs(&error), errno, "{fxfs_error:?}");
            assert_eq!(Errno::of_xattr(&error), xattr_errno, "{fxfs_error:?}");
        }
        let carried = anyhow::Error::from(FxfsError::NotFound).context(Errno::EXDEV);
        assert_eq!(Errno::of_fxfs(&carried), Errno::EXDEV);
    }

}
