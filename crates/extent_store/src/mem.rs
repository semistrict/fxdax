// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store's directory in memory, which simulates crashes, for tests.
//!
//! It keeps what a host's file system would have made durable: a file's contents as of its last
//! sync, and a directory's entries as of its last sync. A crash ([`MemDir::crash`]) keeps that,
//! and any of what happened since, at random: each block of each write since the file's last sync,
//! and each file created or removed since the directory's last sync, in order. Directories
//! themselves are durable once created. [`MemDir::fail_after`] makes the file system fail every
//! operation that would change it after a number of them, as a crash would interrupt them.

use crate::dir::{Dir, File};
use crate::sync::Mutex;
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::Arc;

/// The granularity of a crash: a write survives it in blocks of this size, each whole or not at
/// all.
const BLOCK: usize = 512;

/// A store's directory in memory. See the module docs.
#[derive(Clone, Default)]
pub struct MemDir(Arc<Mutex<Fs>>);

/// A file of a [`MemDir`].
pub struct MemFile {
    fs: Arc<Mutex<Fs>>,
    inode: u64,
}

#[derive(Default)]
struct Fs {
    inodes: HashMap<u64, Inode>,
    next_inode: u64,
    /// Each directory's entries, by its path: the files' names, with their inodes.
    dirs: BTreeMap<String, BTreeMap<String, u64>>,
    /// The same, as durable.
    durable_dirs: BTreeMap<String, BTreeMap<String, u64>>,
    /// The entries created and removed in each directory since its last sync, in order.
    dir_changes: BTreeMap<String, Vec<DirChange>>,
    /// How many more operations that change the file system succeed, if limited.
    budget: Option<u64>,
}

#[derive(Clone, Default)]
struct Inode {
    data: Vec<u8>,
    /// The data as of the last sync.
    durable: Vec<u8>,
    /// The writes since the last sync, in order: their offsets and data.
    writes: Vec<(usize, Vec<u8>)>,
}

#[derive(Clone)]
enum DirChange {
    Add(String, u64),
    Remove(String),
}

fn not_found(path: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{path} doesn't exist"))
}

/// Splits `path` into its directory's path and its name.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

impl Fs {
    /// Spends one of the operations the budget allows, failing if none are left.
    fn spend(&mut self) -> io::Result<()> {
        match &mut self.budget {
            Some(0) => Err(io::Error::other("the file system crashed")),
            Some(budget) => {
                *budget -= 1;
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn lookup(&self, path: &str) -> io::Result<u64> {
        let (dir, name) = split(path);
        self.dirs
            .get(dir)
            .and_then(|entries| entries.get(name))
            .copied()
            .ok_or_else(|| not_found(path))
    }

    fn change_dir(&mut self, path: &str, change: DirChange) -> io::Result<()> {
        let (dir, _) = split(path);
        let entries = self.dirs.get_mut(dir).ok_or_else(|| not_found(dir))?;
        match &change {
            DirChange::Add(name, inode) => {
                if entries.insert(name.clone(), *inode).is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{path} exists"),
                    ));
                }
            }
            DirChange::Remove(name) => {
                entries.remove(name).ok_or_else(|| not_found(path))?;
            }
        }
        self.dir_changes.entry(dir.to_owned()).or_default().push(change);
        Ok(())
    }
}

impl MemDir {
    /// An empty directory.
    pub fn new() -> Self {
        let dir = Self::default();
        {
            let mut fs = dir.0.lock().unwrap();
            fs.dirs.insert(String::new(), BTreeMap::new());
            fs.durable_dirs.insert(String::new(), BTreeMap::new());
        }
        dir
    }

    /// Makes every operation that changes the file system fail after `operations` more of them,
    /// or none if `None`.
    pub fn fail_after(&self, operations: Option<u64>) {
        self.0.lock().unwrap().budget = operations;
    }

    /// Whether the file system failed an operation, or would fail the next, having spent its
    /// budget (see `fail_after`).
    pub fn has_failed(&self) -> bool {
        self.0.lock().unwrap().budget == Some(0)
    }

    /// The directory as a crash would leave it: what was durable, and any of what happened since,
    /// chosen at random by an RNG seeded with `seed`. See the module docs.
    pub fn crash(&self, seed: u64) -> MemDir {
        let mut rng = StdRng::seed_from_u64(seed);
        let fs = self.0.lock().unwrap();
        let mut dirs = BTreeMap::new();
        for (path, durable) in &fs.durable_dirs {
            let mut entries = durable.clone();
            for change in fs.dir_changes.get(path).into_iter().flatten() {
                if rng.random::<bool>() {
                    match change {
                        DirChange::Add(name, inode) => {
                            entries.insert(name.clone(), *inode);
                        }
                        DirChange::Remove(name) => {
                            entries.remove(name);
                        }
                    }
                }
            }
            dirs.insert(path.clone(), entries);
        }
        let mut inodes = HashMap::new();
        for inode in dirs.values().flat_map(|entries| entries.values()) {
            if inodes.contains_key(inode) {
                continue;
            }
            let file = &fs.inodes[inode];
            let mut data = file.durable.clone();
            for (offset, written) in &file.writes {
                for (i, block) in written.chunks(BLOCK).enumerate() {
                    if rng.random::<bool>() {
                        let start = offset + i * BLOCK;
                        if data.len() < start + block.len() {
                            data.resize(start + block.len(), 0);
                        }
                        data[start..start + block.len()].copy_from_slice(block);
                    }
                }
            }
            inodes.insert(*inode, Inode { durable: data.clone(), data, writes: Vec::new() });
        }
        MemDir(Arc::new(Mutex::new(Fs {
            inodes,
            next_inode: fs.next_inode,
            durable_dirs: dirs.clone(),
            dirs,
            dir_changes: BTreeMap::new(),
            budget: None,
        })))
    }
}

