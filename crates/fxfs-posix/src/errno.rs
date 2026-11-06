// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the errnos a VFS gives for Fxfs's errors, the guest kernel's (guest/rust/fxfs-linux)
//! and the host's (crates/fxdax) alike: one table of [`FxfsError`]'s, and one rule for which of
//! an error's it is classified by ([`errno`]).

use anyhow::Error;
use fxfs::errors::FxfsError;
use std::fmt;

/// A Linux errno value, as a guest's Linux kernel understands it.
///
/// Values are Linux's on every host, including macOS, whose own differ: an extent-store device
/// sends them to the guest as they are.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Errno(i32);

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
    /// Math result not representable: a buffer too small for what it is to hold.
    ERANGE = 34,
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
    /// Not a data message: data whose checksum doesn't match, as ext4's and XFS's `EFSBADCRC`.
    EBADMSG = 74,
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
    /// Stale handle: a later placement of a store holds its branch (TASK-39.7); a follower cut
    /// off (doc-8).
    ESTALE = 116,
    /// Structure needs cleaning: metadata found inconsistent, as ext4's and XFS's `EFSCORRUPTED`.
    EUCLEAN = 117,
    /// Required key not available: an encrypted node's, as fscrypt has it.
    ENOKEY = 126,
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

/// What a VFS called that failed, for the few errors whose errno depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    /// An operation on a node or its data.
    Node,
    /// An operation on a node's extended attributes, getxattr(2)'s and its kin's.
    Xattr,
}

/// Whether a VFS's callers expect an errno, as part of how the filesystem answers, or it says
/// something is wrong, which is logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reported {
    /// An answer: what was asked for isn't there, or already is, or doesn't fit.
    Expected,
    /// Something wrong: corruption, an I/O error, a bug, or a request no caller should make.
    Logged,
}

/// The errno of `error`, of an operation on a node or its data, as Linux's filesystems give it.
///
/// The rule: an error is classified by the outermost [`Errno`] it holds, else by the outermost
/// [`FxfsError`] it holds, else it is `EIO`. Outermost is as anyhow's `downcast_ref` finds one,
/// a context added on the way out before the error it was added to, then as the error's chain of
/// causes has it, from the outside in. So a caller restates an error by adding context to it:
/// `.context(Errno::EXDEV)`, or `.context(FxfsError::InvalidArgs)` for an argument found wrong
/// deep inside. Fxfs's own `FxfsError::matches` looks at the root cause alone: an FxfsError has
/// no source, so an error's chain holds one at its root if any, and the two agree but for one
/// added as context, which only a restatement adds.
///
/// What isn't [`Reported::Expected`] is logged, with the error.
pub fn errno(error: &Error) -> Errno {
    errno_of_call(error, Call::Node)
}

/// The errno of `error`, of an extended attribute's operation, as [`errno`] has it, but for
/// `ENODATA` for one that doesn't exist, as getxattr(2) has it, and `E2BIG` for one too large.
pub fn xattr_errno(error: &Error) -> Errno {
    errno_of_call(error, Call::Xattr)
}

fn errno_of_call(error: &Error, call: Call) -> Errno {
    if let Some(errno) = outermost::<Errno>(error) {
        return *errno;
    }
    let (errno, reported) = match outermost::<FxfsError>(error) {
        Some(fxfs_error) => of_fxfs(fxfs_error, call),
        None => (Errno::EIO, Reported::Logged),
    };
    if reported == Reported::Logged {
        log::error!("{errno}: {error:#}");
    }
    errno
}

/// The outermost `E` that `error` holds, as [`errno`] has it.
fn outermost<E: std::error::Error + Send + Sync + 'static>(error: &Error) -> Option<&E> {
    error.downcast_ref::<E>().or_else(|| error.chain().find_map(|cause| cause.downcast_ref()))
}

