// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extent stores in the host's directories, as both of fxdax's host stacks keep them: the images
//! [`Storage`](crate::Storage) mounts, and the stores [`ExtentStore`](crate::ExtentStore) serves a
//! guest. Either makes, opens, forks and deletes its stores here, so that both do it alike. Neither
//! cleans them: what their segments hold that no extent is in leaves with the segments, once
//! acknowledged in the bucket, whose collector takes it back (TASK-165.8).
//!
//! Every store keeps its branch in a bucket, which its description names (TASK-165.6): one made on
//! a host of fxdax's system, this host's (`FXDAX_EXTENTD_REMOTE`); one made elsewhere, as an image
//! of the fxdax CLI's or a test's is, a file bucket of its own, beside its directory, which its
//! forks share ([`create`]). Opening a store opens its bucket ([`Opened`]), the one this process
//! has of that URL ([`bucket`]). A store made by older code, of an older format, is refused
//! (`extent_store::Format`), opened or deleted alike: nothing reads or converts it.
//!
//! A store's directory holds its branch alone. Forks, points and deletions are the bucket's
//! (TASK-165.7): a fork is a commit of its parent's naming the child, made once its segment is
//! acknowledged, which gives the child its record; the child's directory is made only then, a
//! store of its own taking the child's state from the bucket ([`fork`]).

use crate::error::{Errno, Error};
use crate::faults::failpoint;
use anyhow::Context as _;
use extent_remote::layout::{Branch, Cell, Child};
use extent_remote::store::Store as Bucket;
use extent_remote::{branch, cell};
use extent_store::upload::{self, Failure, Remote};
use extent_store::{Description, HostDir, Options, Recovering, Store, Volume};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The size of the blocks stores' extents are read and written in. Their capacities are multiples
/// of it.
pub(crate) const BLOCK_SIZE: u32 = 4096;

/// Whether stores' files bypass the host's page cache, where the host's file system allows: the
/// extent cache (`crate::cache`) caches their pages already.
const DIRECT: bool = true;

/// The tenant of stores made for no tenant of the host's (TASK-364): the fxdax CLI's images, an
/// ungranted caller's stores, tests'.
pub(crate) const NO_TENANT: u64 = 0;

/// A new cell of `tenant`'s, for a store made alone or a fork (TASK-364): random, so that cells
/// made apart are apart.
pub fn new_cell(tenant: u64) -> Cell {
    Cell { tenant, id: rand::random::<u64>().max(1) }
}

/// The cell of a fork of `parent`, a new one of its tenant's, whose one volume the fork is.
pub(crate) fn fork_cell(parent: Branch) -> Cell {
    new_cell(parent.cell.tenant)
}

/// A new branch of `cell`, of `bucket`: its ID one no other branch of the bucket has, nor will
/// (`extent_remote::objectid`), so that stores made apart, and the forks of one, mint extent IDs
/// of their own (backlog doc-4), which a host's cache of extents keys them by (TASK-364).
pub(crate) fn new_branch(bucket: &Bucket, cell: Cell) -> Result<Branch, Failure> {
    let id = upload::block_on(bucket.new_objectid())?;
    Ok(Branch { cell, id })
}

/// A branch of a new cell of no tenant's, of a random ID no allocator gave: for tests that write
/// a branch's objects themselves, as no store does, or name a fork's child
/// (`ExtentStore::fork_to`).
#[cfg(test)]
pub(crate) fn new_alone() -> Branch {
    Branch { cell: new_cell(NO_TENANT), id: rand::random::<u64>().max(1) }
}

/// A child of a fork of `parent`, of a random ID no allocator gave, for tests that name it
/// (`ExtentStore::fork_to`): the one volume of a new cell of its tenant's. Random, not next to its
/// parent's: an allocator gives the IDs of its block in order, the next to another store.
#[cfg(test)]
pub(crate) fn fork_child(parent: Branch) -> Branch {
    Branch { cell: fork_cell(parent), id: rand::random::<u64>().max(1) }
}

