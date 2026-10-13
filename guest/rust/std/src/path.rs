//! Paths, which in the kernel's Rust are only names: there are no files to open by them.

use crate::ffi::{OsStr, OsString};
use alloc_crate::borrow::ToOwned;
use core::borrow::Borrow;
use core::fmt;
use core::ops::Deref;

/// A borrowed path.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct Path(OsStr);

/// An owned path.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PathBuf(OsString);

impl Path {
    pub fn new<S: AsRef<OsStr> + ?Sized>(s: &S) -> &Path {
        // SAFETY: `Path` is a transparent `OsStr`.
        unsafe { &*(s.as_ref() as *const OsStr as *const Path) }
    }

    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }

    pub fn to_str(&self) -> Option<&str> {
        self.0.to_str()
    }

    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf(self.0.to_os_string())
    }

    pub fn display(&self) -> Display<'_> {
        Display(self)
    }
}

/// A path, for printing.
pub struct Display<'a>(&'a Path);

impl fmt::Display for Display<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.0.to_string_lossy(), f)
    }
}

impl Deref for PathBuf {
    type Target = Path;
    fn deref(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl Borrow<Path> for PathBuf {
    fn borrow(&self) -> &Path {
        self
    }
}

impl ToOwned for Path {
    type Owned = PathBuf;
    fn to_owned(&self) -> PathBuf {
        self.to_path_buf()
    }
}

impl AsRef<Path> for Path {
    fn as_ref(&self) -> &Path {
        self
    }
}

impl AsRef<Path> for PathBuf {
    fn as_ref(&self) -> &Path {
        self
    }
}

impl AsRef<Path> for str {
    fn as_ref(&self) -> &Path {
        Path::new(self)
    }
}

impl AsRef<OsStr> for Path {
    fn as_ref(&self) -> &OsStr {
        &self.0
    }
}

impl From<OsString> for PathBuf {
    fn from(s: OsString) -> Self {
        Self(s)
    }
}

impl From<alloc_crate::string::String> for PathBuf {
    fn from(s: alloc_crate::string::String) -> Self {
        Self(s.into())
    }
}
