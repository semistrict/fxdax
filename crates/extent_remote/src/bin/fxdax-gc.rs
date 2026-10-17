// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's collector (backlog doc-7, Garbage collection): reclaims what no live branch in a bucket
//! can reach, and compacts what is mostly freed. Only it deletes; it coordinates with branches'
//! writers, and with other collectors, through the bucket alone.
//!
//! ```sh
//! fxdax-gc URL [--every SECONDS] [--grace SECONDS] [--compact-below FRACTION]
//! ```
//!
//! URL names the bucket (`s3://bucket/prefix`, `gs://bucket/prefix`), reached with credentials
//! from the environment's `AWS_*` or `GOOGLE_*` variables. It collects once, or every SECONDS. A
//! compacted object goes, and moves no state names, a grace period after (60 s by default); objects
//! whose live extents take less than FRACTION of them (0.5) are compacted.

use extent_remote::collector::{Options, collect};
use extent_remote::store::{Store, probe};
use std::time::Duration;

fn usage() -> String {
    "usage: fxdax-gc URL [--every SECONDS] [--grace SECONDS] [--compact-below FRACTION]".into()
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let url = args.next().ok_or_else(usage)?;
    let (mut every, mut grace, mut compact_below) = (None, 60.0, 0.5);
    while let Some(flag) = args.next() {
        let value: f64 = args.next().ok_or_else(usage)?.parse().map_err(|_| usage())?;
        match flag.as_str() {
            "--every" => every = Some(Duration::from_secs_f64(value)),
            "--grace" => grace = value,
            "--compact-below" => compact_below = value,
            _ => return Err(usage()),
        }
    }
    let options = Options { compact_below, grace: (grace * 1000.0) as u64 };
    let bucket = Store::from_url(&url)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("a runtime: {err}"))?;
    runtime.block_on(async {
        // Each pass's holder differs from every other's.
        let holder = || {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
            now.map_or(0, |now| now.as_nanos() as u64) ^ u64::from(std::process::id()) << 48
        };
        probe(&bucket, holder()).await?;
        loop {
            match collect(&bucket, holder(), &options).await {
                Ok(report) => {
                    eprintln!(
                        "fxdax-gc: deleted {}, compacted {}",
                        report.deleted, report.compacted
                    )
                }
                // Nothing it could not read is deleted: the next pass tries again.
                Err(err) => eprintln!("fxdax-gc: collecting: {err}"),
            }
            let Some(every) = every else { return Ok(()) };
            tokio::time::sleep(every).await;
        }
    })
}
