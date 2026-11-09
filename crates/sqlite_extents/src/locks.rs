// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SQLite's file locks, among the connections of one process: a volume's device is its process's
//! alone, so no other process takes them.
//!
//! A connection holds one of SQLite's levels: none, shared (reading), reserved (about to write),
//! pending (waiting for readers to go, keeping new ones out) or exclusive (writing). Any number
//! hold shared; one at most holds more.

/// SQLite's lock levels (`SQLITE_LOCK_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    None = 0,
    Shared = 1,
    Reserved = 2,
    Pending = 3,
    Exclusive = 4,
}

impl Level {
    pub fn from_sqlite(level: i32) -> Option<Self> {
        Some(match level {
            0 => Self::None,
            1 => Self::Shared,
            2 => Self::Reserved,
            3 => Self::Pending,
            4 => Self::Exclusive,
            _ => return None,
        })
    }
}

/// What a file's connections hold.
#[derive(Debug, Default)]
pub struct Locks {
    /// How many hold shared or more.
    shared: u32,
    /// The level of the one holding more than shared, if any.
    writer: Option<Level>,
}

impl Locks {
    /// Raises `held`, a connection's level, to `want`, or as far as it gets: whether it got there.
    /// Short of exclusive, it keeps pending, which keeps new readers out.
    pub fn lock(&mut self, held: &mut Level, want: Level) -> bool {
        if *held >= want {
            return true;
        }
        if *held == Level::None {
            if matches!(self.writer, Some(Level::Pending | Level::Exclusive)) {
                return false;
            }
            self.shared += 1;
            *held = Level::Shared;
        }
        if want == Level::Shared {
            return true;
        }
        if *held == Level::Shared {
            if self.writer.is_some() {
                return false;
            }
            *held = Level::Reserved;
            self.writer = Some(Level::Reserved);
        }
        if want == Level::Reserved {
            return true;
        }
        *held = match self.shared {
            1 => Level::Exclusive,
            _ => Level::Pending,
        };
        self.writer = Some(*held);
        *held == Level::Exclusive
    }

    /// Lowers `held` to `want`, shared or none.
    pub fn unlock(&mut self, held: &mut Level, want: Level) {
        assert!(want <= Level::Shared, "unlocking to {want:?}");
        if *held > Level::Shared {
            self.writer = None;
            *held = Level::Shared;
        }
        if want == Level::None && *held == Level::Shared {
            self.shared -= 1;
            *held = Level::None;
        }
    }

    /// Whether a connection holds more than shared.
    pub fn reserved(&self) -> bool {
        self.writer.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_share_and_a_writer_waits_for_them() {
        let mut locks = Locks::default();
        let (mut reader, mut writer) = (Level::None, Level::None);
        assert!(locks.lock(&mut reader, Level::Shared));
        assert!(locks.lock(&mut writer, Level::Reserved));
        assert!(locks.reserved());
        // Pending, kept, until the reader goes.
        assert!(!locks.lock(&mut writer, Level::Exclusive));
        assert_eq!(writer, Level::Pending);
        let mut late = Level::None;
        assert!(!locks.lock(&mut late, Level::Shared));
        locks.unlock(&mut reader, Level::None);
        assert!(locks.lock(&mut writer, Level::Exclusive));
        locks.unlock(&mut writer, Level::Shared);
        assert!(!locks.reserved());
        assert!(locks.lock(&mut late, Level::Shared));
    }

    #[test]
    fn one_writer_at_a_time() {
        let mut locks = Locks::default();
        let (mut one, mut two) = (Level::None, Level::None);
        assert!(locks.lock(&mut one, Level::Reserved));
        assert!(!locks.lock(&mut two, Level::Reserved));
        assert_eq!(two, Level::Shared);
        // A reader going straight to exclusive, as a hot journal's rollback does, waits too.
        assert!(!locks.lock(&mut two, Level::Exclusive));
        locks.unlock(&mut one, Level::None);
        assert!(locks.lock(&mut two, Level::Exclusive));
    }
}
