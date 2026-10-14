//! I/O's traits and in-memory implementations. The kernel's Rust has no streams to the outside
//! but the kernel's log, which `print!` writes to.

use alloc_crate::boxed::Box;
use alloc_crate::string::String;
use alloc_crate::vec::Vec;
use core::fmt;

pub type Result<T> = core::result::Result<T, Error>;

/// The kinds of I/O error: std's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ErrorKind {
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    HostUnreachable,
    NetworkUnreachable,
    ConnectionAborted,
    NotConnected,
    AddrInUse,
    AddrNotAvailable,
    NetworkDown,
    BrokenPipe,
    AlreadyExists,
    WouldBlock,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    ReadOnlyFilesystem,
    FilesystemLoop,
    StaleNetworkFileHandle,
    InvalidInput,
    InvalidData,
    TimedOut,
    WriteZero,
    StorageFull,
    NotSeekable,
    QuotaExceeded,
    FileTooLarge,
    ResourceBusy,
    ExecutableFileBusy,
    Deadlock,
    CrossesDevices,
    TooManyLinks,
    InvalidFilename,
    ArgumentListTooLong,
    Interrupted,
    Unsupported,
    UnexpectedEof,
    OutOfMemory,
    InProgress,
    TooManyOpenFiles,
    InputOutputError,
    Other,
    Uncategorized,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::NotFound => "entity not found",
            ErrorKind::PermissionDenied => "permission denied",
            ErrorKind::ConnectionRefused => "connection refused",
            ErrorKind::ConnectionReset => "connection reset",
            ErrorKind::HostUnreachable => "host unreachable",
            ErrorKind::NetworkUnreachable => "network unreachable",
            ErrorKind::ConnectionAborted => "connection aborted",
            ErrorKind::NotConnected => "not connected",
            ErrorKind::AddrInUse => "address in use",
            ErrorKind::AddrNotAvailable => "address not available",
            ErrorKind::NetworkDown => "network down",
            ErrorKind::BrokenPipe => "broken pipe",
            ErrorKind::AlreadyExists => "entity already exists",
            ErrorKind::WouldBlock => "operation would block",
            ErrorKind::NotADirectory => "not a directory",
            ErrorKind::IsADirectory => "is a directory",
            ErrorKind::DirectoryNotEmpty => "directory not empty",
            ErrorKind::ReadOnlyFilesystem => "read-only filesystem or storage medium",
            ErrorKind::FilesystemLoop => "filesystem loop or indirection limit (e.g. symlink loop)",
            ErrorKind::StaleNetworkFileHandle => "stale network file handle",
            ErrorKind::InvalidInput => "invalid input parameter",
            ErrorKind::InvalidData => "invalid data",
            ErrorKind::TimedOut => "timed out",
            ErrorKind::WriteZero => "write zero",
            ErrorKind::StorageFull => "no storage space",
            ErrorKind::NotSeekable => "seek on unseekable file",
            ErrorKind::QuotaExceeded => "quota exceeded",
            ErrorKind::FileTooLarge => "file too large",
            ErrorKind::ResourceBusy => "resource busy",
            ErrorKind::ExecutableFileBusy => "executable file busy",
            ErrorKind::Deadlock => "deadlock",
            ErrorKind::CrossesDevices => "cross-device link or rename",
            ErrorKind::TooManyLinks => "too many links",
            ErrorKind::InvalidFilename => "invalid filename",
            ErrorKind::ArgumentListTooLong => "argument list too long",
            ErrorKind::Interrupted => "operation interrupted",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::UnexpectedEof => "unexpected end of file",
            ErrorKind::OutOfMemory => "out of memory",
            ErrorKind::InProgress => "in progress",
            ErrorKind::TooManyOpenFiles => "too many open files",
            ErrorKind::InputOutputError => "input/output error",
            ErrorKind::Other => "other error",
            ErrorKind::Uncategorized => "uncategorized error",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An I/O error: a kind, an errno, or a kind with an error of its own.
pub struct Error(Repr);

enum Repr {
    Os(i32),
    Simple(ErrorKind),
    Custom(ErrorKind, Box<dyn core::error::Error + Send + Sync>),
}

impl Error {
    pub fn new<E: Into<Box<dyn core::error::Error + Send + Sync>>>(
        kind: ErrorKind,
        error: E,
    ) -> Self {
        Self(Repr::Custom(kind, error.into()))
    }

