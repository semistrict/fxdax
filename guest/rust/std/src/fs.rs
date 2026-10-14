//! Files, which the kernel's Rust has none of to open.

use crate::io;
use crate::path::Path;

/// A file, which can never be opened.
#[derive(Debug)]
pub struct File(());

impl File {
    pub fn open<P: AsRef<Path>>(_path: P) -> io::Result<File> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn create<P: AsRef<Path>>(_path: P) -> io::Result<File> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// A file's metadata, which is never had.
#[derive(Clone, Debug)]
pub struct Metadata(());

impl Metadata {
    pub fn len(&self) -> u64 {
        0
    }

    pub fn is_empty(&self) -> bool {
        true
    }
}

impl io::Read for File {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

impl io::Write for File {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
