// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's host tool for VMs' disks, the extent stores extentd serves: makes them, of branches kept
//! in object storage (backlog doc-7) or of container images, places, forks, deletes and resizes
//! them, makes them public bases, and waits for their uploads:
//!
//! ```sh
//! fxdaxctl take DIR CAPACITY BRANCH [TOKEN]
//! fxdaxctl place DIR TOKEN
//! fxdaxctl sync DIR
//! fxdaxctl import DIR CAPACITY IMAGE [REF]
//! fxdaxctl fork SRC DST
//! fxdaxctl delete DIR
//! fxdaxctl resize DIR CAPACITY
//! fxdaxctl public DIR
//! ```
//!
//! With `FXDAX_EXTENTD_GRPC` naming the control socket of the extent daemon (TASK-43.5), every
//! command goes through its control API (`fxdax::extentd::control`), and acts on stores as the
//! daemon serves them: on a store a VM has open, live, or not at all (`EBUSY`). Without, fxdaxctl
//! acts on stores itself, as when building base images where no daemon runs: on stores nothing
//! has open, as their lock says, which an extent daemon serving them holds (it waits a while for
//! it, then refuses).
//!
//! `take` makes an empty store at DIR, of CAPACITY bytes, of BRANCH (hex): once extentd opens it,
//! with `FXDAX_EXTENTD_REMOTE` naming the branch's bucket, it takes what the branch holds there,
//! whose data it fetches as it is read; placed at TOKEN, if given, as `place` places it. `place`
//! places the store at DIR, or the fork whose handle it is, at the placement whose fencing token,
//! its boot ID, is TOKEN (TASK-39.7): its writer takes its branch in object storage with that token
//! from then on, which a later placement's exceeds, so that a placement cut off takes no branch
//! back from the one after it (`ESTALE`), and its commits fail once a later one takes it. `sync`
//! returns once every commit of the store at DIR is durable and in object storage, as before a
//! clean stop: through the daemon, a store a VM has open as of then; without it, with
//! `FXDAX_EXTENTD_REMOTE` naming its bucket. `import` makes a store at DIR, of CAPACITY bytes,
//! holding an Fxfs filesystem of the container image IMAGE (TASK-39.5), a base for VMs to boot
//! forks of: IMAGE is an OCI image layout, as `skopeo copy` makes, or `docker save` (whose tarball
//! is one, once extracted), holding the image REF names, or its only image; or a tar archive of one
//! layer, gzipped or not, as `docker export` makes. Files keep their owners, permissions, times
//! and extended attributes; see `fxdax::import`. `fork` copies the store at SRC to a new store at
//! DST, of a new branch, which it prints: through the daemon, live, if a VM has SRC open, once
//! nothing is open on it, as with the guest's filesystem frozen (fsfreeze), and refused otherwise
//! (TASK-39.3). With `FXDAX_EXTENTD_REMOTE`, the new branch is a fork of SRC's there, made once
//! SRC's commits are uploaded, holding its extents without uploading them again. DST is a handle
//! naming SRC's store, which holds the fork's data: SRC's directory must not be removed while forks
//! of it live. `delete` deletes a fork, or a store, whose data goes but for what other branches
//! hold: a store whose forks live keeps its directory, holding what they share of it, which no
//! longer opens, and which goes with the last of them (TASK-39.4); with `FXDAX_EXTENTD_REMOTE`, its
//! branch there too, its forks' living on. `resize` sets the store's capacity, the size of the disk
//! its guest sees, to CAPACITY bytes: through the daemon, a store its VM has open grows at once,
//! the guest seeing the new size while mounted, and shrinks if the guest's filesystem agrees, no
//! lower than what it holds; otherwise, it grows, and does not shrink. `public` makes the store at
//! DIR a public base, whose pages every tenant's forks of it share (TASK-43).

use anyhow::{Context as _, bail};
use fxdax::ExtentStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn usage() -> anyhow::Error {
    anyhow::anyhow!(
        "usage: fxdaxctl take DIR CAPACITY BRANCH [TOKEN] | fxdaxctl place DIR TOKEN | \
         fxdaxctl sync DIR | fxdaxctl import DIR CAPACITY IMAGE [REF] | fxdaxctl fork SRC DST | \
         fxdaxctl delete DIR | fxdaxctl resize DIR CAPACITY | fxdaxctl public DIR"
    )
}

/// What fxdaxctl was asked to do.
enum Command {
    Take { dir: PathBuf, capacity: u64, branch: u64, token: Option<u64> },
    Place { dir: PathBuf, token: u64 },
    Sync { dir: PathBuf },
    Import { dir: PathBuf, capacity: u64, image: PathBuf, reference: Option<String> },
    Fork { src: PathBuf, dst: PathBuf },
    Delete { dir: PathBuf },
    Resize { dir: PathBuf, capacity: u64 },
    Public { dir: PathBuf },
}