impl Dir for MemDir {
    type File = MemFile;

    fn create(&self, path: &str, _data: bool) -> io::Result<MemFile> {
        let mut fs = self.0.lock().unwrap();
        fs.spend()?;
        let inode = fs.next_inode;
        let (_, name) = split(path);
        fs.change_dir(path, DirChange::Add(name.to_owned(), inode))?;
        fs.next_inode += 1;
        fs.inodes.insert(inode, Inode::default());
        Ok(MemFile { fs: self.0.clone(), inode })
    }

    fn open(&self, path: &str, _data: bool) -> io::Result<MemFile> {
        let inode = self.0.lock().unwrap().lookup(path)?;
        Ok(MemFile { fs: self.0.clone(), inode })
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        let mut fs = self.0.lock().unwrap();
        fs.spend()?;
        let (_, name) = split(path);
        fs.change_dir(path, DirChange::Remove(name.to_owned()))
    }

    fn create_dir(&self, path: &str) -> io::Result<()> {
        let mut fs = self.0.lock().unwrap();
        fs.spend()?;
        if fs.dirs.contains_key(path) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{path} exists")));
        }
        fs.dirs.insert(path.to_owned(), BTreeMap::new());
        fs.durable_dirs.insert(path.to_owned(), BTreeMap::new());
        Ok(())
    }

    fn list(&self, path: &str) -> io::Result<Vec<String>> {
        let fs = self.0.lock().unwrap();
        let mut names: Vec<String> =
            fs.dirs.get(path).ok_or_else(|| not_found(path))?.keys().cloned().collect();
        // And its directories, as a host's file system lists them.
        names.extend(
            (fs.dirs.keys())
                .filter_map(|dir| dir.rsplit_once('/').or(Some(("", dir.as_str()))))
                .filter(|(parent, name)| *parent == path && !name.is_empty())
                .map(|(_, name)| name.to_owned()),
        );
        names.sort();
        Ok(names)
    }

    fn sync_dir(&self, path: &str) -> io::Result<()> {
        let mut fs = self.0.lock().unwrap();
        fs.spend()?;
        let entries = fs.dirs.get(path).ok_or_else(|| not_found(path))?.clone();
        fs.durable_dirs.insert(path.to_owned(), entries);
        fs.dir_changes.remove(path);
        Ok(())
    }

    fn barrier(&self) -> io::Result<()> {
        self.0.lock().unwrap().spend()
    }
}

impl File for MemFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let fs = self.fs.lock().unwrap();
        let data = &fs.inodes[&self.inode].data;
        let offset = offset as usize;
        let held = data.len().saturating_sub(offset).min(buf.len());
        buf[..held].copy_from_slice(&data[offset..offset + held]);
        buf[held..].fill(0);
        Ok(())
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<()> {
        let mut fs = self.fs.lock().unwrap();
        fs.spend()?;
        let inode = fs.inodes.get_mut(&self.inode).unwrap();
        let offset = offset as usize;
        if inode.data.len() < offset + buf.len() {
            inode.data.resize(offset + buf.len(), 0);
        }
        inode.data[offset..offset + buf.len()].copy_from_slice(buf);
        inode.writes.push((offset, buf.to_vec()));
        Ok(())
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.fs.lock().unwrap().inodes[&self.inode].data.len() as u64)
    }

    fn sync(&self) -> io::Result<()> {
        let mut fs = self.fs.lock().unwrap();
        fs.spend()?;
        let inode = fs.inodes.get_mut(&self.inode).unwrap();
        inode.durable = inode.data.clone();
        inode.writes.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(file: &MemFile) -> Vec<u8> {
        let mut data = vec![0; file.size().unwrap() as usize];
        file.read_at(&mut data, 0).unwrap();
        data
    }

    #[test]
    fn crashes_keep_what_was_synced() {
        let dir = MemDir::new();
        dir.create_dir("d").unwrap();
        let synced = dir.create("d/synced", true).unwrap();
        synced.write_at(&[1; BLOCK], 0).unwrap();
        synced.sync().unwrap();
        dir.sync_dir("d").unwrap();
        // Neither the file nor its entry is synced.
        let unsynced = dir.create("d/unsynced", true).unwrap();
        unsynced.write_at(&[2; BLOCK], 0).unwrap();
        synced.write_at(&[3; 4 * BLOCK], BLOCK as u64).unwrap();

        let mut seen_unsynced = [false; 2];
        let mut lens = std::collections::BTreeSet::new();
        for seed in 0..64 {
            let crashed = dir.crash(seed);
            let synced = crashed.open("d/synced", true).unwrap();
            let data = read(&synced);
            assert_eq!(data[..BLOCK], [1; BLOCK]);
            // Each block written since the sync is there whole, or not at all.
            for block in data[BLOCK..].chunks(BLOCK) {
                assert!(block == [3; BLOCK] || block == [0; BLOCK], "{block:?}");
            }
            lens.insert(data.len());
            seen_unsynced[crashed.list("d").unwrap().len() - 1] = true;
            assert_eq!(crashed.list("").unwrap(), ["d"]);
        }
        assert_eq!(seen_unsynced, [true, true]);
        assert!(lens.len() > 2, "{lens:?}");
    }

    #[test]
    fn fails_after_its_budget() {
        let dir = MemDir::new();
        let file = dir.create("f", false).unwrap();
        dir.fail_after(Some(2));
        file.write_at(b"a", 0).unwrap();
        file.sync().unwrap();
        assert!(file.write_at(b"b", 1).is_err());
        assert!(dir.create("g", false).is_err());
        assert!(dir.has_failed());
        // Reads still work.
        assert_eq!(read(&file), b"a");
    }
}
