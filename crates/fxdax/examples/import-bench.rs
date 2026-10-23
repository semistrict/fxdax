// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What importing an image costs the importer, without a store on disk: layers made in memory
//! (`fxdax::import::testing`), applied in turn to a filesystem in memory, each timed. They are
//! the layers that cost building a base the most:
//!
//! - one large file, all data, which the importer reads from the layer and writes to the file a
//!   chunk at a time (TASK-92.36);
//! - many small files, in directories of a few each, as a distribution's are (TASK-92.36);
//! - a layer that replaces each of them, as a `chown -R` or an `apt-get upgrade` layer does, and
//!   takes each directory's new attributes, and one that whites each file out (TASK-92.37).
//!
//! ```sh
//! cargo run --release -p fxdax --example import-bench [FILES [LARGE_MIB [KERNEL_MIB [THREADS]]]]
//! ```
//!
//! FILES is how many small files there are (default 20000), LARGE_MIB the large file's size
//! (default 256), KERNEL_MIB the kernel's memory (default `kernel::Config::default`'s 1 GiB,
//! which `fxdaxctl import` runs with), its pressure levels scaled to it: a large file larger than
//! it is written back and evicted as it is written (TASK-79.70); and THREADS the threads Fxfs runs
//! on (`Options::executor_threads`, default 1, as `fxdaxctl import`'s). On one, its compactions run
//! between the import's requests: the allocator's, of its whole tree, cost more for each file the
//! more files there are, and so does each small file (TASK-92.44). Prints each layer's rate, of
//! bytes for the large file's and of entries for the others', the time setting the directories'
//! times takes, and what compacting Fxfs's trees cost meanwhile: the allocator's, and the object
//! stores' (TASK-92.46).

use futures::executor::block_on;
use fxdax::import::Importer;
use fxdax::import::testing::{Entry, Kind, layer, whiteout};
use fxdax::{Options, Storage, TreeCompactions};
use std::num::NonZeroU8;
use std::time::{Duration, Instant};

/// How many small files a directory holds.
const FILES_PER_DIR: usize = 16;
/// A small file's size.
const SMALL: usize = 1 << 10;

fn rate(name: &str, count: usize, unit: &str, elapsed: Duration) {
    println!(
        "{name:<22} {:>10.0} {unit}/s {:>8.2} µs/{unit} {:>8.3} s",
        count as f64 / elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1e6 / count as f64,
        elapsed.as_secs_f64(),
    );
}

/// Prints what compacting `name`'s trees cost.
fn compacted(name: &str, compactions: &TreeCompactions) {
    println!(
        "{name:<22} {:>10} compactions {:>10.1} MB {:>8.3} s",
        compactions.count,
        compactions.bytes as f64 / 1e6,
        compactions.time.as_secs_f64(),
    );
}

/// Applies `layer` with `importer`, and prints its rate, of `count` `unit`s.
async fn apply(importer: &mut Importer, name: &str, layer: &[u8], count: usize, unit: &str) {
    let start = Instant::now();
    importer.apply(layer).await.unwrap();
    rate(name, count, unit, start.elapsed());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let files: usize = args.next().map_or(20_000, |n| n.parse().unwrap());
    let large_mib: usize = args.next().map_or(256, |n| n.parse().unwrap());
    if let Some(kernel_mib) = args.next().map(|n| n.parse::<u64>().unwrap()) {
        let config = fxdax::kernel::Config::with_memory(kernel_mib << 20);
        fxdax::kernel::configure(config).unwrap();
    }
    let mut options = Options::new();
    if let Some(threads) = args.next().map(|n| n.parse::<NonZeroU8>().unwrap()) {
        options.executor_threads(threads);
    }

    let contents = vec![0x5a; large_mib << 20];
    let large = layer(&[Entry::new("large", Kind::File(&contents))]).unwrap();
    drop(contents);
    let dirs: Vec<String> = (0..files.div_ceil(FILES_PER_DIR)).map(|d| format!("d{d}")).collect();
    let paths: Vec<String> =
        (0..files).map(|i| format!("{}/file-{i}", dirs[i / FILES_PER_DIR])).collect();
    let small = [b'x'; SMALL];
    // The directories, each before its files, as a layer lists them.
    let tree = || {
        let mut entries = Vec::with_capacity(dirs.len() + files);
        for (d, dir) in dirs.iter().enumerate() {
            entries.push(Entry::new(dir, Kind::Dir));
            let end = paths.len().min((d + 1) * FILES_PER_DIR);
            for path in &paths[d * FILES_PER_DIR..end] {
                entries.push(Entry::new(path, Kind::File(&small)));
            }
        }
        entries
    };
    let small_files = layer(&tree()).unwrap();
    let replaced =
        layer(&tree().into_iter().map(|entry| entry.owner(1000, 1000)).collect::<Vec<_>>())
            .unwrap();
    let whiteouts: Vec<String> = paths.iter().map(|path| whiteout(path)).collect();
    let whited_out =
        layer(&whiteouts.iter().map(|path| Entry::new(path, Kind::File(b""))).collect::<Vec<_>>())
            .unwrap();

    let entries = dirs.len() + files;
    // Room for the large file, for the journal, and for the small files a few times over, a block
    // of metadata each, as those replaced are freed only once the journal has moved on.
    let small_files_bytes = 4 * entries as u64 * (SMALL as u64 + Storage::BLOCK_SIZE as u64);
    let capacity = ((large_mib as u64 + 256) << 20) + small_files_bytes;
    let capacity = capacity.next_multiple_of(Storage::BLOCK_SIZE as u64);
    block_on(async {
        let fs = options.format(Storage::memory(capacity)).await.unwrap();
        let mut importer = Importer::new(&fs);
        apply(&mut importer, "large file", &large, large_mib, "MiB").await;
        apply(&mut importer, "small files", &small_files, entries, "entry").await;
        apply(&mut importer, "replacing them", &replaced, entries, "entry").await;
        apply(&mut importer, "whiting them out", &whited_out, files, "entry").await;
        let start = Instant::now();
        importer.finish().await.unwrap();
        rate("finishing", dirs.len(), "dir", start.elapsed());
        let compactions = fs.compactions().unwrap();
        compacted("allocator", &compactions.allocator);
        compacted("object stores", &compactions.stores);
        fs.close().await.unwrap();
    });
}
