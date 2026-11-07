// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the host's disk a tenant takes (TASK-94), counted, and limited at runtime through the
//! control API, a tenant's and every tenant's together, as its memory is ([`super::memory`]).
//!
//! **What is counted.** A tenant takes what is in its directory, the one its VM processes name
//! stores in: its stores' files, whatever the store's local format keeps there (backlog doc-14: the
//! open segment and its log, the write-ahead log's closed segments not yet acknowledged in the
//! bucket, its own extents, the store's description), and whatever else is there, each file's
//! blocks once however many links it has, as `du -sx` counts them. One rule for what is shared: a
//! file is the tenant's whose directory holds it, and one in no tenant's directory is no tenant's.
//! A fork's directory holds what the fork wrote; what it shares of its base is in the bucket, and
//! read through the host's part cache (`FXDAX_EXTENTD_CACHE`), which is no tenant's: copies of the
//! bucket's objects that any tenant's stores read, a closed segment's too once acknowledged, which
//! its own capacity and floor bound, evicting what it must to keep within them. A tenant's reads
//! can't fill the disk through it, and no tenant's limit depends on what another read. The default
//! tenant, of the daemon's own socket, has no directory, and is not counted.
//!
//! **How.** Walking a tenant's directory counts it ([`Disk::recount`]): as it is granted, every
//! [`RECOUNT_EVERY`], soon after a write of it is refused, on a thread of the daemon's own, so that
//! what left its disk (uploaded, freed, a store deleted) lets it write again; and as the control
//! API asks (`GetTenantDisk`). Between walks, each write admitted adds its bytes, which a store
//! writes to its open segment as they come (log-structured, a rewrite too), and each store made
//! [`STORE_MADE`]; a walk counts with what it finds the writes not done as it began, which it may
//! not find. So a tenant's writes are counted as they are admitted, and what a store's own work
//! adds (closing and framing a segment, its log) as walks find it.
//!
//! **Limits.** Past its limit a tenant's writes fail with `ENOSPC`, and so do its VM processes'
//! makings of stores (an `OPEN` making one, a `FORK`), which its guest sees as a full disk; the
//! daemon goes on, and so do other tenants' writes. Past the limit of every tenant's together, any
//! tenant's do. Commits go on: what was written commits, uploads, and leaves the disk, as do
//! frees, which commits make. There are no limits until set. Its metrics name tenants by
//! [`super::memory::label`]s.
//!
//! **Not directory quotas.** A filesystem's quota of a tenant's directory (XFS project quotas,
//! ext4's) refuses whichever write of the directory's comes past it: a store closing and framing a
//! segment, which fails the store (`extent_store`'s `Phase::Failed`), or a flush appending a
//! commit's record, rather than the guest's next write, which the daemon refuses before the store
//! takes any of it. And it is the host's setup, a filesystem's that has them, which the daemon
//! can't set as tenants are granted.

use super::memory::label;
use crate::Errno;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// How often every tenant's disk is counted anew, at most.
pub const RECOUNT_EVERY: Duration = Duration::from_secs(10);

/// How long after one pass of counting the next begins, at least: a tenant refused writes, as it
/// writes again and again, has its directory walked ten times a second at most.
const RECOUNT_GAP: Duration = Duration::from_millis(100);

/// What a store a tenant's VM process makes is admitted as, before it is counted: a store's
/// files, a fork's made and an uploaded base's, take 12 to 16 KiB of a filesystem's 4 KiB blocks
/// (`tests/disk.rs`), four times which leaves room for larger blocks.
pub const STORE_MADE: u64 = 64 << 10;

/// No limit.
const NONE: u64 = u64::MAX;

/// The tenants' disks alive, and their limits, under one lock, so that a tenant made as its limit
/// is set is limited as it was set.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry { disks: Vec::new(), limits: None });

struct Registry {
    /// The disks of the tenants alive: a tenant revoked and granted again with another directory,
    /// while its earlier VM processes live on, has two for a while, each limited.
    disks: Vec<Weak<Disk>>,
    /// The tenants' limits, in bytes, by name: those granted later, or again, are limited as they
    /// are made.
    limits: Option<HashMap<String, u64>>,
}

impl Registry {
    fn alive(&self) -> impl Iterator<Item = Arc<Disk>> + '_ {
        self.disks.iter().filter_map(Weak::upgrade)
    }

    fn limit(&self, name: &str) -> Option<u64> {
        self.limits.as_ref().and_then(|limits| limits.get(name).copied())
    }
}