/// The branch a store is made of (TASK-364).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchOf {
    /// A new branch of the cell, its ID the store's bucket's to give ([`new_branch`]).
    New(Cell),
    /// A branch the store's bucket has (`ENOENT` otherwise), which the store takes as it opens, or
    /// reads, if a point.
    Taken(Branch),
    /// A new branch of an ID a test chose, no allocator's: for tests that write extents of a
    /// branch they name.
    #[cfg(test)]
    Named(Branch),
}

/// The volume of its cell the store at `path` is, of `kind`: named as its directory is, which must
/// be a name a volume may have (`extent_remote::cell::valid_name`), else `EINVAL`.
pub(crate) fn volume_at(path: &Path, kind: cell::Kind) -> Result<Volume, Failure> {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    match cell::valid_name(name) {
        true => Ok(Volume { name: name.to_owned(), kind }),
        false => Err(extent_remote::Error::VolumeName(name.to_owned()).into()),
    }
}

/// What the name of the directory beside a store's that is its own file bucket ends with, where no
/// host's bucket is ([`create`]): beside it, not in it, as its forks keep their branches there,
/// which outlive it.
const OWN_BUCKET: &str = ".bucket";

/// The fencing token a copy of a store never placed takes its branch with where no placement names
/// one, as for the fxdax CLI's images ([`token`]): the first placement's.
const FIRST_PLACEMENT: u64 = 1;

/// The buckets this process keeps stores' branches in, by their URLs: one each, which every store
/// naming it shares, so that a memory bucket (`memory:///NAME`) is one for the process, as a
/// bucket elsewhere is one for every process.
static BUCKETS: Mutex<BTreeMap<String, Arc<Bucket>>> = Mutex::new(BTreeMap::new());

/// The bucket at `url` ([`Bucket::from_url`]): this process's, made if it has none yet.
pub(crate) fn bucket(url: &str) -> anyhow::Result<Arc<Bucket>> {
    let mut buckets = BUCKETS.lock().unwrap();
    if let Some(bucket) = buckets.get(url) {
        return Ok(bucket.clone());
    }
    let bucket = Arc::new(Bucket::from_url(url).map_err(anyhow::Error::msg)?);
    buckets.insert(url.to_owned(), bucket.clone());
    Ok(bucket)
}

/// Makes `bucket` this process's of `url`, before any store naming it opens, as the tests do the
/// buckets they put themselves in front of; unless it is already, the same.
#[cfg(test)]
pub(crate) fn use_bucket(url: &str, bucket: Arc<Bucket>) {
    let mut buckets = BUCKETS.lock().unwrap();
    let had = buckets.entry(url.to_owned()).or_insert_with(|| bucket.clone());
    assert!(Arc::ptr_eq(had, &bucket), "this process has another bucket at {url}");
}

/// The URL of the bucket a store made at `path`, a directory, keeps its branches in: this host's,
/// if it is a host of fxdax's system (`FXDAX_EXTENTD_REMOTE`, as [`crate::remote::Host`] has it);
/// else a file bucket of the store's own ([`bucket_named`]).
fn bucket_for(path: &Path) -> anyhow::Result<String> {
    let host = std::env::var("FXDAX_EXTENTD_REMOTE").ok().filter(|url| !url.is_empty());
    bucket_named(host, path)
}

/// The URL of the bucket a store made at `path`, a directory, keeps its branch in, on the host
/// whose bucket's URL is `host`, if it is one: that; or a file bucket of the store's own, beside
/// its directory ([`OWN_BUCKET`]), which its forks share, and which outlives it.
fn bucket_named(host: Option<String>, path: &Path) -> anyhow::Result<String> {
    if let Some(url) = host {
        return Ok(url);
    }
    let path = path.canonicalize()?;
    let name = path.file_name().ok_or_else(|| anyhow::anyhow!("{} has no name", path.display()))?;
    let own = path.with_file_name(format!("{}{OWN_BUCKET}", name.to_string_lossy()));
    let url = url::Url::from_directory_path(&own)
        .map_err(|()| anyhow::anyhow!("{} names no file bucket", own.display()))?;
    Ok(url.to_string())
}

