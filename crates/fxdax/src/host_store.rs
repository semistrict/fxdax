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
use crate::remote::{Failure, Remote};
use anyhow::Context as _;
use extent_remote::layout::Child;
use extent_remote::store::Store as Bucket;
use extent_store::{Description, HostDir, Options, Store};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The size of the blocks stores' extents are read and written in. Their capacities are multiples
/// of it.
pub(crate) const BLOCK_SIZE: u32 = 4096;

/// Whether stores' files bypass the host's page cache, where the host's file system allows: the
/// extent cache (`crate::cache`) caches their pages already.
const DIRECT: bool = true;

/// A new store's branch, or a fork's: random, so that stores made apart, and the forks of one,
/// mint extent IDs of their own (backlog doc-4).
pub(crate) fn new_branch() -> u64 {
    rand::random::<u64>().max(1)
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

/// Makes `bucket` this process's of `url`, before any store naming it opens, as the extent daemon's
/// tests do its own, which they put themselves in front of: fails if the process has one already.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn use_bucket(url: &str, bucket: Arc<Bucket>) -> anyhow::Result<()> {
    let mut buckets = BUCKETS.lock().unwrap();
    anyhow::ensure!(!buckets.contains_key(url), "this process has the bucket {url} already");
    buckets.insert(url.to_owned(), bucket);
    Ok(())
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

/// A store opened, with the bucket its description names, which its branch is kept in.
pub(crate) struct Opened {
    pub store: Store<HostDir>,
    pub bucket: Arc<Bucket>,
}

impl Opened {
    /// `store`, opened, with the bucket its description names, as every host's store's does
    /// (`extent_store::create_host`).
    fn of(store: Store<HostDir>) -> anyhow::Result<Self> {
        let named = store.bucket().map(str::to_owned);
        let url = named
            .ok_or_else(|| anyhow::anyhow!("{} names no bucket", store.dir().path().display()))?;
        let bucket = bucket(&url).with_context(|| format!("opening the bucket {url}"))?;
        Ok(Self { store, bucket })
    }
}

/// The bucket the store at `path` names, if its description is there: as deleting it, or what of it a deletion that stopped left, goes by. A store of
/// another format is refused (`extent_store::Format`).
pub(crate) fn bucket_at(path: &Path) -> anyhow::Result<Option<Arc<Bucket>>> {
    match extent_store::host_bucket(path) {
        Ok(Some(url)) => Ok(Some(bucket(&url)?)),
        Err(err) if err.is::<extent_store::Format>() => Err(err),
        Ok(None) | Err(_) => Ok(None),
    }
}

/// The options stores are opened with: their extents hold at most what the guest's device says
/// one does (doc-4's `max_extent`).
fn options() -> Options {
    Options { max_extent: crate::ExtentStore::MAX_EXTENT, ..Options::default() }
}

/// Makes a new store at `path`, which must not exist, holding at most `capacity` bytes, of
/// `branch`, kept in this host's bucket, or a file bucket of its own ([`bucket_for`]), and opens
/// it. Its directory entry is as durable as it is. A store that is not made whole is removed;
/// what is at `path` already is left as it is.
pub(crate) fn create(path: &Path, capacity: u64, branch: u64) -> anyhow::Result<Opened> {
    make(path, capacity, branch, None)
}

/// Makes a new store at `path` as [`create`] does, kept in the bucket at `bucket`.
#[cfg(test)]
pub(crate) fn create_in(
    path: &Path,
    capacity: u64,
    branch: u64,
    bucket: &str,
) -> anyhow::Result<Opened> {
    make(path, capacity, branch, Some(bucket))
}

/// Makes a new store at `path` as [`create`] does, kept in the bucket at `bucket`, if given, or
/// else at [`bucket_for`]'s.
fn make(path: &Path, capacity: u64, branch: u64, bucket: Option<&str>) -> anyhow::Result<Opened> {
    std::fs::create_dir(path).with_context(|| format!("creating {}", path.display()))?;
    let made = || -> anyhow::Result<Opened> {
        let description = Description { block_size: BLOCK_SIZE, capacity, branch };
        let bucket = bucket.map_or_else(|| bucket_for(path), |bucket| Ok(bucket.to_owned()))?;
        let store = extent_store::create_host(path, description, &bucket, DIRECT, options())?;
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
        std::fs::File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
        Opened::of(store)
    };
    made().inspect_err(|_| {
        // Leave nothing half made behind.
        let _ = std::fs::remove_dir_all(path);
    })
}

/// Opens the store at `path` as of its last commit, with the bucket it names.
pub(crate) fn open(path: &Path) -> anyhow::Result<Opened> {
    Opened::of(extent_store::open_host(path, DIRECT, options())?)
}

/// Whether `path` is a directory a store's making, or a fork's, left unmade, its host crashing:
/// no store, as its description, made last, is not there; deleting it finishes it ([`delete`]).
// The extent daemon's (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn unmade(path: &Path) -> bool {
    path.is_dir() && !path.join("store").exists()
}