/// The limit of every tenant's disk together, or [`NONE`].
static TOTAL_LIMIT: AtomicU64 = AtomicU64::new(NONE);

/// What every tenant's disk alive takes together, as each counts it, and what is being admitted.
static USED: AtomicU64 = AtomicU64::new(0);

/// A tenant's disk: what its directory takes, as last counted, and what was admitted since.
pub struct Disk {
    name: String,
    dir: PathBuf,
    /// What it takes, in bytes: its directory's, as last walked, and the bytes of the writes
    /// admitted since that walk began, or not done as it began.
    used: AtomicU64,
    /// The bytes of the writes admitted and not done ([`Admitted`]): which a walk may not find in
    /// its files yet.
    writing: AtomicU64,
    /// Its limit, or [`NONE`].
    limit: AtomicU64,
    /// Whether its last write was refused: which is logged as it begins to be.
    full: AtomicBool,
    /// Whether it is to be counted anew soon, a write of it refused ([`Disk::nudge`]).
    nudged: AtomicBool,
    /// Held as it is counted, which one does at a time.
    counting: Mutex<()>,
}

impl Disk {
    /// The disk of the tenant `name`, whose directory is `dir`: counted now, among those alive,
    /// limited as its name is.
    pub(super) fn new(name: &str, dir: &Path) -> Arc<Self> {
        let disk = Arc::new(Self {
            name: name.to_owned(),
            dir: dir.to_owned(),
            used: AtomicU64::new(0),
            writing: AtomicU64::new(0),
            limit: AtomicU64::new(NONE),
            full: AtomicBool::new(false),
            nudged: AtomicBool::new(false),
            counting: Mutex::new(()),
        });
        disk.recount();
        let mut registry = REGISTRY.lock().unwrap();
        disk.limit.store(registry.limit(name).unwrap_or(NONE), Ordering::Relaxed);
        registry.disks.retain(|alive| alive.strong_count() > 0);
        registry.disks.push(Arc::downgrade(&disk));
        drop(registry);
        recounter();
        disk
    }

    /// What it takes, in bytes, as last counted, with what was admitted since.
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// Its limit, if it has one.
    pub fn limit(&self) -> Option<u64> {
        Some(self.limit.load(Ordering::Relaxed)).filter(|limit| *limit != NONE)
    }

    /// Whether its last write was refused, its limit or every tenant's reached.
    pub(super) fn is_full(&self) -> bool {
        self.full.load(Ordering::Relaxed)
    }

    /// Admits a write of `bytes` more, if they are within its limit, and every tenant's: counted
    /// until it is counted anew after the write is done, which the write says by dropping what
    /// this returns. `ENOSPC` otherwise, and it is counted anew soon, as what it took may have
    /// left the disk since.
    pub(super) fn admit(self: &Arc<Self>, bytes: u64) -> Result<Admitted, Errno> {
        let take = |used: &AtomicU64, limit: u64| {
            let within = |held: u64| held.checked_add(bytes).filter(|after| *after <= limit);
            used.try_update(Ordering::SeqCst, Ordering::SeqCst, within).is_ok()
        };
        // Writing before it is counted, so that a walk that counts it counts it as writing.
        self.writing.fetch_add(bytes, Ordering::SeqCst);
        let admitted = Admitted { disk: self.clone(), bytes };
        // Every tenant's first, given back if its own is past: its own only grows between walks,
        // which count from it ([`Disk::recount`]).
        if !take(&USED, TOTAL_LIMIT.load(Ordering::Relaxed)) {
            return Err(self.refuse());
        }
        if !take(&self.used, self.limit.load(Ordering::Relaxed)) {
            USED.fetch_sub(bytes, Ordering::SeqCst);
            return Err(self.refuse());
        }
        self.full.store(false, Ordering::Relaxed);
        Ok(admitted)
    }

    /// `ENOSPC`, a write refused: logged as it begins to be, and counted anew soon.
    fn refuse(&self) -> Errno {
        if !self.full.swap(true, Ordering::Relaxed) {
            log::warn!(
                "extentd: tenant {}'s disk at its limit, its writes refused (ENOSPC): it takes {} \
                 bytes, of {:?}; every tenant's, {} of {:?}",
                self.name,
                self.used(),
                self.limit(),
                total(),
                total_limit()
            );
        }
        self.nudge();
        Errno::ENOSPC
    }