/// Whether `store` keeps its branch in a file bucket no host's collector collects: one made beside
/// a store's directory, where no host's bucket was ([`bucket_named`]), its own or that of the
/// store it was forked from, which its forks share.
pub(crate) fn keeps_own_bucket(store: &Store<HostDir>) -> bool {
    let url = store.bucket().unwrap_or_default();
    url.starts_with("file:") && url.trim_end_matches('/').ends_with(OWN_BUCKET)
}

/// The fencing token copies of stores never placed take their branches with where this process
/// opens them as no host's daemon does, as [`crate::Storage`] does: this host's placement's, if it
/// is a host of fxdax's system (`FXDAX_EXTENTD_TOKEN`, [`crate::remote::host_token`]); else the
/// first placement's. So the daemon and the fxdax CLI on one host take each other's branches.
pub(crate) fn token() -> Result<u64, Failure> {
    match std::env::var_os("FXDAX_EXTENTD_TOKEN").filter(|token| !token.is_empty()) {
        Some(_) => crate::remote::host_token(),
        None => Ok(FIRST_PLACEMENT),
    }
}

/// How a store this process opens where no host's daemon serves it keeps its branch in its bucket:
/// its uploader's options, as a copy never placed takes its branch's lease here ([`token`]).
pub(crate) fn keeping() -> Result<upload::Options, Failure> {
    Ok(upload::Options { token: token()?, ..Default::default() })
}

/// A store opened ([`open`]), with the bucket its description names, which its branch is kept in,
/// and its uploader, which took the branch's lease, or holds its point (TASK-165.9).
pub(crate) struct Opened {
    pub store: Arc<Store<HostDir>>,
    pub bucket: Arc<Bucket>,
    pub remote: Remote<HostDir>,
}

impl Opened {
    /// The store, whose maker made it whole (TASK-196): opening it opens it from now on.
    pub fn made(&self) -> anyhow::Result<()> {
        self.store.made()
    }
}

/// The bucket the store at `path` names, if its description is there: as deleting it, or what of
/// it a deletion that stopped left, goes by. A store of another format is refused
/// (`extent_store::Format`).
pub(crate) fn bucket_at(path: &Path) -> anyhow::Result<Option<Arc<Bucket>>> {
    match extent_store::host_bucket(path) {
        Ok(Some(url)) => Ok(Some(bucket(&url)?)),
        Err(err) if err.is::<extent_store::Format>() => Err(err),
        Ok(None) | Err(_) => Ok(None),
    }
}

/// The options stores are opened with: their extents hold at most what the guest's device says
/// one does (doc-4's `max_extent`).
pub(crate) fn options() -> Options {
    Options { max_extent: crate::ExtentStore::MAX_EXTENT, ..Options::default() }
}

/// Makes a new store at `path`, which must not exist, holding at most `capacity` bytes, of the
/// branch `of` says, kept in this host's bucket, or a file bucket of its own ([`bucket_for`]), a
/// volume of `kind` of its branch's cell named as its directory is ([`volume_at`]), and opens it,
/// its uploader as `keeping` says ([`open`]): whole, as made. Its directory entry is as durable as
/// it is. A store that is not made whole is removed; what is at `path` already is left as it is.
pub(crate) fn create(
    path: &Path,
    capacity: u64,
    of: BranchOf,
    kind: cell::Kind,
    keeping: upload::Options,
) -> Result<Opened, Failure> {
    let opened = create_unmade(path, capacity, of, kind, None, keeping)?;
    opened.made()?;
    Ok(opened)
}