    pub fn other<E: Into<Box<dyn core::error::Error + Send + Sync>>>(error: E) -> Self {
        Self::new(ErrorKind::Other, error)
    }

    pub fn from_raw_os_error(code: i32) -> Self {
        Self(Repr::Os(code))
    }

    pub fn raw_os_error(&self) -> Option<i32> {
        match self.0 {
            Repr::Os(code) => Some(code),
            _ => None,
        }
    }

    pub fn kind(&self) -> ErrorKind {
        match &self.0 {
            Repr::Os(code) => match *code {
                1 | 13 => ErrorKind::PermissionDenied,
                2 => ErrorKind::NotFound,
                4 => ErrorKind::Interrupted,
                11 => ErrorKind::WouldBlock,
                12 => ErrorKind::OutOfMemory,
                17 => ErrorKind::AlreadyExists,
                22 => ErrorKind::InvalidInput,
                95 => ErrorKind::Unsupported,
                110 => ErrorKind::TimedOut,
                _ => ErrorKind::Other,
            },
            Repr::Simple(kind) | Repr::Custom(kind, _) => *kind,
        }
    }

    pub fn get_ref(&self) -> Option<&(dyn core::error::Error + Send + Sync + 'static)> {
        match &self.0 {
            Repr::Custom(_, error) => Some(&**error),
            _ => None,
        }
    }

    pub fn into_inner(self) -> Option<Box<dyn core::error::Error + Send + Sync>> {
        match self.0 {
            Repr::Custom(_, error) => Some(error),
            _ => None,
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self(Repr::Simple(kind))
    }
}

impl From<alloc_crate::collections::TryReserveError> for Error {
    fn from(_: alloc_crate::collections::TryReserveError) -> Self {
        ErrorKind::OutOfMemory.into()
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Os(code) => f.debug_struct("Os").field("code", code).finish(),
            Repr::Simple(kind) => f.debug_tuple("Kind").field(kind).finish(),
            Repr::Custom(kind, error) => {
                f.debug_struct("Custom").field("kind", kind).field("error", error).finish()
            }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Os(code) => write!(f, "errno {code}"),
            Repr::Simple(kind) => f.write_str(kind.as_str()),
            Repr::Custom(_, error) => error.fmt(f),
        }
    }
}

impl core::error::Error for Error {}

/// Where a seek is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekFrom {
    Start(u64),
    End(i64),
    Current(i64),
}

/// A buffer a vectored write reads from.
#[derive(Clone, Copy, Debug)]
pub struct IoSlice<'a>(&'a [u8]);

impl<'a> IoSlice<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self(buf)
    }
}

impl core::ops::Deref for IoSlice<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0
    }
}

/// A buffer a vectored read writes to.
#[derive(Debug)]
pub struct IoSliceMut<'a>(&'a mut [u8]);

impl<'a> IoSliceMut<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self(buf)
    }
}

impl core::ops::Deref for IoSliceMut<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0
    }
}

impl core::ops::DerefMut for IoSliceMut<'_> {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.0
    }
}

pub trait Read {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    /// Reads into the first of `bufs` that isn't empty, as std's default does.
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        let buf = bufs.iter_mut().find(|buf| !buf.is_empty()).map_or(&mut [][..], |buf| &mut **buf);
        self.read(buf)
    }

    fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.read(buf) {
                Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(n) => buf = &mut buf[n..],
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let start = buf.len();
        let mut chunk = [0; 512];
        loop {
            match self.read(&mut chunk) {
                Ok(0) => return Ok(buf.len() - start),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        let mut bytes = Vec::new();
        let n = self.read_to_end(&mut bytes)?;
        buf.push_str(
            core::str::from_utf8(&bytes).map_err(|_| Error::from(ErrorKind::InvalidData))?,
        );
        Ok(n)
    }

    fn by_ref(&mut self) -> &mut Self
    where
        Self: Sized,
    {
        self
    }

    fn bytes(self) -> Bytes<Self>
    where
        Self: Sized,
    {
        Bytes(self)
    }

    fn take(self, limit: u64) -> Take<Self>
    where
        Self: Sized,
    {
        Take { inner: self, limit }
    }
}

pub trait Write {
    fn write(&mut self, buf: &[u8]) -> Result<usize>;

    /// Writes the first of `bufs` that isn't empty, as std's default does.
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        let buf = bufs.iter().find(|buf| !buf.is_empty()).map_or(&[][..], |buf| &**buf);
        self.write(buf)
    }

    fn flush(&mut self) -> Result<()>;

    fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(n) => buf = &buf[n..],
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<()> {
        struct Adapter<'a, W: ?Sized> {
            inner: &'a mut W,
            error: Result<()>,
        }
        impl<W: Write + ?Sized> fmt::Write for Adapter<'_, W> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.inner.write_all(s.as_bytes()).map_err(|e| {
                    self.error = Err(e);
                    fmt::Error
                })
            }
        }
        let mut adapter = Adapter { inner: self, error: Ok(()) };
        match fmt::write(&mut adapter, args) {
            Ok(()) => Ok(()),
            Err(_) => adapter.error.and(Err(Error::other("formatter error"))),
        }
    }

    fn by_ref(&mut self) -> &mut Self
    where
        Self: Sized,
    {
        self
    }
}