/// Forks `store`, as of its last commit, made durable first, to `child`, a new branch holding what
/// it holds, whose store is a new directory at `path`, which [`open`] opens: `EEXIST` if anything is
/// at `path`. The fork is made in the bucket alone (doc-7, Forks; TASK-165.7): `remote`, the
/// store's uploader, uploads the store's commit naming the child ([`Store::commit_fork`]), whose
/// acknowledgement gives the child its record, inheriting the store's extents rather than copies;
/// `held`, given, is let go of once that commit is made, before its upload goes. Only then is the
/// child's store made, at `path`, taking the child's state from the bucket
/// (`crate::remote::take_branch`): every extent of it there, at its location there, read from
/// there, or the host's cache. `prepare` writes what the caller keeps beside the store into its
/// directory first, before its description, which makes it a store. A fork of a store with no
/// uploader fails with `ENOTSUP`, making nothing.
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
    remote: Option<&Remote>,
    path: &Path,
    child: Child,
    prepare: impl FnOnce(&Path) -> anyhow::Result<()>,
    held: H,
) -> Result<(), Error> {
    let branch = child.branch;
    let Some(remote) = remote else {
        let refused = format!(
            "forking to branch {branch:x}: a fork is the bucket's, and the store has no uploader"
        );
        return Err(Error::new(anyhow::Error::new(Errno::ENOTSUP).context(refused)));
    };
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
    remote: &Remote,
    path: &Path,
    child: Child,
    prepare: impl FnOnce(&Path) -> anyhow::Result<()>,
    held: H,
) -> Result<(), Error> {
    let branch = child.branch;
    let failed = |err: Failure| match err {
        Failure::Remote(extent_remote::Error::ChildTaken(_)) => taken(branch),
        Failure::PointDeleted => anyhow::Error::new(Errno::ENOENT)
            .context(format!("forking to branch {branch:x}: the point was deleted")),
        err => anyhow::anyhow!("forking branch {branch:x}: {err}"),
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
                    Err(err.context(format!("forking branch {branch:x}")))
                }
                // It made nothing.
                Err(err) => return Err(Error::new(err)),
            }
        }
    };
    let made = made.and_then(|()| {
        failpoint!("fork::recorded");
        make_child(store, remote, path, branch, prepare)
            .with_context(|| format!("forking to branch {branch:x}: making its store"))?;
        failpoint!("fork::made");
        Ok(())
    });
    let Err(err) = made else { return Ok(()) };
    // Leave no fork whose store went behind, as the fork's alone: its segment may land yet.
    if let Err(why) = remote.unfork(child) {
        log::warn!("fxdax: leaving branch {branch:x}, whose fork failed: {why}");
    }
    Err(Error::new(err))
}

