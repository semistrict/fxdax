// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Errors: the errno values a guest is given, and the errors of setting a filesystem up.

use std::fmt;

/// A Linux errno value, as a guest's Linux kernel understands it: Fxfs's VFSs' own, which gives
/// the errnos of Fxfs's errors (`fxfs_posix::errno`).
pub use fxfs_posix::Errno;

/// The result of a filesystem operation, or with `E`, of setting one up.
pub type Result<T, E = Errno> = std::result::Result<T, E>;

/// An error setting up, checking or tearing down a filesystem.
pub struct Error(anyhow::Error);

impl Error {
    pub(crate) fn new(error: impl Into<anyhow::Error>) -> Self {
        Self(error.into())
    }

    pub(crate) fn msg(message: &'static str) -> Self {
        Self(anyhow::Error::msg(message))
    }

    /// The error, saying what failed with it: its errno stays its own.
    pub(crate) fn context(self, context: impl fmt::Display + Send + Sync + 'static) -> Self {
        Self(self.0.context(context))
    }

    /// The errno that caused the error, if one did: `ENOTSUP` for a store of another format, an
    /// older one included, which is refused (`extent_store::Format`, TASK-165.6).
    pub fn errno(&self) -> Option<Errno> {
        // An errno added as context is found by downcasting the error, not its causes.
        let errno = self.0.downcast_ref::<Errno>().copied();
        errno
            .or_else(|| self.0.chain().find_map(|cause| cause.downcast_ref::<Errno>()).copied())
            .or_else(|| {
                self.0
                    .chain()
                    .any(|cause| cause.is::<extent_store::Format>())
                    .then_some(Errno::ENOTSUP)
            })
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

impl From<crate::faults::Failpoint> for Error {
    fn from(failpoint: crate::faults::Failpoint) -> Self {
        Self::new(anyhow::Error::new(failpoint))
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
    use fxfs::errors::FxfsError;

    /// Fxfs's errors are the errnos Linux's filesystems give them, the guest's among them
    /// (guest/rust/fxfs-linux), as the one table has them (`fxfs_posix::errno`), and an errno an
    /// error carries is its own.
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
            (FxfsError::Inconsistent, Errno::EUCLEAN, Errno::EUCLEAN),
            (FxfsError::IntegrityError, Errno::EBADMSG, Errno::EBADMSG),
        ];
        for (fxfs_error, errno, xattr_errno) in table {
            let error = anyhow::Error::from(fxfs_error.clone()).context("in context");
            assert_eq!(fxfs_posix::errno(&error), errno, "{fxfs_error:?}");
            assert_eq!(fxfs_posix::xattr_errno(&error), xattr_errno, "{fxfs_error:?}");
        }
        let carried = anyhow::Error::from(FxfsError::NotFound).context(Errno::EXDEV);
        assert_eq!(fxfs_posix::errno(&carried), Errno::EXDEV);
    }
}