pub trait Seek {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64>;

    fn stream_position(&mut self) -> Result<u64> {
        self.seek(SeekFrom::Current(0))
    }
}

pub trait BufRead: Read {
    fn fill_buf(&mut self) -> Result<&[u8]>;

    fn consume(&mut self, amount: usize);

    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> Result<usize> {
        let mut read = 0;
        loop {
            let available = self.fill_buf()?;
            let (done, used) = match available.iter().position(|&b| b == byte) {
                Some(i) => (true, i + 1),
                None => (available.is_empty(), available.len()),
            };
            buf.extend_from_slice(&available[..used]);
            self.consume(used);
            read += used;
            if done {
                return Ok(read);
            }
        }
    }

    fn read_line(&mut self, buf: &mut String) -> Result<usize> {
        let mut bytes = Vec::new();
        let n = self.read_until(b'\n', &mut bytes)?;
        buf.push_str(
            core::str::from_utf8(&bytes).map_err(|_| Error::from(ErrorKind::InvalidData))?,
        );
        Ok(n)
    }
}

impl<R: Read + ?Sized> Read for &mut R {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        (**self).read(buf)
    }
}

impl<R: Read + ?Sized> Read for Box<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        (**self).read(buf)
    }
}

impl<W: Write + ?Sized> Write for &mut W {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        (**self).write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

impl<W: Write + ?Sized> Write for Box<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        (**self).write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

impl<B: BufRead + ?Sized> BufRead for &mut B {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        (**self).fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        (**self).consume(amount)
    }
}

impl Read for &[u8] {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = buf.len().min(self.len());
        buf[..n].copy_from_slice(&self[..n]);
        *self = &self[n..];
        Ok(n)
    }
}

impl BufRead for &[u8] {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(self)
    }

    fn consume(&mut self, amount: usize) {
        *self = &self[amount..];
    }
}

impl Write for &mut [u8] {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let n = buf.len().min(self.len());
        let (head, tail) = core::mem::take(self).split_at_mut(n);
        head.copy_from_slice(&buf[..n]);
        *self = tail;
        Ok(n)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for Vec<u8> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// A reader or writer over memory, with a position.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cursor<T> {
    inner: T,
    pos: u64,
}

impl<T> Cursor<T> {
    pub const fn new(inner: T) -> Self {
        Self { inner, pos: 0 }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    pub const fn get_ref(&self) -> &T {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    pub const fn position(&self) -> u64 {
        self.pos
    }

    pub fn set_position(&mut self, pos: u64) {
        self.pos = pos;
    }
}

impl<T: AsRef<[u8]>> Cursor<T> {
    fn remaining(&self) -> &[u8] {
        let inner = self.inner.as_ref();
        &inner[(self.pos as usize).min(inner.len())..]
    }
}

impl<T: AsRef<[u8]>> Read for Cursor<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = self.remaining().read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<T: AsRef<[u8]>> BufRead for Cursor<T> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(self.remaining())
    }

    fn consume(&mut self, amount: usize) {
        self.pos += amount as u64;
    }
}

impl<T: AsRef<[u8]>> Seek for Cursor<T> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let (base, offset) = match pos {
            SeekFrom::Start(n) => {
                self.pos = n;
                return Ok(n);
            }
            SeekFrom::End(n) => (self.inner.as_ref().len() as u64, n),
            SeekFrom::Current(n) => (self.pos, n),
        };
        match base.checked_add_signed(offset) {
            Some(n) => {
                self.pos = n;
                Ok(n)
            }
            None => Err(Error::new(ErrorKind::InvalidInput, "seek to a negative position")),
        }
    }
}

