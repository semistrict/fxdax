// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Says what a stopped store's Fxfs holds at a file's offset, as the host reads it: for a guest
//! test's failure, whose store its results keep (TASK-133). Mounting replays its journal: give it
//! a copy.
//!
//! ```sh
//! cargo run -p fxdax --example store-read -- STORE NAME OFFSET LEN
//! ```

use anyhow::Context as _;
use futures::executor::block_on;
use fxdax::{Filesystem, Storage};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [store, name, offset, len] = &args[..] else {
        anyhow::bail!("usage: store-read STORE NAME OFFSET LEN");
    };
    let number = |text: &str| u64::from_str_radix(text.trim_start_matches("0x"), 16);
    let (offset, len) = (number(offset).context("OFFSET")?, number(len).context("LEN")?);
    block_on(async {
        let fs = Filesystem::mount(Storage::open(store)?.check().await?).await?;
        let (mut file, mut attr) = (None, None);
        for part in name.split('/') {
            let dir = file.as_ref().unwrap_or(fs.root());
            let (node, found) = dir.lookup(part.as_bytes()).await?;
            (file, attr) = (Some(node), Some(found));
        }
        let (file, attr) = (file.context("a name")?, attr.context("a name")?);
        println!("size {:#x}", attr.size);
        let mut read = vec![0; len as usize];
        let got = file.read(offset, &mut read).await?;
        for (at, row) in read[..got].chunks(32).enumerate() {
            let bytes: Vec<String> = row.iter().map(|byte| format!("{byte:02x}")).collect();
            println!("{:#08x} {}", offset + (at as u64) * 32, bytes.join(" "));
        }
        fs.close().await?;
        anyhow::Ok(())
    })
}
