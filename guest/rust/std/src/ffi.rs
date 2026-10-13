//! C strings, and OS strings, which in the kernel are bytes.

use alloc_crate::borrow::{Cow, ToOwned};
use alloc_crate::boxed::Box;
use alloc_crate::string::String;
use alloc_crate::vec::Vec;
use core::borrow::Borrow;
use core::fmt;
use core::ops::Deref;

pub use alloc_crate::ffi::*;
// alloc's, which has `CStr` and `CString`.
pub use alloc_crate::ffi::c_str;
pub use core::ffi::*;

/// A borrowed OS string: bytes.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct OsStr([u8]);

/// An owned OS string: bytes.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OsString(Vec<u8>);

impl OsStr {
    pub fn new<S: AsRef<OsStr> + ?Sized>(s: &S) -> &OsStr {
        s.as_ref()
    }

    pub fn from_bytes(bytes: &[u8]) -> &OsStr {
        // SAFETY: `OsStr` is a transparent `[u8]`.
        unsafe { &*(bytes as *const [u8] as *const OsStr) }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_encoded_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.0).ok()
    }

    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    pub fn to_os_string(&self) -> OsString {
        OsString(self.0.to_vec())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn display(&self) -> impl fmt::Display + '_ {
        self.to_string_lossy()
    }
}

impl OsString {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }

    pub fn as_os_str(&self) -> &OsStr {
        self
    }

    pub fn into_string(self) -> Result<String, OsString> {
        String::from_utf8(self.0).map_err(|e| OsString(e.into_bytes()))
    }

    pub fn push<T: AsRef<OsStr>>(&mut self, s: T) {
        self.0.extend_from_slice(&s.as_ref().0);
    }

    pub fn into_boxed_os_str(self) -> Box<OsStr> {
        let bytes = Box::into_raw(self.0.into_boxed_slice());
        // SAFETY: `OsStr` is a transparent `[u8]`.
        unsafe { Box::from_raw(bytes as *mut OsStr) }
    }
}

impl Deref for OsString {
    type Target = OsStr;
    fn deref(&self) -> &OsStr {
        OsStr::from_bytes(&self.0)
    }
}

impl Borrow<OsStr> for OsString {
    fn borrow(&self) -> &OsStr {
        self
    }
}

impl ToOwned for OsStr {
    type Owned = OsString;
    fn to_owned(&self) -> OsString {
        self.to_os_string()
    }
}

impl AsRef<OsStr> for OsStr {
    fn as_ref(&self) -> &OsStr {
        self
    }
}

impl AsRef<OsStr> for OsString {
    fn as_ref(&self) -> &OsStr {
        self
    }
}

impl AsRef<OsStr> for str {
    fn as_ref(&self) -> &OsStr {
        OsStr::from_bytes(self.as_bytes())
    }
}

impl AsRef<OsStr> for String {
    fn as_ref(&self) -> &OsStr {
        OsStr::from_bytes(self.as_bytes())
    }
}

impl From<String> for OsString {
    fn from(s: String) -> Self {
        Self(s.into_bytes())
    }
}

impl From<&str> for OsString {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl<T: ?Sized + AsRef<OsStr>> From<&T> for Box<OsStr> {
    fn from(s: &T) -> Self {
        s.as_ref().to_os_string().into_boxed_os_str()
    }
}

impl fmt::Debug for OsStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.to_string_lossy(), f)
    }
}

impl fmt::Debug for OsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

/// Byte access to OS strings, as std's `std::os::unix::ffi` has it.
pub trait OsStrExt {
    fn from_bytes(bytes: &[u8]) -> &Self;
    fn as_bytes(&self) -> &[u8];
}

impl OsStrExt for OsStr {
    fn from_bytes(bytes: &[u8]) -> &Self {
        OsStr::from_bytes(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