impl Write for Cursor<&mut [u8]> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let start = (self.pos as usize).min(self.inner.len());
        let n = (&mut self.inner[start..]).write(buf)?;
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Writes `buf` at `pos` in `vec`, which grows as need be, as std's cursors over vectors do.
fn vec_write(pos: &mut u64, vec: &mut Vec<u8>, buf: &[u8]) -> usize {
    let start = *pos as usize;
    let end = start + buf.len();
    if vec.len() < end {
        vec.resize(end, 0);
    }
    vec[start..end].copy_from_slice(buf);
    *pos = end as u64;
    buf.len()
}

/// Writes `buf` at `pos` in `slice`, as much as fits, as std's cursors over slices do.
fn slice_write(pos: &mut u64, slice: &mut [u8], buf: &[u8]) -> usize {
    let start = (*pos as usize).min(slice.len());
    let n = buf.len().min(slice.len() - start);
    slice[start..start + n].copy_from_slice(&buf[..n]);
    *pos += n as u64;
    n
}

impl Write for Cursor<Vec<u8>> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Ok(vec_write(&mut self.pos, &mut self.inner, buf))
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for Cursor<&mut Vec<u8>> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Ok(vec_write(&mut self.pos, self.inner, buf))
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for Cursor<alloc_crate::boxed::Box<[u8]>> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Ok(slice_write(&mut self.pos, &mut self.inner, buf))
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl<const N: usize> Write for Cursor<[u8; N]> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Ok(slice_write(&mut self.pos, &mut self.inner, buf))
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// A reader, buffered.
pub struct BufReader<R> {
    inner: R,
    buf: Box<[u8]>,
    pos: usize,
    filled: usize,
}

impl<R: Read> BufReader<R> {
    pub fn new(inner: R) -> Self {
        Self::with_capacity(8192, inner)
    }

    pub fn with_capacity(capacity: usize, inner: R) -> Self {
        Self { inner, buf: alloc_crate::vec![0; capacity].into_boxed_slice(), pos: 0, filled: 0 }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for BufReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = self.fill_buf()?.read(buf)?;
        self.consume(n);
        Ok(n)
    }
}

impl<R: Read> BufRead for BufReader<R> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        if self.pos == self.filled {
            self.filled = self.inner.read(&mut self.buf)?;
            self.pos = 0;
        }
        Ok(&self.buf[self.pos..self.filled])
    }

    fn consume(&mut self, amount: usize) {
        self.pos = (self.pos + amount).min(self.filled);
    }
}

/// A reader's bytes, one at a time.
pub struct Bytes<R>(R);

impl<R: Read> Iterator for Bytes<R> {
    type Item = Result<u8>;
    fn next(&mut self) -> Option<Result<u8>> {
        let mut byte = 0;
        match self.0.read(core::slice::from_mut(&mut byte)) {
            Ok(0) => None,
            Ok(_) => Some(Ok(byte)),
            Err(e) => Some(Err(e)),
        }
    }
}

/// A reader, limited to a number of bytes.
pub struct Take<R> {
    inner: R,
    limit: u64,
}

impl<R: Read> Read for Take<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let max = (buf.len() as u64).min(self.limit) as usize;
        let n = self.inner.read(&mut buf[..max])?;
        self.limit -= n as u64;
        Ok(n)
    }
}

/// A writer that discards what it is given.
pub struct Sink;

pub const fn sink() -> Sink {
    Sink
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Copies what `reader` has into `writer`.
pub fn copy<R: Read + ?Sized, W: Write + ?Sized>(reader: &mut R, writer: &mut W) -> Result<u64> {
    let mut chunk = [0; 512];
    let mut copied = 0;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(copied),
            Ok(n) => {
                writer.write_all(&chunk[..n])?;
                copied += n as u64;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// Prints to the kernel's log at a printk level, for `print!` and friends.
#[doc(hidden)]
pub fn _print(level: i32, args: fmt::Arguments<'_>) {
    let message = alloc_crate::fmt::format(args);
    // SAFETY: Logs the message's bytes.
    unsafe { crate::sys::fxfs_rt_log(level, message.as_ptr().cast(), message.len()) };
}