    /// Counts it anew: walks its directory, what it takes then, with the writes not done as the
    /// walk began, and those admitted meanwhile, which the walk may not have found.
    pub(super) fn recount(&self) {
        let _counting = self.counting.lock().unwrap();
        let before = self.used.load(Ordering::SeqCst);
        let writing = self.writing.load(Ordering::SeqCst);
        let walked = walk(&self.dir);
        // Only admissions add to it meanwhile.
        let delta = (walked + writing).wrapping_sub(before);
        self.used.fetch_add(delta, Ordering::SeqCst);
        USED.fetch_add(delta, Ordering::SeqCst);
    }

    /// Has it counted anew soon, on the daemon's thread that counts disks ([`recounter`]).
    fn nudge(&self) {
        if !self.nudged.swap(true, Ordering::Relaxed) {
            let (nudged, wake) = &*NUDGED;
            *nudged.lock().unwrap() = true;
            wake.notify_one();
        }
    }
}

/// A write admitted to a tenant's disk ([`Disk::admit`]), until it is done: dropped.
pub(super) struct Admitted {
    disk: Arc<Disk>,
    bytes: u64,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.disk.writing.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        USED.fetch_sub(self.used(), Ordering::Relaxed);
    }
}

/// Whether a disk was nudged, and what tells the thread that counts disks so.
static NUDGED: std::sync::LazyLock<(Mutex<bool>, Condvar)> =
    std::sync::LazyLock::new(|| (Mutex::new(false), Condvar::new()));

/// Starts, once, the daemon's thread that counts tenants' disks anew: those nudged as they are,
/// and every one every [`RECOUNT_EVERY`]; a pass at most every [`RECOUNT_GAP`].
fn recounter() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let counting = std::thread::Builder::new().name("extentd disk".into()).spawn(|| {
            let mut every = Instant::now();
            loop {
                let (nudged, wake) = &*NUDGED;
                let waited = wake.wait_timeout_while(
                    nudged.lock().unwrap(),
                    RECOUNT_EVERY.saturating_sub(every.elapsed()),
                    |nudged| !*nudged,
                );
                *waited.unwrap().0 = false;
                let all = every.elapsed() >= RECOUNT_EVERY;
                if all {
                    every = Instant::now();
                }
                for disk in alive() {
                    if disk.nudged.swap(false, Ordering::Relaxed) || all {
                        disk.recount();
                    }
                }
                std::thread::sleep(RECOUNT_GAP);
            }
        });
        if let Err(err) = counting {
            log::error!("extentd: starting the thread that counts tenants' disks: {err}");
        }
    });
}

/// The disks of the tenants alive.
fn alive() -> Vec<Arc<Disk>> {
    REGISTRY.lock().unwrap().alive().collect()
}

/// The bytes of the filesystem's blocks the files under `dir` take, and its own, on its
/// filesystem alone, without following links, each file once however many links it has: what
/// `du -sx` says. What goes as it is walked is not counted; what can't be read is logged, and not
/// counted.
fn walk(dir: &Path) -> u64 {
    let root = match std::fs::symlink_metadata(dir) {
        Ok(root) => root,
        Err(err) => {
            log::warn!("extentd: counting {}: {err}", dir.display());
            return 0;
        }
    };
    let mut bytes = root.blocks() * 512;
    let mut linked = HashSet::new();
    let mut dirs = vec![dir.to_owned()];
    while let Some(dir) = dirs.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                log::warn!("extentd: counting {}: {err}", dir.display());
                continue;
            }
        };
        for entry in entries {
            let metadata = entry.and_then(|entry| Ok((entry.path(), entry.metadata()?)));
            let (path, metadata) = match metadata {
                Ok(found) => found,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => {
                    log::warn!("extentd: counting in {}: {err}", dir.display());
                    continue;
                }
            };
            if metadata.dev() != root.dev()
                || (metadata.nlink() > 1 && !linked.insert((metadata.dev(), metadata.ino())))
            {
                continue;
            }
            bytes += metadata.blocks() * 512;
            if metadata.is_dir() {
                dirs.push(path);
            }
        }
    }
    bytes
}