/// Makes a new store at `path` as [`create`] does, kept in the bucket at `bucket`, if given, or
/// else at [`bucket_for`]'s; unmade until its maker says it is whole ([`Opened::made`]), as an
/// import does once filled (TASK-196): opened before, it refuses (`extent_store::Unmade`), and
/// [`unmade`] says it is no store.
pub(crate) fn create_unmade(
    path: &Path,
    capacity: u64,
    of: BranchOf,
    kind: cell::Kind,
    bucket: Option<&str>,
    keeping: upload::Options,
) -> Result<Opened, Failure> {
    let volume = volume_at(path, kind)?;
    std::fs::create_dir(path)
        .map_err(|err| Failure::local(err, format!("creating {}", path.display())))?;
    let made = || -> Result<Opened, Failure> {
        let url = bucket.map_or_else(|| bucket_for(path), |bucket| Ok(bucket.to_owned()))?;
        let branch = match of {
            BranchOf::New(cell) => new_branch(&*self::bucket(&url)?, cell)?,
            BranchOf::Taken(branch) => {
                let found = upload::block_on(branch::record(&*self::bucket(&url)?, branch))??;
                found.ok_or(extent_remote::Error::NoBranch)?;
                branch
            }
            #[cfg(test)]
            BranchOf::Named(branch) => branch,
        };
        let description = Description { block_size: BLOCK_SIZE, capacity, branch };
        extent_store::create_host(path, description, &url, volume, DIRECT)?;
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
        let synced = std::fs::File::open(parent.unwrap_or(Path::new(".")))
            .and_then(|parent| parent.sync_all());
        synced.map_err(|err| Failure::local(err, "syncing a store's parent"))?;
        let recovering = Store::recover_unmade(host_dir(path)?)?;
        opened(recovering, keeping)
    };
    made().inspect_err(|_| {
        // Leave nothing half made behind.
        let _ = std::fs::remove_dir_all(path);
    })
}

/// Opens the store at `path` as of its last commit, with the bucket it names: its uploader, as
/// `keeping` says, takes its branch's lease, with the token its placement names or `keeping`'s,
/// and recovers it, the store's write-ahead log put after its end (`extent_store::upload::open`,
/// TASK-165.9). Fails as that does: a log another writer's commits moved past (`ESTALE`), a branch
/// deleted (`ENOENT`) or a later placement's (`ESTALE`); and a store never made whole (`Unmade`).
pub(crate) fn open(path: &Path, keeping: upload::Options) -> Result<Opened, Failure> {
    let recovering = Store::recover(host_dir(path)?)?;
    opened(recovering, keeping)
}

/// The directory of the store at `path`.
fn host_dir(path: &Path) -> Result<HostDir, Failure> {
    HostDir::new(path, DIRECT).map_err(|err| Failure::local(err, format!("{}", path.display())))
}

/// Opens the store at `path` as [`open`] does, its write-ahead log dropped if another writer's
/// commits moved its branch past it (`extent_remote::Error::Diverged`): what `fxdaxctl take` does
/// of a store that refuses to open so, the commits the log held, never acknowledged, lost (doc-14,
/// Opening a store; doc-7, Recovery on the same host).
pub(crate) fn take(path: &Path, keeping: upload::Options) -> Result<Opened, Failure> {
    match open(path, keeping.clone()) {
        Err(Failure::Remote(extent_remote::Error::Diverged)) => {
            let mut recovering = Store::recover(host_dir(path)?)?;
            log::warn!(
                "fxdax: dropping {}'s write-ahead log: another writer moved branch {} past it",
                path.display(),
                recovering.branch()
            );
            recovering.drop_wal()?;
            opened(recovering, keeping)
        }
        opened => opened,
    }
}

/// The store `recovering` recovered, opened with its uploader as `keeping` says.
fn opened(recovering: Recovering<HostDir>, keeping: upload::Options) -> Result<Opened, Failure> {
    let url = recovering.bucket().map(str::to_owned);
    let path = || recovering_path(&recovering);
    let url = url.ok_or_else(|| Failure::msg(format!("{} names no bucket", path())))?;
    let bucket = bucket(&url).with_context(|| format!("opening the bucket {url}"))?;
    let runtime = crate::remote::runtime();
    let holding = (rand::random(), keeping);
    let (store, remote) = upload::open(recovering, options(), bucket.clone(), holding, &runtime)?;
    Ok(Opened { store, bucket, remote })
}

