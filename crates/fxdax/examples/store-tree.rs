// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Lists the filesystem in a store, as a guest's Fxfs left it: each node's path, type,
//! permissions, owner, size and link count, a line each, sorted, for comparing stores or finding
//! what a guest's crash kept:
//!
//! ```sh
//! cargo run -p fxdax --example store-tree STORE
//! ```
//!
//! The store is opened as of its last durable commit, and closed unchanged but for what opening it
//! recovers.

use futures::executor::block_on;
use fxdax::{FileType, Filesystem, Storage};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: store-tree STORE"))?;
    block_on(async {
        let fs = Filesystem::mount(Storage::open(&path)?).await?;
        let mut lines = Vec::new();
        let mut dirs = vec![(String::new(), fs.root().clone())];
        while let Some((dir_path, dir)) = dirs.pop() {
            for entry in dir.entries().await? {
                let name = String::from_utf8_lossy(&entry.name).into_owned();
                let path = format!("{dir_path}/{name}");
                let (node, attr) = dir.lookup(&entry.name).await?;
                lines.push(format!(
                    "{path} {:?} {:o} {}:{} size={} nlink={}",
                    attr.file_type, attr.perm, attr.uid, attr.gid, attr.size, attr.nlink
                ));
                if attr.file_type == FileType::Directory {
                    dirs.push((path, node));
                }
            }
        }
        lines.sort();
        for line in lines {
            println!("{line}");
        }
        fs.close().await?;
        anyhow::Ok(())
    })
}
