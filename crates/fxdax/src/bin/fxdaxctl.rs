// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's host tool for VMs' disks, the extent stores extentd serves: makes them, of branches kept
//! in object storage (backlog doc-7) or of container images, places, forks, deletes and resizes
//! them, and waits for their uploads:
//!
//! ```sh
//! fxdaxctl take DIR CAPACITY BRANCH [TOKEN]
//! fxdaxctl place DIR TOKEN
//! fxdaxctl sync DIR
//! fxdaxctl import DIR CAPACITY IMAGE [REF]
//! fxdaxctl fork SRC DST
//! fxdaxctl delete DIR
//! fxdaxctl resize DIR CAPACITY
//! ```
//!
//! `take` makes an empty store at DIR, of CAPACITY bytes, of BRANCH (hex): once extentd opens it,
//! with `FXDAX_EXTENTD_REMOTE` naming the branch's bucket, it takes what the branch holds there,
//! whose data it fetches as it is read; placed at TOKEN, if given, as `place` places it. `place`
//! places the store at DIR, or the fork whose handle it is, at the placement whose fencing token,
//! its boot ID, is TOKEN (TASK-39.7): its writer takes its branch in object storage with that token
//! from then on, which a later placement's exceeds, so that a placement cut off takes no branch
//! back from the one after it (`ESTALE`), and its commits fail once a later one takes it. `sync`
//! returns once every commit of the store at DIR is durable and in object storage, as before a
//! clean stop: with `FXDAX_EXTENTD`, through the daemon, a store a VM has open as of then; without,
//! the store at DIR, which nothing may have open, with `FXDAX_EXTENTD_REMOTE` naming its bucket.
//! `import` makes a store at DIR, of CAPACITY bytes, holding
//! an Fxfs filesystem of the container image IMAGE (TASK-39.5), a base for VMs to boot forks of:
//! IMAGE is an OCI image layout, as `skopeo copy` makes, or `docker save` (whose tarball is one,
//! once extracted), holding the image REF names, or its only image; or a tar archive of one layer,
//! gzipped or not, as `docker export` makes. Files keep their owners, permissions, times and
//! extended attributes; see `fxdax::import`. `fork` copies the store at SRC to a new store at DST, of a
//! new branch, which it prints: with `FXDAX_EXTENTD` naming the extent daemon's socket, through
//! it, which forks a store a VM has open live, once nothing is open on it, as with the guest's
//! filesystem frozen (fsfreeze), and refuses otherwise (TASK-39.3); without, SRC, which nothing may
//! have open. With `FXDAX_EXTENTD_REMOTE`, the new
//! branch is a fork of SRC's there, made once SRC's commits are uploaded, holding its extents
//! without uploading them again. DST is a handle naming SRC's store, which holds the fork's data:
//! SRC's directory must not be removed while forks of it live. `delete` deletes a fork, or a
//! store, whose data goes but for what other branches hold: a store whose forks live keeps its
//! directory, holding what they share of it, which no longer opens, and which goes with the last
//! of them (TASK-39.4); with `FXDAX_EXTENTD`, through the daemon, which refuses a store a VM has
//! open; with `FXDAX_EXTENTD_REMOTE`, its branch there too, its forks' living on. `resize` sets the
//! store's capacity, the size of the disk its guest sees, to CAPACITY bytes: with `FXDAX_EXTENTD`
//! naming the socket of the extent daemon, through it, which grows a store its VM has open at once,
//! the guest seeing the new size while mounted, and shrinks it if the guest's filesystem agrees, no
//! lower than what it holds; without, it grows the store at DIR, which nothing may have open, and
//! does not shrink it.

use anyhow::{Context as _, bail};
use fxdax::ExtentStore;
use std::path::Path;
use std::sync::Arc;

