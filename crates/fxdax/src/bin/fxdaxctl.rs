// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's host tool for VMs' disks, the extent stores extentd serves: makes them, of branches kept
//! in object storage (backlog doc-7), forks, deletes and resizes them:
//!
//! ```sh
//! fxdaxctl take DIR CAPACITY BRANCH
//! fxdaxctl fork SRC DST
//! fxdaxctl delete DIR
//! fxdaxctl resize DIR CAPACITY
//! ```
//!
//! `take` makes an empty store at DIR, of CAPACITY bytes, of BRANCH (hex): once extentd opens it,
//! with `FXDAX_EXTENTD_REMOTE` naming the branch's bucket, it takes what the branch holds there,
//! whose data it fetches as it is read. `fork` copies the store at SRC, which nothing may have open,
//! to a new store at DST, of a new branch, which it prints: with `FXDAX_EXTENTD_REMOTE`, the new
//! branch is a fork of SRC's there, made once SRC's commits are uploaded, holding its extents
//! without uploading them again. DST is a handle naming SRC's store, which holds the fork's data:
//! SRC's directory must not be removed while forks of it live. `delete` deletes a fork, whose data
//! goes once no other branch holds it, or a store, once no fork of it is left. `resize` sets the
//! store's capacity, the size of the disk its guest sees, to CAPACITY bytes: with `FXDAX_EXTENTD`
//! naming the socket of the extent daemon, through it, which grows a store its VM has open at once,
//! the guest seeing the new size while mounted, and shrinks it if the guest's filesystem agrees, no
//! lower than what it holds; without, it grows the store at DIR, which nothing may have open, and
//! does not shrink it.

use anyhow::{Context as _, bail};
use fxdax::ExtentStore;
use std::sync::Arc;

fn usage() -> anyhow::Error {
    anyhow::anyhow!(
        "usage: fxdaxctl take DIR CAPACITY BRANCH | fxdaxctl fork SRC DST | \
         fxdaxctl delete DIR | fxdaxctl resize DIR CAPACITY"
    )
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["take", dir, capacity, branch] => {
            let capacity = capacity.parse().context("CAPACITY must be a number")?;
            let branch = u64::from_str_radix(branch, 16).context("BRANCH must be hex")?;
            ExtentStore::create_for_branch(dir, capacity, branch)?;
            Ok(())
        }
        ["fork", src, dst] => {
            let store = ExtentStore::open(src)?;
            if let Ok(url) = std::env::var("FXDAX_EXTENTD_REMOTE") {
                let bucket =
                    extent_remote::store::Store::from_url(&url).map_err(anyhow::Error::msg)?;
                let options = fxdax::remote::Options::default();
                store.attach_remote(Arc::new(bucket), rand::random(), options)?;
            }
            let branch = rand::random::<u64>().max(1);
            store.fork(dst, branch)?;
            println!("{branch:x}");
            Ok(())
        }
        ["delete", dir] => Ok(extent_store::delete_host(dir.as_ref(), Default::default())?),
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
