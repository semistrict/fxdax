// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A volume: the files of SQLite's databases on an extent device, by name.
//!
//! **Commits.** A file's changes are committed alone ([`Volume::sync`]): its plan's extents are
//! written, then the root, naming every file's tree as the last commit of each left it, is
//! committed with the device's `commit_root`, a durable barrier. So a crash leaves each file as of
//! a commit of its own, whatever another file had written meanwhile. A commit that fails before
//! its root leaves the file as it was, the extents it wrote freed; one that fails committing the
//! root fails the volume, which may or may not hold it, until it is opened again.
//!
//! **Freeing.** What a commit leaves unreferenced is freed once its root is durable, and named in
//! that root as dying until then, so that the next opening frees it if a crash came between. The
//! root also says where minting goes on: the extents past it a crash left, written for a commit
//! whose root it lost, are freed by the next opening, as Fxfs's allocator frees its orphans.
//!
//! **Memory files.** Journals and temporary files live in memory: a database's file changes only
//! at its commits, whole, so that no crash needs a journal to roll it back.
//!
//! I/O to the device blocks the calling thread ([`futures::executor::block_on`]), as SQLite's
//! calls do: SQLite is not to be called from a thread whose executor the device needs.

use crate::format::{self, Root};
use crate::locks::{Level, Locks};
use crate::pages::Pages;
use anyhow::{Context as _, Error, bail};
use futures::executor::block_on;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use storage_device::{Device, DeviceHolder, ExtentId};

/// The most sequence numbers an opening frees from either end of what a crash may have left
/// (Fxfs's `MAX_ORPHANS_SOUGHT`).
const MAX_ORPHANS_SOUGHT: u64 = 1 << 16;

/// The most extents freed in one request.
const FREES_AT_ONCE: usize = 4096;

/// The error of opening a database file the volume doesn't hold.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the volume holds no file {0}")]
pub struct NotFound(pub String);

/// The error of using a volume whose commit failed as it committed its root: whether the device
/// holds that commit, its next opening finds.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the volume failed: {0}")]
pub struct Failed(pub String);

/// How a volume lays out what it writes.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The most blocks an extent holds: a commit writing more writes several.
    pub max_extent_blocks: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { max_extent_blocks: 1024 }
    }
}

/// A file of a volume.
struct File {
    pages: Pages,
    locks: Locks,
    /// How many connections have it open.
    opens: usize,
    /// Whether a commit made it: one not committed yet goes with its last opening's close.
    committed: bool,
}

/// A file in memory, a journal or a temporary file.
pub type MemoryFile = Arc<Mutex<Vec<u8>>>;

/// What a volume holds, under its lock.
struct Inner {
    files: BTreeMap<String, File>,
    /// The journals, by name. Temporary files have none.
    memory: HashMap<String, MemoryFile>,
    /// The sequence number the next extent minted gets.
    mint: u64,
    /// Extents left unreferenced whose frees failed, or have not been made yet.
    dying: Vec<ExtentId>,
    failed: Option<String>,
}

/// A volume on a device. See the module docs.
pub struct Volume {
    device: DeviceHolder,
    options: Options,
    inner: Mutex<Inner>,
}

/// Frees `extents` on `device`, a request at most per [`FREES_AT_ONCE`]: those it failed to free.
async fn free(device: &dyn Device, extents: &[ExtentId]) -> Vec<ExtentId> {
    let mut failed = Vec::new();
    for batch in extents.chunks(FREES_AT_ONCE) {
        for (extent, result) in batch.iter().zip(device.free_extents(batch).await) {
            if let Err(error) = result {
                log::warn!("sqlite_extents: freeing {extent:?}: {error:#}");
                failed.push(*extent);
            }
        }
    }
    failed
}

/// The sequence numbers between `minted` and `next` a crash may have left extents at: all of them,
/// or the first and last [`MAX_ORPHANS_SOUGHT`], as minting starts past a floor that may be far
/// past `minted`.
fn orphans(minted: u64, next: u64) -> impl Iterator<Item = u64> {
    let first = minted..minted.saturating_add(MAX_ORPHANS_SOUGHT).min(next.max(minted));
    let last = next.saturating_sub(MAX_ORPHANS_SOUGHT).max(first.end)..next;
    first.chain(last)
}

impl Volume {
    /// The volume on `device`, made if the device's root is empty.
    pub fn open(device: DeviceHolder, options: Options) -> Result<Arc<Self>, Error> {
        assert!(options.max_extent_blocks > 0, "extents of no blocks");
        let inner = block_on(Self::recover(&**device))?;
        Ok(Arc::new(Self { device, options, inner: Mutex::new(inner) }))
    }

