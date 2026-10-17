//! Makes extent stores of branches kept in object storage (backlog doc-7), for extentd to serve:
//!
//! ```sh
//! fxdax-branch take DIR CAPACITY BRANCH
//! fxdax-branch fork SRC DST
//! ```
//!
//! `take` makes an empty store at DIR, of CAPACITY bytes, of BRANCH (hex): once extentd opens it,
//! with `FXDAX_EXTENTD_REMOTE` naming the branch's bucket, it takes what the branch holds there,
//! whose data it fetches as it is read. `fork` copies the store at SRC, which nothing may have open,
//! to a new store at DST, of a new branch, which it prints: with `FXDAX_EXTENTD_REMOTE`, the new
//! branch is a fork of SRC's there, made once SRC's commits are uploaded, holding its extents
//! without uploading them again.

use anyhow::{Context as _, bail};
use fxdax::ExtentStore;
use std::sync::Arc;

fn usage() -> anyhow::Error {
    anyhow::anyhow!("usage: fxdax-branch take DIR CAPACITY BRANCH | fxdax-branch fork SRC DST")
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
        _ => bail!(usage()),
    }
}
