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
//! fxdaxctl stat DIR
//! fxdaxctl --version
//! ```
//!
//! With `FXDAX_EXTENTD_GRPC` naming the control socket of the extent daemon (TASK-43.5), every
//! command goes through its control API (`fxdax::extentd::control`), and acts on stores as the
//! daemon serves them: on a store a VM has open, live, or not at all (`EBUSY`). Without, fxdaxctl
//! acts on stores itself, on stores nothing has open, as their lock says, which an extent daemon
//! serving them holds (it waits a while for it, then refuses). `take` and `import`, which make new
//! stores, work so anywhere, as when building base images where no daemon runs. The others change
//! existing stores' branches, which are in a bucket: they need the bucket (`FXDAX_EXTENTD_REMOTE`)
//! and this host's placement's fencing token (`FXDAX_EXTENTD_TOKEN`), and refuse otherwise
//! (`fxdax::remote::Host`, TASK-45).
//!
//! `take` makes an empty store at DIR, of CAPACITY bytes, of BRANCH (hex): once extentd opens it,
//! with `FXDAX_EXTENTD_REMOTE` naming the branch's bucket, it takes what the branch holds there,
//! whose data it fetches as it is read; placed at TOKEN, if given, as `place` places it. `place`
//! places the store at DIR, or the fork whose handle it is, at the placement whose fencing token,
//! its boot ID, is TOKEN (TASK-39.7): its writer takes its branch in object storage with that token
//! from then on, which a later placement's exceeds, so that a placement cut off takes no branch
//! back from the one after it (`ESTALE`), and its commits fail once a later one takes it. `sync`
//! returns once every commit of the store at DIR is durable and in object storage, as before a
//! clean stop: through the daemon, a store a VM has open as of then. `import` makes a store at DIR, of CAPACITY bytes,
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
//! DIR a public base, whose pages every tenant's forks of it share (TASK-43). `stat` prints what the
//! store at DIR is and holds, one `name value` a line: its branch (hex), capacity, used and
//! available bytes, and of those used, its own (its branch wrote them) and inherited (from the
//! branches it was forked from), what billing it alone charges it and what it shares (TASK-49);
//! whether it is public, and, through the daemon, open. `--version` prints the commit it was built
//! of (`fxdax::BUILD_COMMIT`, TASK-111.7).

use anyhow::{Context as _, bail};
use fxdax::ExtentStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn usage() -> anyhow::Error {
    anyhow::anyhow!(
        "usage: fxdaxctl take DIR CAPACITY BRANCH [TOKEN] | fxdaxctl place DIR TOKEN | \
         fxdaxctl sync DIR | fxdaxctl import DIR CAPACITY IMAGE [REF] | fxdaxctl fork SRC DST | \
         fxdaxctl delete DIR | fxdaxctl resize DIR CAPACITY | fxdaxctl public DIR | \
         fxdaxctl stat DIR | fxdaxctl --version"
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
    Stat { dir: PathBuf },
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
            ["stat", dir] => Self::Stat { dir: dir.into() },
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
            Self::Stat { dir } => {
                let stat = daemon.stat_store(&dir)?;
                let lines = stat_lines(
                    stat.branch,
                    stat.capacity,
                    stat.used,
                    stat.available,
                    stat.own,
                    stat.inherited,
                    stat.public,
                    Some(stat.open),
                );
                print!("{lines}");
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn through(self, _socket: &Path) -> anyhow::Result<()> {
        bail!("the extent daemon runs on Linux only")
    }

    /// Does it here, on stores nothing has open: new ones anywhere; existing ones with the bucket
    /// and a placement token.
    fn locally(self) -> anyhow::Result<()> {
        // What stat reads needs no branch changed.
        let existing = !matches!(self, Self::Take { .. } | Self::Import { .. } | Self::Stat { .. });
        let bucket = match existing.then(fxdax::remote::Host::from_env).transpose() {
            Ok(Some(host)) => Some(
                extent_remote::store::Store::from_url(&host.bucket).map_err(anyhow::Error::msg)?,
            ),
            Ok(None) => None,
            Err(why) => bail!(
                "without the extent daemon (FXDAX_EXTENTD_GRPC), fxdaxctl changes no existing \
                 store's branch here: {why}"
            ),
        };
        match self {
            Self::Take { dir, capacity, branch, token } => {
                ExtentStore::create_for_branch(&dir, capacity, branch)?;
                if let Some(token) = token {
                    ExtentStore::place(&dir, token)?;
                }
            }
            Self::Place { dir, token } => ExtentStore::place(&dir, token)?,
            Self::Sync { dir } => {
                let bucket = bucket.context("an existing store's sync has its bucket")?;
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
                if let Some(bucket) = bucket {
                    attach(&store, bucket)?;
                }
                let branch = rand::random::<u64>().max(1);
                store.fork(&dst, branch)?;
                println!("{branch:x}");
            }
            Self::Delete { dir } => ExtentStore::delete(&dir, bucket.as_ref())?,
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
            Self::Stat { dir } => {
                let store = ExtentStore::open(&dir)?;
                let (info, bytes) = (store.info()?, store.bytes()?);
                let lines = stat_lines(
                    store.branch(),
                    info.capacity,
                    info.used,
                    info.available,
                    bytes.own,
                    bytes.inherited,
                    store.is_public(),
                    None,
                );
                print!("{lines}");
            }
        }
        Ok(())
    }
}

/// What `stat` prints, one `name value` a line; whether the store is open, if known.
#[allow(clippy::too_many_arguments)]
fn stat_lines(
    branch: u64,
    capacity: u64,
    used: u64,
    available: u64,
    own: u64,
    inherited: u64,
    public: bool,
    open: Option<bool>,
) -> String {
    let mut lines = format!(
        "branch {branch:x}\ncapacity {capacity}\nused {used}\navailable {available}\nown {own}\n\
         inherited {inherited}\npublic {public}\n"
    );
    if let Some(open) = open {
        lines += &format!("open {open}\n");
    }
    lines
}

fn main() -> anyhow::Result<()> {
    // fxdax: its threads' stacks on SIGQUIT, as an import that hung needed (TASK-98, TASK-44.13).
    #[cfg(target_os = "linux")]
    fxdax::stacks::on_sigquit()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    // fxdax: the commit it was built of (TASK-111.7).
    if args == ["--version"] {
        println!("fxdaxctl {}", fxdax::BUILD_COMMIT);
        return Ok(());
    }
    let command = Command::parse(&args)?;
    match std::env::var_os("FXDAX_EXTENTD_GRPC").filter(|socket| !socket.is_empty()) {
        Some(socket) => command.through(socket.as_ref()),
        None => command.locally(),
    }
}

/// Keeps `store`'s branch in `bucket`, with this host's fencing token.
fn attach(store: &ExtentStore, bucket: extent_remote::store::Store) -> anyhow::Result<()> {
    let token = fxdax::remote::host_token()?;
    let options = fxdax::remote::Options { token, ..Default::default() };
    store.attach_remote(Arc::new(bucket), rand::random(), options)?;
    Ok(())
}