fn usage() -> anyhow::Error {
    anyhow::anyhow!(
        "usage: fxdaxctl take DIR CAPACITY BRANCH [TOKEN] | fxdaxctl place DIR TOKEN | \
         fxdaxctl sync DIR | fxdaxctl import DIR CAPACITY IMAGE [REF] | fxdaxctl fork SRC DST | \
         fxdaxctl delete DIR | fxdaxctl resize DIR CAPACITY"
    )
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["take", dir, capacity, branch, ref token @ ..] if token.len() <= 1 => {
            let capacity = capacity.parse().context("CAPACITY must be a number")?;
            let branch = u64::from_str_radix(branch, 16).context("BRANCH must be hex")?;
            ExtentStore::create_for_branch(dir, capacity, branch)?;
            if let Some(token) = token.first() {
                ExtentStore::place(dir, token.parse().context("TOKEN must be a number")?)?;
            }
            Ok(())
        }
        ["place", dir, token] => {
            Ok(ExtentStore::place(dir, token.parse().context("TOKEN must be a number")?)?)
        }
        ["sync", dir] => {
            match std::env::var_os("FXDAX_EXTENTD").filter(|socket| !socket.is_empty()) {
                #[cfg(target_os = "linux")]
                Some(socket) => Ok(fxdax::extentd::wait_uploaded(socket, dir)?),
                #[cfg(not(target_os = "linux"))]
                Some(_) => bail!("the extent daemon runs on Linux only"),
                None => {
                    let bucket = bucket()?.context("FXDAX_EXTENTD_REMOTE names no bucket")?;
                    let store = ExtentStore::open(dir)?;
                    let token = fxdax::remote::host_token()?;
                    let options = fxdax::remote::Options { token, ..Default::default() };
                    store.attach_remote(Arc::new(bucket), rand::random(), options)?;
                    Ok(store.wait_uploaded()?)
                }
            }
        }
        ["import", dir, capacity, image, ref reference @ ..] if reference.len() <= 1 => {
            let capacity = capacity.parse().context("CAPACITY must be a number")?;
            let image = Path::new(image);
            let made = fxdax::import::store(dir, capacity, image, reference.first().copied());
            Ok(futures::executor::block_on(made)
                .with_context(|| format!("importing {}", image.display()))?)
        }
        ["fork", src, dst] => {
            // A store the daemon serves, through it: live, if a VM has it open (TASK-39.3).
            if let Some(socket) =
                std::env::var_os("FXDAX_EXTENTD").filter(|socket| !socket.is_empty())
            {
                #[cfg(target_os = "linux")]
                {
                    let branch = fxdax::extentd::fork(socket, src, dst)?;
                    println!("{branch:x}");
                    return Ok(());
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = socket;
                    bail!("the extent daemon runs on Linux only");
                }
            }
            let store = ExtentStore::open(src)?;
            if let Some(bucket) = bucket()? {
                let token = fxdax::remote::host_token()?;
                let options = fxdax::remote::Options { token, ..Default::default() };
                store.attach_remote(Arc::new(bucket), rand::random(), options)?;
            }
            let branch = rand::random::<u64>().max(1);
            store.fork(dst, branch)?;
            println!("{branch:x}");
            Ok(())
        }
        ["delete", dir] => {
            match std::env::var_os("FXDAX_EXTENTD").filter(|socket| !socket.is_empty()) {
                #[cfg(target_os = "linux")]
                Some(socket) => Ok(fxdax::extentd::delete(socket, dir)?),
                #[cfg(not(target_os = "linux"))]
                Some(_) => bail!("the extent daemon runs on Linux only"),
                None => Ok(ExtentStore::delete(dir, bucket()?.as_ref())?),
            }
        }
        ["resize", dir, capacity] => {
            let capacity = capacity.parse().context("CAPACITY must be a number")?;
            match std::env::var_os("FXDAX_EXTENTD").filter(|socket| !socket.is_empty()) {
                #[cfg(target_os = "linux")]
                Some(socket) => Ok(fxdax::extentd::resize(socket, dir, capacity)?),
                #[cfg(not(target_os = "linux"))]
                Some(_) => bail!("the extent daemon runs on Linux only"),
                None => {
                    let store = ExtentStore::open(dir)?;
                    if capacity < store.info()?.capacity {
                        bail!(
                            "shrinking a store needs its guest to agree: resize it through the \
                             extent daemon serving its VM (FXDAX_EXTENTD)"
                        );
                    }
                    Ok(store.set_capacity(capacity)?)
                }
            }
        }
        _ => bail!(usage()),
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
