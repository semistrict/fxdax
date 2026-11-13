// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: fills a bucket of files with cells of branches holding garbage, for measuring the
//! collector's sweeps over many cells (TASK-395.5; `scripts/gc-sweep.sh`).
//!
//! ```sh
//! fxdax-gc-fill DIR TENANTS CELLS SEGMENTS [LARGEST]
//! ```
//!
//! DIR, a directory, made if missing and empty, becomes a bucket of files (`file:///DIR/`) of
//! TENANTS tenants of CELLS cells each, each cell's object and one branch, its record, its lease,
//! SEGMENTS segments and a checkpoint covering them all: SEGMENTS + 5 objects a cell, but the first
//! cell of tenant 1, the largest, whose branch has LARGEST segments (SEGMENTS by default). Each
//! segment holds four extents of 1 KiB, and the next frees three of them, every other one all
//! four: so a pass deletes about half of each branch's segments, those freed whole, and compacts
//! the rest, a quarter live, once it compacts what is less than half live.
//!
//! Each cell is written by a writer of its own into memory, then copied into DIR, its files
//! written without `fsync` (the measurement's bucket, not data to keep), many cells at once, one
//! a thread. It prints `cells N objects N bytes N largest-objects N largest-bytes N`: what it
//! wrote, and the largest cell's share.

use extent_remote::batch::{Batch, Commit};
use extent_remote::bucket::Bucket as _;
use extent_remote::layout::{Branch, Cell};
use extent_remote::store::Store;
use extent_remote::{Writer, branch, cell};
use object_store::ObjectStore as _;
use object_store::local::LocalFileSystem;
use object_store::{PutOptions, PutPayload};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use storage_device::ExtentId;

/// The extents each segment holds, and their bytes.
const EXTENTS: u64 = 4;
const EXTENT_BYTES: usize = 1024;

fn usage() -> String {
    "usage: fxdax-gc-fill DIR TENANTS CELLS SEGMENTS [LARGEST]".into()
}

/// What a cell's objects take.
#[derive(Clone, Copy, Debug, Default)]
struct Sum {
    objects: u64,
    bytes: u64,
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let count = |at: usize| -> Result<u64, String> {
        let arg = args.get(at).ok_or_else(usage)?;
        arg.parse().ok().filter(|&count| count > 0).ok_or_else(usage)
    };
    if !(4..=5).contains(&args.len()) {
        return Err(usage());
    }
    let dir = std::path::PathBuf::from(&args[0]);
    let (tenants, cells, segments) = (count(1)?, count(2)?, count(3)?);
    let largest = if args.len() == 5 { count(4)? } else { segments };
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    if std::fs::read_dir(&dir).map_err(|err| err.to_string())?.next().is_some() {
        return Err(format!("{}: not empty", dir.display()));
    }
    let files = LocalFileSystem::new_with_prefix(&dir).map_err(|err| err.to_string())?;
    let shapes: Vec<(Cell, u64)> = (1..=tenants)
        .flat_map(|tenant| (1..=cells).map(move |id| Cell { tenant, id }))
        .map(|cell| (cell, if cell == (Cell { tenant: 1, id: 1 }) { largest } else { segments }))
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, |threads| threads.get());
    let next = AtomicUsize::new(0);
    let sums: Vec<Result<Vec<(Cell, Sum)>, String>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut sums = Vec::new();
                    while let Some(&(cell, segments)) =
                        shapes.get(next.fetch_add(1, Ordering::Relaxed))
                    {
                        let sum = futures::executor::block_on(fill(&files, cell, segments))
                            .map_err(|err| format!("cell {cell}: {err}"))?;
                        sums.push((cell, sum));
                    }
                    Ok(sums)
                })
            })
            .collect();
        workers.into_iter().map(|worker| worker.join().expect("a filler panicked")).collect()
    });
    let (mut total, mut biggest) = (Sum::default(), Sum::default());
    for (cell, sum) in sums.into_iter().collect::<Result<Vec<_>, _>>()?.into_iter().flatten() {
        total.objects += sum.objects;
        total.bytes += sum.bytes;
        if cell == (Cell { tenant: 1, id: 1 }) {
            biggest = sum;
        }
    }
    println!(
        "cells {} objects {} bytes {} largest-objects {} largest-bytes {}",
        shapes.len(),
        total.objects,
        total.bytes,
        biggest.objects,
        biggest.bytes
    );
    Ok(())
}

/// Writes `cell`, its branch of `segments` segments, into memory, then into `files`: what it took.
async fn fill(files: &LocalFileSystem, cell: Cell, segments: u64) -> Result<Sum, String> {
    let memory = Arc::new(object_store::memory::InMemory::new());
    let bucket = Arc::new(Store::new(memory, ""));
    let failed = |err: extent_remote::Error| err.to_string();
    cell::create(&*bucket, cell, 10, 1).await.map_err(failed)?;
    let id = Branch { cell, id: 1 };
    branch::create(&*bucket, id, 0).await.map_err(failed)?;
    let writer = Writer::open(bucket.clone(), id, 10, 1).await.map_err(failed)?;
    let mut held: Vec<ExtentId> = Vec::new();
    for n in 0..segments {
        let first = writer.chain().next_sequence();
        let extents: Vec<(ExtentId, Vec<u8>)> = (0..EXTENTS)
            .map(|i| {
                let bytes = (0..EXTENT_BYTES).map(|at| ((at as u64 * 7 + n + i) % 251) as u8);
                (ExtentId::new(1, first + i), bytes.collect())
            })
            .collect();
        // The segment before freed whole if it was an even one, else but for its first extent.
        let kept = usize::from(n % 2 == 0 && n > 0);
        let frees = held.split_off(kept.min(held.len()));
        held = extents.iter().map(|(extent, _)| *extent).collect();
        let commit = Commit { tag: n + 1, extents, frees, ..Commit::default() };
        let segment = Batch::from(commit).close(&mut writer.chain()).frame();
        let seq = writer.add_segment(segment).map_err(failed)?;
        writer.upload(seq).await.map_err(failed)?;
        writer.acknowledge().await.map_err(failed)?;
    }
    writer.checkpoint().await.map_err(failed)?;
    let mut sum = Sum::default();
    for meta in bucket.list("").await.map_err(|err| err.to_string())? {
        let object = bucket.get(&meta.key).await.map_err(|err| err.to_string())?;
        let path = object_store::path::Path::from(meta.key.as_str());
        let payload = PutPayload::from(object.bytes);
        files
            .put_opts(&path, payload, PutOptions::default())
            .await
            .map_err(|err| err.to_string())?;
        sum.objects += 1;
        sum.bytes += meta.size;
    }
    Ok(sum)
}