    /// What `device` holds, its root's dying extents and what a crash left freed, and the root
    /// made if it was empty.
    async fn recover(device: &dyn Device) -> Result<Inner, Error> {
        format::check_block_size(device.block_size() as usize)?;
        let root = Root::decode(&device.root().await.context("reading the root")?)?;
        let next = device.next_sequence().await?;
        let floor = device.sequence_floor().await?;
        let mut inner = Inner {
            files: BTreeMap::new(),
            memory: HashMap::new(),
            mint: next.max(floor),
            dying: Vec::new(),
            failed: None,
        };
        let Some(root) = root else {
            let made = Root { minted: inner.mint, ..Root::default() };
            device.commit_root(&made.encode()).await.context("making the volume")?;
            return Ok(inner);
        };
        inner.mint = inner.mint.max(root.minted);
        for (name, file) in &root.files {
            let pages =
                Pages::open(device, file).await.with_context(|| format!("opening {name}"))?;
            inner.files.insert(
                name.clone(),
                File { pages, locks: Locks::default(), opens: 0, committed: true },
            );
        }
        let branch = device.branch();
        let orphans = orphans(root.minted, next);
        let freeing: Vec<_> = root
            .dying
            .iter()
            .copied()
            .chain(orphans.map(|sequence| ExtentId::new(branch, sequence)))
            .collect();
        if !freeing.is_empty() {
            inner.dying = free(device, &freeing).await;
            // Sealed before anything is written past them.
            device.flush().await.context("sealing the frees of what a crash left")?;
        }
        Ok(inner)
    }

    /// Closes the device: every commit is durable already, and the device may keep them
    /// elsewhere too, as an image uploads them to its bucket, which closing it waits for.
    pub fn close(&self) -> Result<(), Error> {
        let _inner = self.lock()?;
        block_on(self.device.close()).context("closing the device")
    }

