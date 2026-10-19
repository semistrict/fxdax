// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Errors: the errno values a guest is given, and the errors of setting a filesystem up.

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

    /// The errno fdio gives `status` (`fdio_status_to_errno` in `sdk/lib/fdio/fdio_unistd.cc`).
    pub(crate) fn of_status(status: zx::Status) -> Self {
        match status {
            zx::Status::NOT_FOUND => Self::ENOENT,
            zx::Status::NO_MEMORY => Self::ENOMEM,
            zx::Status::INVALID_ARGS => Self::EINVAL,
            zx::Status::BUFFER_TOO_SMALL => Self::EINVAL,
            zx::Status::TIMED_OUT => Self::ETIMEDOUT,
            zx::Status::UNAVAILABLE => Self::EBUSY,
            zx::Status::ALREADY_EXISTS => Self::EEXIST,
            zx::Status::PEER_CLOSED => Self::EPIPE,
            zx::Status::BAD_STATE => Self::EPIPE,
            zx::Status::BAD_PATH => Self::ENAMETOOLONG,
            zx::Status::NOT_FILE => Self::EISDIR,
            zx::Status::NOT_DIR => Self::ENOTDIR,
            zx::Status::NOT_SUPPORTED => Self::ENOTSUP,
            zx::Status::WRONG_TYPE => Self::ENOTSUP,
            zx::Status::OUT_OF_RANGE => Self::EINVAL,
            zx::Status::NO_RESOURCES => Self::ENOMEM,
            zx::Status::BAD_HANDLE => Self::EBADF,
            zx::Status::ACCESS_DENIED => Self::EACCES,
            zx::Status::SHOULD_WAIT => Self::EAGAIN,
            zx::Status::FILE_BIG => Self::EFBIG,
            zx::Status::NO_SPACE => Self::ENOSPC,
            zx::Status::NOT_EMPTY => Self::ENOTEMPTY,
            zx::Status::IO_REFUSED => Self::ECONNREFUSED,
            zx::Status::CANCELED => Self::EBADF,
            zx::Status::PROTOCOL_NOT_SUPPORTED => Self::EPROTONOSUPPORT,
            zx::Status::ADDRESS_UNREACHABLE => Self::ENETUNREACH,
            zx::Status::ADDRESS_IN_USE => Self::EADDRINUSE,
            zx::Status::NOT_CONNECTED => Self::ENOTCONN,
            zx::Status::CONNECTION_REFUSED => Self::ECONNREFUSED,
            zx::Status::CONNECTION_RESET => Self::ECONNRESET,
            zx::Status::CONNECTION_ABORTED => Self::ECONNABORTED,

            // No specific translation, so return a generic value: as fdio does for IO and
            // IO_INVALID too.
            _ => Self::EIO,
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

/// The results the kernel and fuchsia.io give, as errno values.
pub(crate) trait IntoErrno {
    type Ok;

    fn errno(self) -> Result<Self::Ok>;
}

impl<T> IntoErrno for std::result::Result<T, zx::Status> {
    type Ok = T;

    fn errno(self) -> Result<T> {
        self.map_err(Errno::of_status)
    }
}

impl<T> IntoErrno for std::result::Result<T, i32> {
    type Ok = T;

    /// A raw status, as fuchsia.io methods' results have.
    fn errno(self) -> Result<T> {
        self.map_err(|status| Errno::of_status(zx::Status::err_from_raw(status)))
    }
}

impl<T> IntoErrno for std::result::Result<T, fidl::Error> {
    type Ok = T;

    /// A call on a closed connection fails with the status of its epitaph, as fdio's do.
    fn errno(self) -> Result<T> {
        self.map_err(|error| {
            let fidl::Error::ClientChannelClosed { epitaph, .. } = error;
            let status: std::result::Result<(), zx::Status> = epitaph.into();
            status.err().map_or(Errno::EIO, Errno::of_status)
        })
    }
}

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

    /// Statuses are the errnos fdio gives them.
    #[test]
    fn statuses_are_fdios_errnos() {
        use zx::Status;
        let table = [
            (Status::NOT_FOUND, Errno::ENOENT),
            (Status::NO_MEMORY, Errno::ENOMEM),
            (Status::INVALID_ARGS, Errno::EINVAL),
            (Status::BUFFER_TOO_SMALL, Errno::EINVAL),
            (Status::TIMED_OUT, Errno::ETIMEDOUT),
            (Status::UNAVAILABLE, Errno::EBUSY),
            (Status::ALREADY_EXISTS, Errno::EEXIST),
            (Status::PEER_CLOSED, Errno::EPIPE),
            (Status::BAD_STATE, Errno::EPIPE),
            (Status::BAD_PATH, Errno::ENAMETOOLONG),
            (Status::IO, Errno::EIO),
            (Status::NOT_FILE, Errno::EISDIR),
            (Status::NOT_DIR, Errno::ENOTDIR),
            (Status::NOT_SUPPORTED, Errno::ENOTSUP),
            (Status::WRONG_TYPE, Errno::ENOTSUP),
            (Status::OUT_OF_RANGE, Errno::EINVAL),
            (Status::NO_RESOURCES, Errno::ENOMEM),
            (Status::BAD_HANDLE, Errno::EBADF),
            (Status::ACCESS_DENIED, Errno::EACCES),
            (Status::SHOULD_WAIT, Errno::EAGAIN),
            (Status::FILE_BIG, Errno::EFBIG),
            (Status::NO_SPACE, Errno::ENOSPC),
            (Status::NOT_EMPTY, Errno::ENOTEMPTY),
            (Status::IO_REFUSED, Errno::ECONNREFUSED),
            (Status::IO_INVALID, Errno::EIO),
            (Status::CANCELED, Errno::EBADF),
            (Status::PROTOCOL_NOT_SUPPORTED, Errno::EPROTONOSUPPORT),
            (Status::ADDRESS_UNREACHABLE, Errno::ENETUNREACH),
            (Status::ADDRESS_IN_USE, Errno::EADDRINUSE),
            (Status::NOT_CONNECTED, Errno::ENOTCONN),
            (Status::CONNECTION_REFUSED, Errno::ECONNREFUSED),
            (Status::CONNECTION_RESET, Errno::ECONNRESET),
            (Status::CONNECTION_ABORTED, Errno::ECONNABORTED),
            (Status::INTERNAL, Errno::EIO),
        ];
        for (status, errno) in table {
            assert_eq!(Errno::of_status(status), errno, "{status}");
        }
    }
}
