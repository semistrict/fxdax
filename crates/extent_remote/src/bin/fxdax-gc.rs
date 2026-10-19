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
//! URL names the bucket (`s3://bucket/prefix`, `gs://bucket/prefix`, `az://container/prefix`),
//! reached with credentials from the environment's `AWS_*`, `GOOGLE_*` or `AZURE_*` variables. It
//! collects once, or every SECONDS. A
//! compacted object goes, and moves no state names, a grace period after (60 s by default); objects
//! whose live extents take less than FRACTION of them (0.5) are compacted.

use extent_remote::collector::{Options, collect};
use extent_remote::store::{Store, probe};
use std::time::Duration;

fn usage() -> String {
    "usage: fxdax-gc URL [--every SECONDS] [--grace SECONDS] [--compact-below FRACTION]".into()
}

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Args {
    url: String,
    /// How long between passes, if it collects more than once.
    every: Option<Duration>,
    options: Options,
}

/// The command line `args`, after the program's name.
fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
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
    Ok(Args { url, every, options: Options { compact_below, grace: (grace * 1000.0) as u64 } })
}

fn main() -> Result<(), String> {
    let Args { url, every, options } = parse(std::env::args().skip(1))?;
    let bucket = Store::from_url(&url)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("a runtime: {err}"))?;
    runtime.block_on(async {
        // Each pass's holder differs from every other's.
        let holder = rand::random::<u64>;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    /// It collects once, with a grace period of a minute, compacting objects less than half live,
    /// unless its flags say otherwise; a grace period in seconds is kept in milliseconds.
    #[test]
    fn flags_override_the_defaults() {
        let url = "s3://bucket/prefix".to_owned();
        let defaults = Options { compact_below: 0.5, grace: 60_000 };
        let once = Args { url: url.clone(), every: None, options: defaults };
        assert_eq!(parsed(&["s3://bucket/prefix"]), Ok(once));
        let args = [&url, "--every", "30", "--grace", "2.5", "--compact-below", "0.25"];
        let options = Options { compact_below: 0.25, grace: 2500 };
        let every = Some(Duration::from_secs(30));
        assert_eq!(parsed(&args), Ok(Args { url, every, options }));
    }

    #[test]
    fn other_command_lines_get_the_usage() {
        for args in [&[][..], &["u", "--every"], &["u", "--every", "soon"], &["u", "--fast", "1"]] {
            assert_eq!(parsed(args), Err(usage()), "{args:?}");
        }
    }
}
