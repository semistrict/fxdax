// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Stand-ins for the kernel's `fbl` containers.

/// A growable array whose insertions report allocation failure, as the
/// kernel's `fbl::Vector` does. Allocation failure aborts on the host, so
/// insertions always succeed.
#[derive(Debug, Default)]
pub struct Vector<T>(Vec<T>);

impl<T> Vector<T> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn push_back(&mut self, value: T) -> Result<(), std::collections::TryReserveError> {
        self.0.try_reserve(1)?;
        self.0.push(value);
        Ok(())
    }
}

impl<T> std::ops::Deref for Vector<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Vector<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<'a, T> IntoIterator for &'a Vector<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T> IntoIterator for Vector<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