/// The error of a fork to `branch`, whose ID another branch has in the bucket.
fn taken(branch: u64) -> anyhow::Error {
    let refused = format!("forking to branch {branch:x}: another branch has its ID");
    anyhow::Error::new(Errno::EEXIST).context(refused)
}

/// Makes the store of `branch`, a fork of `parent`'s branch the bucket has the record of, in the
/// directory at `path`, made empty: `prepare` writes into it first; then the store, of the parent's
/// capacity, naming the parent's bucket, taking what the bucket its uploader, `remote`, keeps the
/// branch in holds of the child.
fn make_child(
    parent: &Store<HostDir>,
    remote: &Remote,
    path: &Path,
    branch: u64,
    prepare: impl FnOnce(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    prepare(path)?;
    let capacity = parent.description().capacity;
    let url = parent.bucket().map(str::to_owned);
    let url =
        url.ok_or_else(|| anyhow::anyhow!("{} names no bucket", parent.dir().path().display()))?;
    let description = Description { block_size: BLOCK_SIZE, capacity, branch };
    let store = extent_store::create_host(path, description, &url, DIRECT, options())?;
    crate::remote::take_branch(&store, remote.bucket(), branch)?;
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
            let placed = crate::remote::placed_token(dir, branch)?;
            let token = placed.map_or_else(&token, Ok)?;
            crate::remote::delete_branch(bucket, branch, token)
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
    elsewhere: impl Fn(&Path, u64) -> Result<(), crate::remote::Failure>,
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
        let failure = err.chain().find_map(|cause| cause.downcast_ref::<crate::remote::Failure>());
        match failure.and_then(crate::remote::Failure::errno) {
            Some(errno) => Error::new(err.context(errno)),
            None => Error::new(err),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

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
        let Opened { store, bucket } = create(&made, MIB, 3).unwrap();
        let store = Arc::new(store);
        let options = crate::remote::Options { token: 1, ..Default::default() };
        let remote = Remote::attach(store.clone(), bucket.clone(), 1, options).unwrap();
        let child = Child { branch: 4, owner: 1, point: false };
        super::fork(&store, Some(&remote), &fork, child, |_| Ok(()), ()).unwrap();
        let own = dir.path().canonicalize().unwrap().join("made.bucket");
        let url = format!("file://{}/", own.display());
        for named in [&made, &fork] {
            assert_eq!(extent_store::host_bucket(named).unwrap(), Some(url.clone()));
        }
        assert!(keeps_own_bucket(&store), "a store's own bucket is a host's");
        // The bucket is this process's one of its URL, whose objects are in its directory.
        assert!(Arc::ptr_eq(&bucket, &self::bucket(&url).unwrap()));
        assert!(own.join("branches").exists(), "{:?}", std::fs::read_dir(&own).unwrap());
        // The store deleted, its fork's branch lives on in it.
        drop(remote);
        drop(store);
        delete(&made, Some(&bucket), || Ok(1)).unwrap();
        assert!(!made.exists() && own.exists());
        let record = futures::executor::block_on(extent_remote::branch::record(&*bucket, 4));
        assert_eq!(record.unwrap().map(|record| record.deleted), Some(false));
    }

    /// TASK-165.6: a store made by older code, of an older format, is refused by its format,
    /// opened or deleted: nothing reads it.
    #[test]
    fn older_stores_are_refused_by_their_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        drop(create(&path, MIB, 3).unwrap());
        let description = path.join("store");
        let text = std::fs::read_to_string(&description).unwrap();
        let older: String = std::iter::once("fxdax extent store 5")
            .chain(text.lines().skip(1).filter(|line| !line.starts_with("bucket ")))
            .map(|line| format!("{line}\n"))
            .collect();
        std::fs::write(&description, older).unwrap();

        let format = extent_store::Format { found: "fxdax extent store 5".to_owned() };
        let refused = open(&path).err().unwrap();
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
