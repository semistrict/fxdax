//! Errors as Linux errno values, which a guest's FUSE client understands.

use std::fmt;

/// Linux's errno values (`include/uapi/asm-generic/errno-base.h` and `errno.h`), which differ from
/// the host's on macOS.
pub mod linux {
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const EIO: i32 = 5;
    pub const EBADF: i32 = 9;
    pub const EAGAIN: i32 = 11;
    pub const ENOMEM: i32 = 12;
    pub const EACCES: i32 = 13;
    pub const EBUSY: i32 = 16;
    pub const EEXIST: i32 = 17;
    pub const EXDEV: i32 = 18;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const EFBIG: i32 = 27;
    pub const ENOSPC: i32 = 28;
    pub const EPIPE: i32 = 32;
    pub const ENAMETOOLONG: i32 = 36;
    pub const ENOSYS: i32 = 38;
    pub const ENOTEMPTY: i32 = 39;
    pub const EPROTONOSUPPORT: i32 = 93;
    pub const ENOTSUP: i32 = 95;
    pub const EADDRINUSE: i32 = 98;
    pub const ENETUNREACH: i32 = 101;
    pub const ECONNABORTED: i32 = 103;
    pub const ECONNRESET: i32 = 104;
    pub const ENOTCONN: i32 = 107;
    pub const ETIMEDOUT: i32 = 110;
    pub const ECONNREFUSED: i32 = 111;
}

/// An error, as the Linux errno value a guest is given.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Error(i32);

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub const fn new(errno: i32) -> Self {
        Self(errno)
    }

    /// The Linux errno value.
    pub fn errno(self) -> i32 {
        self.0
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error(errno {})", self.0)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "errno {}", self.0)
    }
}

impl std::error::Error for Error {}

impl From<zx::Status> for Error {
    /// The errno fdio gives a status (`fdio_status_to_errno` in `sdk/lib/fdio/fdio_unistd.cc`).
    fn from(status: zx::Status) -> Self {
        use linux::*;
        Self(match status {
            zx::Status::NOT_FOUND => ENOENT,
            zx::Status::NO_MEMORY => ENOMEM,
            zx::Status::INVALID_ARGS => EINVAL,
            zx::Status::BUFFER_TOO_SMALL => EINVAL,
            zx::Status::TIMED_OUT => ETIMEDOUT,
            zx::Status::UNAVAILABLE => EBUSY,
            zx::Status::ALREADY_EXISTS => EEXIST,
            zx::Status::PEER_CLOSED => EPIPE,
            zx::Status::BAD_STATE => EPIPE,
            zx::Status::BAD_PATH => ENAMETOOLONG,
            zx::Status::IO => EIO,
            zx::Status::NOT_FILE => EISDIR,
            zx::Status::NOT_DIR => ENOTDIR,
            zx::Status::NOT_SUPPORTED => ENOTSUP,
            zx::Status::WRONG_TYPE => ENOTSUP,
            zx::Status::OUT_OF_RANGE => EINVAL,
            zx::Status::NO_RESOURCES => ENOMEM,
            zx::Status::BAD_HANDLE => EBADF,
            zx::Status::ACCESS_DENIED => EACCES,
            zx::Status::SHOULD_WAIT => EAGAIN,
            zx::Status::FILE_BIG => EFBIG,
            zx::Status::NO_SPACE => ENOSPC,
            zx::Status::NOT_EMPTY => ENOTEMPTY,
            zx::Status::IO_REFUSED => ECONNREFUSED,
            zx::Status::IO_INVALID => EIO,
            zx::Status::CANCELED => EBADF,
            zx::Status::PROTOCOL_NOT_SUPPORTED => EPROTONOSUPPORT,
            zx::Status::ADDRESS_UNREACHABLE => ENETUNREACH,
            zx::Status::ADDRESS_IN_USE => EADDRINUSE,
            zx::Status::NOT_CONNECTED => ENOTCONN,
            zx::Status::CONNECTION_REFUSED => ECONNREFUSED,
            zx::Status::CONNECTION_RESET => ECONNRESET,
            zx::Status::CONNECTION_ABORTED => ECONNABORTED,

            // No specific translation, so return a generic value.
            _ => EIO,
        })
    }
}

impl From<fidl::Error> for Error {
    /// A call on a closed connection fails with the status of its epitaph, as fdio's do.
    fn from(error: fidl::Error) -> Self {
        let fidl::Error::ClientChannelClosed { epitaph, .. } = error;
        let status: std::result::Result<(), zx::Status> = epitaph.into();
        status.err().map_or(Self(linux::EIO), Self::from)
    }
}

/// A raw status from a fuchsia.io method's result.
pub(crate) fn raw(status: i32) -> Error {
    zx::Status::err_from_raw(status).into()
}