/// The path of the directory `recovering` is of, for what fails of it.
fn recovering_path(recovering: &Recovering<HostDir>) -> String {
    format!("branch {}'s store", recovering.branch())
}

/// Whether `path` is a directory a store's making, or a fork's, left unmade, its host crashing:
/// no store, as its description, made last, is not there, or it was never made whole
/// (`extent_store::unmade_host`, TASK-196); deleting it finishes it ([`delete`]).
pub(crate) fn unmade(path: &Path) -> bool {
    extent_store::unmade_host(path)
}

/// Forks `store`, as of its last commit, made durable first, to `child`, a new branch holding what
/// it holds, whose store is a new directory at `path`, which [`open`] opens: `EEXIST` if anything is
/// at `path`. The fork is made in the bucket alone (doc-7, Forks; TASK-165.7): `remote`, the
/// store's uploader, uploads the store's commit naming the child ([`Store::commit_fork`]), whose
/// acknowledgement gives the child its record, inheriting the store's extents rather than copies;
/// `held`, given, is let go of once that commit is made, before its upload goes. Only then is the
/// child's store made, at `path`, holding nothing of its own: opening it recovers the child's
/// state from the bucket, as any store's opening does (TASK-165.9), every extent of it there, at
/// its location there, read from there, or the host's cache. `prepare` writes what the caller keeps beside the store into its
/// directory first, before its description, which makes it a store. A fork of a store with no
/// uploader fails with `ENOTSUP`, making nothing: its callers attach one first, an image's
/// (`storage::kept`), and an extent store's opened where no daemon serves it
/// (`ExtentStore::kept_in_bucket`).
///
/// The caller names the child: a fork to a child whose ID another branch has in the bucket fails
/// with `EEXIST`, making nothing; and so does one whose child's ID another branch took as it was
/// made, its commit standing, which names a child its parent's writers leave be (TASK-79.82). A
/// fork that fails once its commit was made, its upload failing, or the store's, is deleted in the
/// bucket as the fork's alone (`Remote::unfork`), so that its segment, landing late, makes nothing
/// (TASK-79.78); its host crashing, it stands, or never was, as the bucket has it.
///
/// A fork of a point commits nothing: its child's record is the fork, which fails with `ENOENT`
/// once the point is deleted (`Remote::forked`).
pub(crate) fn fork<H>(
    store: &Store<HostDir>,
    remote: &Remote<HostDir>,
    path: &Path,
    child: Child,
    prepare: impl FnOnce(&Path) -> anyhow::Result<()>,
    held: H,
) -> Result<(), Error> {
    std::fs::create_dir(path).map_err(|err| match err.kind() {
        std::io::ErrorKind::AlreadyExists => {
            Error::new(anyhow::Error::new(err).context(Errno::EEXIST))
        }
        _ => Error::new(anyhow::Error::new(err).context(format!("creating {}", path.display()))),
    })?;
    let forked = fork_in(store, remote, path, child, prepare, held);
    if forked.is_err() {
        // Leave nothing half made behind.
        let _ = std::fs::remove_dir_all(path);
    }
    forked
}

/// Forks `store` to `child` as [`fork`] does, into the directory at `path`, made empty.
fn fork_in<H>(
    store: &Store<HostDir>,
    remote: &Remote<HostDir>,
    path: &Path,
    child: Child,
    prepare: impl FnOnce(&Path) -> anyhow::Result<()>,
    held: H,
) -> Result<(), Error> {
    let branch = child.branch;
    let failed = |err: Failure| match err {
        Failure::Remote(extent_remote::Error::IdTaken(_)) => taken(branch),
        Failure::PointDeleted => anyhow::Error::new(Errno::ENOENT)
            .context(format!("forking to branch {branch}: the point was deleted")),
        err => anyhow::anyhow!("forking to branch {branch}: {err}"),
    };
    if !remote.child_free(branch).map_err(|err| Error::new(failed(err)))? {
        return Err(Error::new(taken(branch)));
    }
    let made = match remote.is_point() {
        true => {
            drop(held);
            remote.forked(0, child).map_err(failed)
        }
        false => {
            let committed = store.commit_fork(child);
            drop(held);
            match committed {
                Ok(epoch) => remote.forked(epoch, child).map_err(failed),
                Err(err) if err.downcast_ref::<extent_store::ForkCommitted>().is_some() => {
                    Err(err.context(format!("forking to branch {branch}")))
                }
                // It made nothing.
                Err(err) => return Err(Error::new(err)),
            }
        }
    };
    let made = made.and_then(|()| {
        failpoint!("fork::recorded");
        make_child(store, path, branch, prepare)
            .with_context(|| format!("forking to branch {branch}: making its store"))?;
        failpoint!("fork::made");
        Ok(())
    });
    let Err(err) = made else { return Ok(()) };
    // Leave no fork whose store went behind, as the fork's alone: its segment may land yet.
    if let Err(why) = remote.unfork(child) {
        log::warn!("fxdax: leaving branch {branch}, whose fork failed: {why}");
    }
    Err(Error::new(err))
}