/// Limits the disk of the tenant `tenant` to `bytes`, or every tenant's together, without one; or
/// lifts the limit, without `bytes`. A tenant's limit holds for it alive now, and granted later,
/// until it is set again: one alive twice, revoked and granted again with another directory, has
/// it for each.
pub fn set_limit(tenant: Option<&str>, bytes: Option<u64>) -> Result<(), Errno> {
    let Some(name) = tenant else {
        TOTAL_LIMIT.store(bytes.unwrap_or(NONE), Ordering::Relaxed);
        return Ok(());
    };
    super::grants::check_name(name)?;
    let mut registry = REGISTRY.lock().unwrap();
    let limits = registry.limits.get_or_insert_with(HashMap::new);
    match bytes {
        Some(bytes) => limits.insert(name.to_owned(), bytes),
        None => limits.remove(name),
    };
    for disk in registry.alive().filter(|disk| disk.name == name) {
        disk.limit.store(bytes.unwrap_or(NONE), Ordering::Relaxed);
    }
    Ok(())
}

/// The limit of every tenant's disk together, if it has one.
pub fn total_limit() -> Option<u64> {
    Some(TOTAL_LIMIT.load(Ordering::Relaxed)).filter(|limit| *limit != NONE)
}

/// What every tenant's disk takes together.
pub fn total() -> u64 {
    USED.load(Ordering::Relaxed)
}

/// What the disk of the tenant `tenant` takes, or every tenant's together, without one, counted
/// now, and its limit, if it has one: `ENOENT` if no tenant `tenant` is alive. It blocks, as it
/// walks their directories.
pub fn count(tenant: Option<&str>) -> Result<(u64, Option<u64>), Errno> {
    let disks: Vec<_> =
        alive().into_iter().filter(|disk| tenant.is_none_or(|name| disk.name == name)).collect();
    disks.iter().for_each(|disk| disk.recount());
    let Some(name) = tenant else { return Ok((total(), total_limit())) };
    if disks.is_empty() {
        return Err(Errno::ENOENT);
    }
    let limit = REGISTRY.lock().unwrap().limit(name);
    Ok((disks.iter().map(|disk| disk.used()).sum(), limit))
}

/// A tenant's disk, as the metrics show it.
pub struct TenantDisk {
    /// Its [`label`].
    pub label: String,
    /// What it takes, in bytes, as last counted, with what was admitted since.
    pub bytes: u64,
    /// Its limit, if it has one.
    pub limit: Option<u64>,
}

/// The disk of each tenant alive, by label: a tenant alive twice, revoked and granted again with
/// another directory, is counted once, with both its directories'.
pub fn tenants() -> Vec<TenantDisk> {
    let registry = REGISTRY.lock().unwrap();
    let mut disks: HashMap<&str, TenantDisk> = HashMap::new();
    let alive: Vec<_> = registry.alive().collect();
    for disk in &alive {
        let entry = disks.entry(&disk.name).or_insert_with(|| TenantDisk {
            label: label(&disk.name),
            bytes: 0,
            limit: registry.limit(&disk.name),
        });
        entry.bytes += disk.used();
    }
    let mut disks: Vec<_> = disks.into_values().collect();
    disks.sort_by(|a, b| a.label.cmp(&b.label));
    disks
}

#[cfg(test)]
mod tests {
    use super::{Disk, set_limit};
    use crate::Errno;

    const MIB: u64 = 1 << 20;

    /// A walk that begins while a write admitted isn't done counts it, as its bytes may not be in
    /// the tenant's files yet: counted as the walk found them, writes in flight then were lost,
    /// and the tenant was admitted as many more, past its limit, as often as a refusal had it
    /// counted anew (found by the hostile proptest, mallory's fills: 2 MiB past its limit).
    #[test]
    fn writes_not_done_stay_counted_across_walks() {
        let dir = tempfile::tempdir().unwrap();
        let disk = Disk::new("acme", dir.path());
        let empty = disk.used();
        set_limit(Some("acme"), Some(empty + 2 * MIB)).unwrap();
        let writing = disk.admit(MIB).unwrap();
        disk.recount();
        assert_eq!(disk.used(), empty + MIB);
        assert_eq!(disk.admit(2 * MIB).map(drop), Err(Errno::ENOSPC));
        // Done, its bytes in a file of the tenant's: counted as the walk finds them.
        std::fs::write(dir.path().join("written"), vec![1; MIB as usize]).unwrap();
        drop(writing);
        disk.recount();
        assert_eq!(disk.used(), empty + MIB);
        assert_eq!(disk.admit(MIB).map(drop), Ok(()));
    }
}
