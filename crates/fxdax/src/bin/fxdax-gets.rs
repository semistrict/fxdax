// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's measurement of ranged GETs from object storage (TASK-28.11; doc-7, decision 9): the
//! throughput a host gets reading parts of objects, by the parts' size and how many GETs it keeps
//! in flight, which choose the size of the parts stores frame their data in (`extent_frame::PART`)
//! and a host's budget of GETs in flight (`fxdax::remote`'s).
//!
//! ```sh
//! fxdax-gets URL [OBJECT_MIB] [SECS]
//! ```
//!
//! It puts an object of OBJECT_MIB MiB (default 512) of bytes that don't compress under URL (a
//! bucket and prefix, as `FXDAX_EXTENTD_REMOTE` names them: `gs://bucket/prefix`), then, for each
//! part size from 256 KiB to 4 MiB and each number of GETs in flight from 1 to 128, GETs parts of
//! it at random for SECS seconds (default 10), and prints a Markdown table of what each did: MiB/s,
//! GETs/s, and their latencies' median and 99th percentile. Then it deletes the object.

use extent_remote::bucket::{Bucket, ByteRange, PutMode};
use extent_remote::store::Store;
use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The parts' sizes measured.
const PARTS: [u64; 5] = [256 << 10, 512 << 10, 1 << 20, 2 << 20, 4 << 20];
/// The numbers of GETs in flight measured.
const IN_FLIGHT: [usize; 6] = [1, 4, 16, 32, 64, 128];
/// The object's key, under the URL's prefix.
const KEY: &str = "gets/object";

/// What GETs of one size, so many in flight, did.
struct Measured {
    bytes: u64,
    gets: u64,
    elapsed: Duration,
    /// Sorted.
    latencies: Vec<Duration>,
}

impl Measured {
    fn percentile(&self, percent: usize) -> Duration {
        let at = (self.latencies.len() * percent / 100).min(self.latencies.len() - 1);
        self.latencies[at]
    }
}

/// `len` bytes that don't compress: a xorshift's.
fn incompressible(len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut bytes = Vec::with_capacity(len + 8);
    while bytes.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

/// GETs parts of `part` bytes of the object of `len` bytes, at random, `in_flight` at once, for
/// `secs`.
async fn measure(
    store: &Arc<Store>,
    len: u64,
    part: u64,
    in_flight: usize,
    secs: Duration,
) -> Result<Measured, String> {
    let parts = len / part;
    let started = Instant::now();
    let deadline = started + secs;
    let mut gets = FuturesUnordered::new();
    let mut measured =
        Measured { bytes: 0, gets: 0, elapsed: Duration::ZERO, latencies: Vec::new() };
    loop {
        while gets.len() < in_flight && Instant::now() < deadline {
            let at = rand::random_range(0..parts) * part;
            let store = store.clone();
            gets.push(async move {
                let start = Instant::now();
                let got = store.get_range(KEY, ByteRange::Bounded(at..at + part)).await;
                (start.elapsed(), got)
            });
        }
        let Some((took, got)) = gets.next().await else { break };
        let got = got.map_err(|err| format!("a GET: {err:?}"))?;
        measured.bytes += got.len() as u64;
        measured.gets += 1;
        measured.latencies.push(took);
    }
    measured.elapsed = started.elapsed();
    measured.latencies.sort();
    Ok(measured)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: fxdax-gets URL [OBJECT_MIB] [SECS]";
    let url = args.first().ok_or(usage)?;
    let number = |at: usize, default: u64| -> Result<u64, String> {
        args.get(at).map_or(Ok(default), |arg| arg.parse().map_err(|_| usage.to_owned()))
    };
    let len = number(1, 512)? << 20;
    let secs = Duration::from_secs(number(2, 10)?);
    let store = Arc::new(Store::from_url(url)?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("a runtime: {err}"))?;
    runtime.block_on(async {
        let bytes = incompressible(len as usize);
        let put = store.put(KEY, bytes, PutMode::Overwrite).await;
        put.map_err(|err| format!("putting the object: {err:?}"))?;
        // Connections made, and the object where it is read from, before what is measured.
        measure(&store, len, 1 << 20, 16, Duration::from_secs(2)).await?;
        println!(
            "Ranged GETs of an object of {} MiB at {url}, {} s each.\n",
            len >> 20,
            secs.as_secs()
        );
        println!("| part | in flight | MiB/s | GETs/s | p50 ms | p99 ms |");
        println!("|---:|---:|---:|---:|---:|---:|");
        for part in PARTS {
            for in_flight in IN_FLIGHT {
                let measured = measure(&store, len, part, in_flight, secs).await?;
                let elapsed = measured.elapsed.as_secs_f64();
                println!(
                    "| {} KiB | {in_flight} | {:.1} | {:.1} | {:.1} | {:.1} |",
                    part >> 10,
                    measured.bytes as f64 / elapsed / (1 << 20) as f64,
                    measured.gets as f64 / elapsed,
                    measured.percentile(50).as_secs_f64() * 1e3,
                    measured.percentile(99).as_secs_f64() * 1e3,
                );
            }
        }
        store.delete(KEY).await.map_err(|err| format!("deleting the object: {err:?}"))
    })
}