impl Command {
    fn parse(args: &[String]) -> anyhow::Result<Self> {
        let capacity = |capacity: &str| capacity.parse().context("CAPACITY must be a number");
        let token = |token: &str| token.parse().context("TOKEN must be a number");
        Ok(match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            ["take", dir, size, branch, ref rest @ ..] if rest.len() <= 1 => Self::Take {
                dir: dir.into(),
                capacity: capacity(size)?,
                branch: u64::from_str_radix(branch, 16).context("BRANCH must be hex")?,
                token: rest.first().map(|rest| token(rest)).transpose()?,
            },
            ["place", dir, given] => Self::Place { dir: dir.into(), token: token(given)? },
            ["sync", dir] => Self::Sync { dir: dir.into() },
            ["import", dir, size, image, ref rest @ ..] if rest.len() <= 1 => Self::Import {
                dir: dir.into(),
                capacity: capacity(size)?,
                image: image.into(),
                reference: rest.first().map(|reference| reference.to_string()),
            },
            ["fork", src, dst] => Self::Fork { src: src.into(), dst: dst.into() },
            ["delete", dir] => Self::Delete { dir: dir.into() },
            ["resize", dir, size] => Self::Resize { dir: dir.into(), capacity: capacity(size)? },
            ["public", dir] => Self::Public { dir: dir.into() },
            _ => bail!(usage()),
        })
    }

    /// Does it through the control API of the daemon whose control socket is at `socket`.
    #[cfg(target_os = "linux")]
    fn through(self, socket: &Path) -> anyhow::Result<()> {
        let daemon = fxdax::extentd::control::Client::connect(socket)?;
        match self {
            Self::Take { dir, capacity, branch, token } => {
                daemon.create_store(&dir, capacity, Some(branch), token)?;
            }
            Self::Place { dir, token } => daemon.place_store(&dir, token)?,
            Self::Sync { dir } => daemon.sync_store(&dir)?,
            Self::Import { dir, capacity, image, reference } => {
                daemon.import_image(&dir, capacity, &image, reference.as_deref())?;
            }
            Self::Fork { src, dst } => println!("{:x}", daemon.fork_store(&src, &dst)?),
            Self::Delete { dir } => daemon.delete_store(&dir)?,
            Self::Resize { dir, capacity } => daemon.resize_store(&dir, capacity)?,
            Self::Public { dir } => daemon.make_public(&dir)?,
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn through(self, _socket: &Path) -> anyhow::Result<()> {
        bail!("the extent daemon runs on Linux only")
    }

    /// Does it here, on stores nothing has open.
    fn locally(self) -> anyhow::Result<()> {
        match self {
            Self::Take { dir, capacity, branch, token } => {
                ExtentStore::create_for_branch(&dir, capacity, branch)?;
                if let Some(token) = token {
                    ExtentStore::place(&dir, token)?;
                }
            }
            Self::Place { dir, token } => ExtentStore::place(&dir, token)?,
            Self::Sync { dir } => {
                let bucket = bucket()?.context("FXDAX_EXTENTD_REMOTE names no bucket")?;
                let store = ExtentStore::open(&dir)?;
                attach(&store, bucket)?;
                store.wait_uploaded()?;
            }
            Self::Import { dir, capacity, image, reference } => {
                let made = fxdax::import::store(&dir, capacity, &image, reference.as_deref());
                futures::executor::block_on(made)
                    .with_context(|| format!("importing {}", image.display()))?;
            }
            Self::Fork { src, dst } => {
                let store = ExtentStore::open(&src)?;
                if let Some(bucket) = bucket()? {
                    attach(&store, bucket)?;
                }
                let branch = rand::random::<u64>().max(1);
                store.fork(&dst, branch)?;
                println!("{branch:x}");
            }
            Self::Delete { dir } => ExtentStore::delete(&dir, bucket()?.as_ref())?,
            Self::Resize { dir, capacity } => {
                let store = ExtentStore::open(&dir)?;
                if capacity < store.info()?.capacity {
                    bail!(
                        "shrinking a store needs its guest to agree: resize it through the \
                         extent daemon serving its VM (FXDAX_EXTENTD_GRPC)"
                    );
                }
                store.set_capacity(capacity)?;
            }
            Self::Public { dir } => ExtentStore::open(&dir)?.make_public()?,
        }
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = Command::parse(&args)?;
    match std::env::var_os("FXDAX_EXTENTD_GRPC").filter(|socket| !socket.is_empty()) {
        Some(socket) => command.through(socket.as_ref()),
        None => command.locally(),
    }
}

/// The bucket `FXDAX_EXTENTD_REMOTE` names, if it names one: the stores' branches' remote.
fn bucket() -> anyhow::Result<Option<extent_remote::store::Store>> {
    match std::env::var("FXDAX_EXTENTD_REMOTE") {
        Ok(url) if !url.is_empty() => {
            Ok(Some(extent_remote::store::Store::from_url(&url).map_err(anyhow::Error::msg)?))
        }
        _ => Ok(None),
    }
}

/// Keeps `store`'s branch in `bucket`, with this host's fencing token.
fn attach(store: &ExtentStore, bucket: extent_remote::store::Store) -> anyhow::Result<()> {
    let token = fxdax::remote::host_token()?;
    let options = fxdax::remote::Options { token, ..Default::default() };
    store.attach_remote(Arc::new(bucket), rand::random(), options)?;
    Ok(())
}