/// The errno of `error`, for `call`, and whether it is one a VFS's callers expect: the table,
/// one row a variant. Corruption says so, as ext4's and XFS's do (OPEN_QUESTIONS.md, TASK-160.21).
const fn of_fxfs(error: &FxfsError, call: Call) -> (Errno, Reported) {
    use Reported::{Expected, Logged};
    let xattr = matches!(call, Call::Xattr);
    match error {
        FxfsError::AlreadyExists => (Errno::EEXIST, Expected),
        FxfsError::Inconsistent => (Errno::EUCLEAN, Logged),
        FxfsError::Internal => (Errno::EIO, Logged),
        FxfsError::NotDir => (Errno::ENOTDIR, Expected),
        FxfsError::NotFile => (Errno::EISDIR, Expected),
        FxfsError::NotFound => (if xattr { Errno::ENODATA } else { Errno::ENOENT }, Expected),
        FxfsError::NotEmpty => (Errno::ENOTEMPTY, Expected),
        FxfsError::ReadOnlyFilesystem => (Errno::EROFS, Logged),
        FxfsError::NoSpace => (Errno::ENOSPC, Expected),
        FxfsError::Deleted => (Errno::ENOENT, Expected),
        FxfsError::InvalidArgs => (Errno::EINVAL, Logged),
        FxfsError::TooBig if xattr => (Errno::E2BIG, Expected),
        FxfsError::TooBig => (Errno::EFBIG, Logged),
        // A format other than this Fxfs's, older or later: as ext4's mount of features it lacks.
        FxfsError::InvalidVersion => (Errno::EINVAL, Logged),
        FxfsError::JournalFlushError => (Errno::EIO, Logged),
        FxfsError::NotSupported => (Errno::ENOTSUP, Logged),
        FxfsError::AccessDenied => (Errno::EACCES, Logged),
        // An offset, or an index, past what it is of.
        FxfsError::OutOfRange => (Errno::EINVAL, Logged),
        // A store unlocked, or a volume mounted, already.
        FxfsError::AlreadyBound => (Errno::EBUSY, Logged),
        FxfsError::BadPath => (Errno::EINVAL, Logged),
        FxfsError::WrongType => (Errno::EINVAL, Logged),
        FxfsError::IntegrityError => (Errno::EBADMSG, Logged),
        FxfsError::Unavailable => (Errno::EBUSY, Logged),
        FxfsError::NoKey => (Errno::ENOKEY, Logged),
        // A link or rename across encryption policies, as fscrypt refuses one.
        FxfsError::InconsistentEncryptionPolicy => (Errno::EXDEV, Logged),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context as _, anyhow};

    /// Each of Fxfs's errors, its errno, its extended attribute's, and whether callers expect it.
    #[test]
    fn each_fxfs_error_has_its_errno() {
        use Reported::{Expected, Logged};
        let table = [
            (FxfsError::AlreadyExists, Errno::EEXIST, Errno::EEXIST, Expected),
            (FxfsError::Inconsistent, Errno::EUCLEAN, Errno::EUCLEAN, Logged),
            (FxfsError::Internal, Errno::EIO, Errno::EIO, Logged),
            (FxfsError::NotDir, Errno::ENOTDIR, Errno::ENOTDIR, Expected),
            (FxfsError::NotFile, Errno::EISDIR, Errno::EISDIR, Expected),
            (FxfsError::NotFound, Errno::ENOENT, Errno::ENODATA, Expected),
            (FxfsError::NotEmpty, Errno::ENOTEMPTY, Errno::ENOTEMPTY, Expected),
            (FxfsError::ReadOnlyFilesystem, Errno::EROFS, Errno::EROFS, Logged),
            (FxfsError::NoSpace, Errno::ENOSPC, Errno::ENOSPC, Expected),
            (FxfsError::Deleted, Errno::ENOENT, Errno::ENOENT, Expected),
            (FxfsError::InvalidArgs, Errno::EINVAL, Errno::EINVAL, Logged),
            (FxfsError::InvalidVersion, Errno::EINVAL, Errno::EINVAL, Logged),
            (FxfsError::JournalFlushError, Errno::EIO, Errno::EIO, Logged),
            (FxfsError::NotSupported, Errno::ENOTSUP, Errno::ENOTSUP, Logged),
            (FxfsError::AccessDenied, Errno::EACCES, Errno::EACCES, Logged),
            (FxfsError::OutOfRange, Errno::EINVAL, Errno::EINVAL, Logged),
            (FxfsError::AlreadyBound, Errno::EBUSY, Errno::EBUSY, Logged),
            (FxfsError::BadPath, Errno::EINVAL, Errno::EINVAL, Logged),
            (FxfsError::WrongType, Errno::EINVAL, Errno::EINVAL, Logged),
            (FxfsError::IntegrityError, Errno::EBADMSG, Errno::EBADMSG, Logged),
            (FxfsError::Unavailable, Errno::EBUSY, Errno::EBUSY, Logged),
            (FxfsError::NoKey, Errno::ENOKEY, Errno::ENOKEY, Logged),
            (FxfsError::InconsistentEncryptionPolicy, Errno::EXDEV, Errno::EXDEV, Logged),
        ];
        for (fxfs_error, node, xattr, reported) in table {
            assert_eq!(of_fxfs(&fxfs_error, Call::Node), (node, reported), "{fxfs_error:?}");
            let error = anyhow!(fxfs_error.clone()).context("in context");
            assert_eq!(errno(&error), node, "{fxfs_error:?}");
            assert_eq!(xattr_errno(&error), xattr, "{fxfs_error:?}");
        }
        // TooBig alone depends on the call for whether it is expected: an extended attribute's
        // value that doesn't fit is an answer.
        assert_eq!(of_fxfs(&FxfsError::TooBig, Call::Node), (Errno::EFBIG, Logged));
        assert_eq!(of_fxfs(&FxfsError::TooBig, Call::Xattr), (Errno::E2BIG, Expected));
    }

    /// An error is classified by the outermost Errno it holds, else the outermost FxfsError,
    /// else it is EIO.
    #[test]
    fn the_outermost_errno_or_fxfs_error_classifies() {
        // An errno added as context, or caused it, over any FxfsError.
        let restated = anyhow!(FxfsError::NotFound).context(Errno::EXDEV);
        assert_eq!(errno(&restated), Errno::EXDEV);
        let caused = anyhow::Error::new(Errno::EINVAL).context(FxfsError::Inconsistent);
        assert_eq!(errno(&caused), Errno::EINVAL);
        // An FxfsError added as context, over the one that caused it, which Fxfs's `matches` sees.
        let restated = anyhow!(FxfsError::NotFound).context(FxfsError::Inconsistent);
        assert_eq!(errno(&restated), Errno::EUCLEAN);
        assert!(FxfsError::NotFound.matches(&restated));
        // An FxfsError deep in the chain, as `bail!(anyhow!(..).context(..))` leaves one: its root.
        let deep = anyhow!(anyhow!(FxfsError::NoSpace).context("deep")).context("on the way");
        assert_eq!(errno(&deep), Errno::ENOSPC);
        assert!(FxfsError::NoSpace.matches(&deep));
        // Neither: an I/O error, as anything Fxfs didn't classify.
        let io = anyhow::Error::new(std::io::Error::other("the device")).context("reading");
        assert_eq!((errno(&io), xattr_errno(&io)), (Errno::EIO, Errno::EIO));
        let result: Result<(), Error> = Err(anyhow!("unclassified"));
        assert_eq!(errno(&result.context("in context").unwrap_err()), Errno::EIO);
    }
}