/// The error of a fork to `branch`, whose ID another branch has in the bucket.
fn taken(branch: Branch) -> anyhow::Error {
    let refused = format!("forking to branch {branch}: another branch has its ID");
    anyhow::Error::new(Errno::EEXIST).context(refused)
}

/// Makes the store of `branch`, a fork of `parent`'s branch the bucket has the record of, in the
/// directory at `path`, made empty: `prepare` writes into it first; then the store's directory, of
/// the parent's capacity, naming the parent's bucket, the volume of its cell the parent is of its
/// own, whole (TASK-364). It holds nothing of its own: opening it recovers the child from the
/// bucket, as any store's opening does (TASK-165.9), and holds its cell, making it.
fn make_child(
    parent: &Store<HostDir>,
    path: &Path,
    branch: Branch,
    prepare: impl FnOnce(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    prepare(path)?;
    let capacity = parent.description().capacity;
    let named = || anyhow::anyhow!("{} names no bucket", parent.dir().path().display());
    let url = parent.bucket().map(str::to_owned).ok_or_else(named)?;
    let volume = parent.volume().cloned().ok_or_else(|| {
        anyhow::anyhow!("{} is no volume of a cell", parent.dir().path().display())
    })?;
    let description = Description { block_size: BLOCK_SIZE, capacity, branch };
    extent_store::create_host(path, description, &url, volume, DIRECT)?;
    extent_store::made_host(path)?;
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
    std::fs::File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
    Ok(())
}

/// Deletes the store at `path`, which nothing has open: with `bucket`, its branch there first, its
/// forks' branches living on, as the placement its copy was placed at, or, never placed, as the one
/// `token` gives; then its directory. A deletion that stopped partway is finished by deleting
/// again (TASK-79.71), and a directory a store's or a fork's making left unmade ([`unmade`]) goes.
///
/// Fails with `ENOENT` if nothing is at `path`, and with `ESTALE` if a later placement of the
/// store holds its branch in `bucket`.
pub(crate) fn delete(
    path: &Path,
    bucket: Option<&Bucket>,
    token: impl Fn() -> Result<u64, Failure>,
) -> Result<(), Error> {
    // As this copy's placement, or this host's, deletes it: not from under a later one (`ESTALE`).
    delete_with(path, |dir, branch| match bucket {
        Some(bucket) => {
            let placed = extent_store::host_token(dir)?;
            let token = placed.map_or_else(&token, Ok)?;
            extent_store::upload::delete_branch(bucket, branch, token)
        }
        None => Ok(()),
    })
}

/// Deletes the store at `path` as [`delete`] does, `elsewhere` deleting its branch where else it
/// is kept, given the store's directory and the branch: once the store is found open nowhere here,
/// before it goes here (`extent_store::delete_host`), so that a deletion that stopped partway, the
/// host crashing or `elsewhere` failing, leaves the store still, which deleting again finishes,
/// never a branch kept elsewhere alone. What a store's making, or a fork's, left unmade, which is
/// no store, goes, `elsewhere` called for nothing.
pub(crate) fn delete_with(
    path: &Path,
    elsewhere: impl Fn(&Path, Branch) -> Result<(), Failure>,
) -> Result<(), Error> {
    if !path.exists() {
        return Err(Errno::ENOENT.into());
    }
    let deleting = |branch| -> anyhow::Result<()> {
        failpoint!("delete::elsewhere");
        Ok(elsewhere(path, branch)?)
    };
    extent_store::delete_host(path, deleting).map_err(|err| {
        // Its errno, if deleting the branch elsewhere failed with one: `ESTALE` for a branch a
        // later placement holds.
        let failure = err.chain().find_map(|cause| cause.downcast_ref::<Failure>());
        match failure.and_then(crate::remote::errno) {
            Some(errno) => Error::new(err.context(errno)),
            None => Error::new(err),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    /// A new branch of a new cell of no tenant's.
    fn new() -> BranchOf {
        BranchOf::New(new_cell(NO_TENANT))
    }

    /// TASK-196: a store its maker never made whole is no store: it refuses to open, and
    /// [`unmade`] says so, which deleting it, or making it anew, goes by; once made, it opens.
    #[test]
    fn stores_unmade_open_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        let made = create_unmade(&path, MIB, new(), cell::Kind::Fxfs, None, keeping().unwrap());
        let branch = made.unwrap().store.description().branch;
        assert!(unmade(&path));
        let refused = open(&path, keeping().unwrap()).err().unwrap();
        let Failure::Local(refused) = refused else { panic!("{refused}") };
        assert_eq!(refused.downcast_ref::<extent_store::Unmade>(), Some(&extent_store::Unmade));
        extent_store::made_host(&path).unwrap();
        assert!(!unmade(&path));
        assert_eq!(
            open(&path, keeping().unwrap()).map(|opened| opened.store.description().branch).ok(),
            Some(branch)
        );
    }

    /// TASK-364: a new store's branch's ID is its bucket's to give, one of a block this process
    /// took there, the next store's the next of it; a store of a branch the bucket has not is
    /// refused (`NoBranch`), and leaves nothing.
    #[test]
    fn new_branches_ids_are_their_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (dir.path().join("one"), dir.path().join("two"));
        let url = format!("memory:///new-branches-{}", std::process::id());
        let made = |path: &Path, of| {
            let made =
                create_unmade(path, MIB, of, cell::Kind::Fxfs, Some(&url), keeping().unwrap());
            made.unwrap().store.description().branch
        };
        let first = made(&one, new());
        let second = made(&two, new());
        assert_eq!(second.id, first.id + 1);
        let block = first.id / extent_remote::objectid::BLOCK;
        let bucket = bucket(&url).unwrap();
        let taken = extent_remote::layout::objectid_block(block);
        assert!(upload::block_on(extent_remote::bucket::get(&*bucket, &taken)).unwrap().is_some());

        let absent = Branch { cell: new_cell(NO_TENANT), id: second.id + 1 };
        let path = dir.path().join("absent");
        let refused = create_unmade(
            &path,
            MIB,
            BranchOf::Taken(absent),
            cell::Kind::Fxfs,
            Some(&url),
            keeping().unwrap(),
        );
        let Err(Failure::Remote(refused)) = refused.map(drop) else { panic!("made") };
        assert_eq!(refused, extent_remote::Error::NoBranch);
        assert!(!path.exists(), "a store of no branch was left");
    }

    /// TASK-165.6: a store made on a host of fxdax's system names the host's bucket; one made
    /// elsewhere, a file bucket of its own, beside its directory, where its branch is kept: its
    /// forks' too, which name it. Beside it, not in it, so that it outlives the store, its forks'
    /// branches living on (TASK-165.7).
    #[test]
    fn stores_name_their_hosts_bucket_or_one_of_their_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        std::fs::create_dir(&path).unwrap();
        let host = Some("gs://bucket/prefix".to_owned());
        assert_eq!(bucket_named(host, &path).unwrap(), "gs://bucket/prefix");
        let own = dir.path().canonicalize().unwrap().join("store.bucket");
        let url = format!("file://{}/", own.display());
        assert_eq!(bucket_named(None, &path).unwrap(), url);

        // Where no host's bucket is, as in this test's process.
        let (made, fork) = (dir.path().join("made"), dir.path().join("fork"));
        let Opened { store, bucket, remote } =
            create(&made, MIB, new(), cell::Kind::Fxfs, keeping().unwrap()).unwrap();
        let forked = new_branch(&bucket, fork_cell(store.description().branch)).unwrap();
        let child = Child { branch: forked, owner: 1, point: false };
        super::fork(&store, &remote, &fork, child, |_| Ok(()), ()).unwrap();
        let own = dir.path().canonicalize().unwrap().join("made.bucket");
        let url = format!("file://{}/", own.display());
        for named in [&made, &fork] {
            assert_eq!(extent_store::host_bucket(named).unwrap(), Some(url.clone()));
        }
        assert!(keeps_own_bucket(&store), "a store's own bucket is a host's");
        // The bucket is this process's one of its URL, whose objects are in its directory.
        assert!(Arc::ptr_eq(&bucket, &self::bucket(&url).unwrap()));
        // fxdax: under its cell's prefix (TASK-364).
        let cell = own.join(store.description().branch.cell.prefix());
        assert!(cell.join("branches").exists(), "{:?}", std::fs::read_dir(&own).unwrap());
        // The store deleted, its fork's branch lives on in it.
        drop(remote);
        drop(store);
        delete(&made, Some(&bucket), || Ok(1)).unwrap();
        assert!(!made.exists() && own.exists());
        let record = futures::executor::block_on(extent_remote::branch::record(&*bucket, forked));
        assert_eq!(record.unwrap().map(|record| record.deleted), Some(false));
    }

    /// TASK-165.6: a store made by older code, of an older format, is refused by its format,
    /// opened or deleted: nothing reads it.
    #[test]
    fn older_stores_are_refused_by_their_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        drop(create(&path, MIB, new(), cell::Kind::Fxfs, keeping().unwrap()).unwrap());
        let description = path.join("store");
        let text = std::fs::read_to_string(&description).unwrap();
        let older: String = std::iter::once("fxdax extent store 5")
            .chain(text.lines().skip(1).filter(|line| !line.starts_with("bucket ")))
            .map(|line| format!("{line}\n"))
            .collect();
        std::fs::write(&description, older).unwrap();

        let format = extent_store::Format { found: "fxdax extent store 5".to_owned() };
        let refused = open(&path, keeping().unwrap()).err().unwrap();
        let Failure::Local(refused) = refused else { panic!("{refused}") };
        assert_eq!(refused.downcast_ref::<extent_store::Format>(), Some(&format), "{refused:#}");
        let refused = crate::Storage::open(&path).err().unwrap();
        assert_eq!(refused.errno(), Some(Errno::ENOTSUP), "{refused:#}");
        let refused = crate::Storage::delete(&path).err().unwrap();
        assert_eq!(refused.errno(), Some(Errno::ENOTSUP), "{refused:#}");
        assert!(path.join("store").exists());
    }

    /// A store's directory whose deletion stopped once its description was gone, or whose making
    /// stopped before it began, empty, is removed by deleting it again.
    #[test]
    fn deleting_an_emptied_store_finishes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fork");
        std::fs::create_dir(&path).unwrap();
        assert!(unmade(&path));
        delete(&path, None, token).unwrap();
        assert!(!path.exists());
    }

    /// TASK-79.20: a store whose description was never put in place, its host crashing as its
    /// making, or a fork's, wrote what goes before it, is no store, and is removed by deleting it,
    /// as extentd and fxdaxctl delete it.
    #[test]
    fn deleting_a_store_never_described_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fork");
        std::fs::create_dir_all(path.join("segments")).unwrap();
        std::fs::write(path.join("ancestry"), "3 /elsewhere\n").unwrap();
        assert!(unmade(&path));
        delete(&path, None, token).unwrap();
        assert!(!path.exists());
    }
}