    /// The device.
    pub fn device(&self) -> &DeviceHolder {
        &self.device
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>, Error> {
        let inner = self.inner.lock().unwrap();
        if let Some(failed) = &inner.failed {
            bail!(Failed(failed.clone()));
        }
        Ok(inner)
    }

    /// The files it holds, committed or open.
    pub fn files(&self) -> Result<Vec<String>, Error> {
        Ok(self.lock()?.files.keys().cloned().collect())
    }

    /// The extents its files' last commits reference, in order.
    pub fn extents(&self) -> Result<Vec<ExtentId>, Error> {
        let inner = self.lock()?;
        let mut extents: Vec<_> =
            inner.files.values().flat_map(|file| file.pages.extents()).collect();
        extents.sort();
        Ok(extents)
    }

    /// How many connections have its files open.
    pub fn opens(&self) -> usize {
        self.inner.lock().unwrap().files.values().map(|file| file.opens).sum()
    }

    /// Opens file `name`, made empty if it isn't there and `create`.
    pub fn open_file(&self, name: &str, create: bool) -> Result<(), Error> {
        let mut inner = self.lock()?;
        let block_size = self.device.block_size() as usize;
        let file = match inner.files.get_mut(name) {
            Some(file) => file,
            None if create => inner.files.entry(name.to_owned()).or_insert(File {
                pages: Pages::new(block_size),
                locks: Locks::default(),
                opens: 0,
                committed: false,
            }),
            None => bail!(NotFound(name.to_owned())),
        };
        file.opens += 1;
        Ok(())
    }

    /// Closes file `name`, opened once more than closed.
    pub fn close_file(&self, name: &str) {
        let mut inner = self.inner.lock().unwrap();
        let file = inner.files.get_mut(name).expect("a file open");
        file.opens -= 1;
        if file.pages.changed() {
            log::warn!("sqlite_extents: {name} closed with changes not committed, which are lost");
        }
        if file.opens == 0 && !file.committed {
            inner.files.remove(name);
        }
    }

    fn file<'a>(inner: &'a mut Inner, name: &str) -> &'a mut File {
        inner.files.get_mut(name).expect("a file open")
    }

    /// Reads file `name` into `buf` from `offset`, as far as it goes: how many bytes it read.
    pub fn read(&self, name: &str, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let mut inner = self.lock()?;
        let file = Self::file(&mut inner, name);
        block_on(file.pages.read(&**self.device, offset, buf))
    }

    pub fn write(&self, name: &str, offset: u64, data: &[u8]) -> Result<(), Error> {
        let mut inner = self.lock()?;
        let file = Self::file(&mut inner, name);
        block_on(file.pages.write(&**self.device, offset, data))
    }

    pub fn truncate(&self, name: &str, size: u64) -> Result<(), Error> {
        let mut inner = self.lock()?;
        let file = Self::file(&mut inner, name);
        block_on(file.pages.truncate(&**self.device, size))
    }

    pub fn size(&self, name: &str) -> Result<u64, Error> {
        let mut inner = self.lock()?;
        Ok(Self::file(&mut inner, name).pages.size())
    }

    /// Commits what changed in file `name` since its last commit, durably.
    pub fn sync(&self, name: &str) -> Result<(), Error> {
        let mut inner = self.lock()?;
        self.commit(&mut inner, name)
    }

    /// Raises a connection's lock of file `name`, `held`, to `want`: whether it got there.
    pub fn lock_file(&self, name: &str, held: &mut Level, want: Level) -> Result<bool, Error> {
        let mut inner = self.lock()?;
        Ok(Self::file(&mut inner, name).locks.lock(held, want))
    }

    /// Lowers a connection's lock of file `name`, `held`, to `want`, committing what it changed
    /// first if it was writing: SQLite syncs at a transaction's end unless told not to
    /// (`synchronous=OFF`), and the transaction is committed then.
    pub fn unlock_file(&self, name: &str, held: &mut Level, want: Level) -> Result<(), Error> {
        let mut inner = self.inner.lock().unwrap();
        let committed = match *held > Level::Shared && inner.failed.is_none() {
            true => self.commit(&mut inner, name),
            false => Ok(()),
        };
        Self::file(&mut inner, name).locks.unlock(held, want);
        committed
    }

    /// Whether a connection holds a lock of file `name` past shared.
    pub fn reserved(&self, name: &str) -> Result<bool, Error> {
        let mut inner = self.lock()?;
        Ok(Self::file(&mut inner, name).locks.reserved())
    }

    /// Whether file `name`, a database's or in memory, exists.
    pub fn exists(&self, name: &str) -> Result<bool, Error> {
        let inner = self.lock()?;
        Ok(inner.files.contains_key(name) || inner.memory.contains_key(name))
    }

    /// Deletes file `name`, a database's, durably, or in memory: whether it was there.
    pub fn delete(&self, name: &str) -> Result<bool, Error> {
        let mut inner = self.lock()?;
        if inner.memory.remove(name).is_some() {
            return Ok(true);
        }
        let Some(file) = inner.files.get(name) else { return Ok(false) };
        if file.opens > 0 {
            bail!("deleting {name}, open");
        }
        let file = inner.files.remove(name).unwrap();
        if file.committed {
            let dead: Vec<_> = file.pages.extents().collect();
            self.commit_root(&mut inner, None, dead)?;
        }
        Ok(true)
    }

    /// The journal `name`, made empty if it isn't there and `create`.
    pub fn memory_file(&self, name: &str, create: bool) -> Result<MemoryFile, Error> {
        let mut inner = self.lock()?;
        match inner.memory.get(name) {
            Some(file) => Ok(file.clone()),
            None if create => Ok(inner.memory.entry(name.to_owned()).or_default().clone()),
            None => bail!(NotFound(name.to_owned())),
        }
    }

    /// Commits file `name`'s changes, if any, or makes it if no commit has.
    fn commit(&self, inner: &mut Inner, name: &str) -> Result<(), Error> {
        let device = &**self.device;
        let (branch, mut next) = (device.branch(), inner.mint);
        let file = Self::file(inner, name);
        if !file.pages.changed() && file.committed {
            return Ok(());
        }
        let mint = || {
            next += 1;
            ExtentId::new(branch, next - 1)
        };
        let plan = block_on(file.pages.plan(device, mint, self.options.max_extent_blocks));
        // What it minted is spent, whatever came of it.
        inner.mint = next;
        let plan = plan?;
        if let Err(error) = block_on(plan.write(device)) {
            let written: Vec<_> = plan.extents().collect();
            let failed = block_on(free(device, &written));
            inner.dying.extend(failed);
            return Err(error.context(format!("committing {name}")));
        }
        self.commit_root(inner, Some((name, plan.root.clone())), plan.dead.clone())?;
        let file = Self::file(inner, name);
        file.pages.apply(plan);
        file.committed = true;
        Ok(())
    }

    /// Commits the root: each file committed as it is, but `changed`, if given, as it says, and
    /// `dead` freed once it is durable, with what earlier frees didn't free. A failure fails the
    /// volume.
    fn commit_root(
        &self,
        inner: &mut Inner,
        changed: Option<(&str, format::FileRoot)>,
        dead: Vec<ExtentId>,
    ) -> Result<(), Error> {
        let device = &**self.device;
        let mut files: BTreeMap<_, _> = (inner.files.iter())
            .filter(|(_, file)| file.committed)
            .map(|(name, file)| (name.clone(), file.pages.root()))
            .collect();
        if let Some((name, root)) = changed {
            files.insert(name.to_owned(), root);
        }
        let mut dying = std::mem::take(&mut inner.dying);
        dying.extend(dead);
        let root = Root { minted: inner.mint, dying, files };
        if let Err(error) = block_on(device.commit_root(&root.encode())) {
            let error = error.context("committing the volume's root");
            inner.failed = Some(format!("{error:#}"));
            return Err(error);
        }
        inner.dying = block_on(free(device, &root.dying));
        Ok(())
    }

    /// Fails unless the volume is usable: what its users see as an error of their own.
    pub fn check(&self) -> Result<(), Error> {
        self.lock().map(drop)
    }
}

impl std::fmt::Debug for Volume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock().map_err(|_| std::fmt::Error)?;
        f.debug_struct("Volume")
            .field("files", &inner.files.keys().collect::<Vec<_>>())
            .field("mint", &inner.mint)
            .field("failed", &inner.failed)
            .finish()
    }
}
