// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What a filesystem's read or write of a page in memory costs ([`Node::read`] and
//! [`Node::write`], TASK-79.74): each call's overhead, the locks it takes and the file it holds
//! open under a guard on the volume's scope, beside its copy of one page through the file's
//! stream, which is too small to hide the overhead. Each thread reads, then writes, a file of its
//! own, a page at a time, round and round its pages, which are in memory and dirty, so that no
//! read or write waits for Fxfs's pager; on 1 thread, then on 8 at once, all on one volume, whose
//! locks they share.
//!
//! ```sh
//! cargo run --release -p fxdax --example filesystem-io-bench [OPS [ROUNDS]]
//! ```
//!
//! OPS is how many reads, and writes, each thread makes (default 200000), ROUNDS how many times
//! (default 5), each on a filesystem of its own. Prints the time a read and a write take on each
//! thread: the median over the rounds.

use futures::executor::block_on;
use fxdax::{Filesystem, Node, Owner, Storage};
use std::time::Instant;

/// The pages of each thread's file.
const PAGES: u64 = 64;
/// The threads the second run reads and writes on.
const THREADS: usize = 8;

/// Reads or writes `count` pages of `file` on the calling thread, round and round its pages.
fn ops(file: &Node, count: u64, write: bool) {
    let page = fxdax::pages::page_size();
    let mut buf = vec![0x5a; page as usize];
    block_on(async {
        for op in 0..count {
            let offset = op % PAGES * page;
            let done = if write {
                file.write(offset, &buf).await
            } else {
                file.read(offset, &mut buf).await
            };
            assert_eq!(done, Ok(page as usize));
        }
    });
}

/// The time, in nanoseconds, a read and a write take on each of `files.len()` threads at once,
/// each making `count` of each on a file of its own.
fn run(files: &[Node], count: u64) -> (f64, f64) {
    let time = |write: bool| {
        let start = Instant::now();
        std::thread::scope(|scope| {
            for file in files {
                scope.spawn(move || ops(file, count, write));
            }
        });
        start.elapsed().as_secs_f64() * 1e9 / count as f64
    };
    (time(false), time(true))
}

/// The times of a round's runs, on 1 thread, then on [`THREADS`]: read and write each.
fn round(count: u64) -> [(f64, f64); 2] {
    block_on(async {
        let fs = Filesystem::format(Storage::memory(64 << 20)).await.unwrap();
        let owner = Owner { uid: 0, gid: 0 };
        let page = fxdax::pages::page_size();
        let mut files = Vec::new();
        for thread in 0..THREADS {
            let name = format!("file-{thread}");
            let (file, _) = fs.root().create(name.as_bytes(), 0o644, owner).await.unwrap();
            let len = (PAGES * page) as usize;
            assert_eq!(file.write(0, &vec![1; len]).await, Ok(len));
            files.push(file);
        }
        let times = [run(&files[..1], count), run(&files, count)];
        drop(files);
        fs.close().await.unwrap();
        times
    })
}

/// The median of `times`, and all of them, in order.
fn median(mut times: Vec<f64>) -> (f64, String) {
    times.sort_by(f64::total_cmp);
    let all: Vec<String> = times.iter().map(|time| format!("{time:.0}")).collect();
    (times[times.len() / 2], all.join(", "))
}

fn main() {
    assert!(!cfg!(debug_assertions), "measure an optimized build: run with --release");
    let mut args = std::env::args().skip(1);
    let count: u64 = args.next().map_or(200_000, |n| n.parse().unwrap());
    let rounds: usize = args.next().map_or(5, |n| n.parse().unwrap());
    let rounds: Vec<[(f64, f64); 2]> = (0..rounds).map(|_| round(count)).collect();
    for (run, threads) in [1, THREADS].into_iter().enumerate() {
        let (reads, writes): (Vec<f64>, Vec<f64>) = rounds.iter().map(|times| times[run]).unzip();
        let ((read, reads), (write, writes)) = (median(reads), median(writes));
        println!(
            "{threads} thread{}: read {read:>6.0} ns ({reads}), write {write:>6.0} ns ({writes})",
            if threads == 1 { " " } else { "s" },
        );
    }
}
