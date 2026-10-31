// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's collector (backlog doc-7, Garbage collection): reclaims what no live branch in a bucket
//! can reach, and compacts what is mostly freed. Only it deletes; it coordinates with branches'
//! writers, and with other collectors, through the bucket alone.
//!
//! ```sh
//! fxdax-gc URL [--every SECONDS] [--grace SECONDS] [--compact-below FRACTION]
//! fxdax-gc URL --branches [--held FILE]
//! fxdax-gc URL --delete-branch ID
//! ```
//!
//! URL names the bucket (`s3://bucket/prefix`, `gs://bucket/prefix`, `az://container/prefix`),
//! reached with credentials from the environment's `AWS_*`, `GOOGLE_*` or `AZURE_*` variables. It
//! collects once, or every SECONDS. A
//! compacted object goes, and moves no state names, a grace period after (60 s by default); objects
//! whose live extents take less than FRACTION of them (0.5) are compacted.
//!
//! The collector keeps what every live branch inherited, whether or not a host holds the branch,
//! which only hosts know. `--branches` lists the live branches, one a line, a fork's after the
//! branch and the segment it forked at, all in hexadecimal, but those FILE names, as
//! `fxdaxctl stat` calls them, separated by white space: with FILE naming every branch every host
//! holds, those left are no host's, or newer than FILE. `--delete-branch` deletes one, whichever
//! host took it last, which stops writing if there is one, and the next pass frees what only it
//! held (TASK-79.85).

use extent_remote::branch;
use extent_remote::collector::{Collector, Options};
use extent_remote::layout::Record;
use extent_remote::store::{Store, probe};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

fn usage() -> String {
    "usage: fxdax-gc URL [--every SECONDS] [--grace SECONDS] [--compact-below FRACTION] | \
     fxdax-gc URL --branches [--held FILE] | fxdax-gc URL --delete-branch ID"
        .into()
}

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Args {
    url: String,
    command: Command,
}

/// What it does to the bucket.
#[derive(Debug, PartialEq)]
enum Command {
    /// Collects, every so long if more than once.
    Collect { every: Option<Duration>, options: Options },
    /// Lists the live branches, but those a file names.
    Branches { held: Option<PathBuf> },
    /// Deletes a branch.
    DeleteBranch(u64),
}

/// The command line `args`, after the program's name.
fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let url = args.next().ok_or_else(usage)?;
    let args: Vec<String> = args.collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let command = match args.as_slice() {
        ["--branches"] => Command::Branches { held: None },
        ["--branches", "--held", file] => Command::Branches { held: Some(file.into()) },
        ["--delete-branch", branch] => {
            Command::DeleteBranch(u64::from_str_radix(branch, 16).map_err(|_| usage())?)
        }
        flags => parse_collect(flags)?,
    };
    Ok(Args { url, command })
}

/// A pass's `flags`.
fn parse_collect(flags: &[&str]) -> Result<Command, String> {
    let (mut every, mut grace, mut compact_below) = (None, 60.0, 0.5);
    for pair in flags.chunks(2) {
        let [flag, value] = pair else { return Err(usage()) };
        let value: f64 = value.parse().map_err(|_| usage())?;
        match *flag {
            "--every" => every = Some(Duration::from_secs_f64(value)),
            "--grace" => grace = value,
            "--compact-below" => compact_below = value,
            _ => return Err(usage()),
        }
    }
    let options = Options { compact_below, grace: (grace * 1000.0) as u64 };
    Ok(Command::Collect { every, options })
}

/// The branches `text` names, in hexadecimal, separated by white space.
fn held_branches(text: &str) -> Result<BTreeSet<u64>, String> {
    let branch = |word| u64::from_str_radix(word, 16).map_err(|_| format!("no branch: {word}"));
    text.split_whitespace().map(branch).collect()
}

/// What `--branches` prints of `live`, but those `held`.
fn listed(live: &BTreeMap<u64, Record>, held: &BTreeSet<u64>) -> String {
    let listed = live.iter().filter(|(branch, _)| !held.contains(branch));
    listed
        .map(|(branch, record)| match record.origin {
            Some(origin) => format!("{branch:x} {:x} {:x}\n", origin.parent, origin.at),
            None => format!("{branch:x}\n"),
        })
        .collect()
}

/// Collects `bucket`, once, or every `every`.
async fn collect(bucket: &Store, every: Option<Duration>, options: Options) -> Result<(), String> {
    // Each pass's holder differs from every other's.
    let holder = rand::random::<u64>;
    // What it read of branches' records it reads again only once they change.
    let mut collector = Collector::new(options);
    loop {
        match collector.collect(bucket, holder()).await {
            Ok(report) => {
                eprintln!("fxdax-gc: deleted {}, compacted {}", report.deleted, report.compacted)
            }
            // Nothing it could not read is deleted: the next pass tries again.
            Err(err) => eprintln!("fxdax-gc: collecting: {err}"),
        }
        let Some(every) = every else { return Ok(()) };
        tokio::time::sleep(every).await;
    }
}

fn main() -> Result<(), String> {
    let Args { url, command } = parse(std::env::args().skip(1))?;
    let bucket = Store::from_url(&url)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("a runtime: {err}"))?;
    runtime.block_on(async {
        probe(&bucket, rand::random()).await?;
        match command {
            Command::Collect { every, options } => collect(&bucket, every, options).await,
            Command::Branches { held } => {
                let held = match held {
                    Some(file) => std::fs::read_to_string(&file)
                        .map_err(|err| format!("{}: {err}", file.display()))?,
                    None => String::new(),
                };
                let held = held_branches(&held)?;
                let live = branch::live(&bucket).await.map_err(|err| err.to_string())?;
                print!("{}", listed(&live, &held));
                Ok(())
            }
            // Whichever placement took it last: u64::MAX is no lower than any's token.
            Command::DeleteBranch(id) => branch::delete(&bucket, id, rand::random(), u64::MAX)
                .await
                .map_err(|err| err.to_string()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use extent_remote::layout::Origin;

    fn parsed(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    /// It collects once, with a grace period of a minute, compacting objects less than half live,
    /// unless its flags say otherwise; a grace period in seconds is kept in milliseconds.
    #[test]
    fn flags_override_the_defaults() {
        let url = "s3://bucket/prefix".to_owned();
        let defaults = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect { every: None, options: defaults };
        assert_eq!(parsed(&["s3://bucket/prefix"]), Ok(Args { url: url.clone(), command }));
        let args = [&url, "--every", "30", "--grace", "2.5", "--compact-below", "0.25"];
        let options = Options { compact_below: 0.25, grace: 2500 };
        let every = Some(Duration::from_secs(30));
        let command = Command::Collect { every, options };
        assert_eq!(parsed(&args), Ok(Args { url, command }));
    }

    /// TASK-79.85: an operator lists what no host holds, and deletes a branch, by IDs as
    /// `fxdaxctl stat` says them.
    #[test]
    fn branches_are_listed_and_deleted_by_their_hexadecimal_ids() {
        let url = "u".to_owned();
        let command = Command::Branches { held: None };
        assert_eq!(parsed(&["u", "--branches"]), Ok(Args { url: url.clone(), command }));
        let command = Command::Branches { held: Some("hosts".into()) };
        let args = parsed(&["u", "--branches", "--held", "hosts"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let args = parsed(&["u", "--delete-branch", "ff"]);
        assert_eq!(args, Ok(Args { url, command: Command::DeleteBranch(0xff) }));
    }

    /// TASK-79.85: a list names the live branches but those held, a fork's with where it forked.
    #[test]
    fn lists_leave_out_what_is_held() {
        let base = Record { origin: None, forked: false, deleted: false };
        let origin = Some(Origin { parent: 0xa, at: 0x10 });
        let fork = Record { origin, forked: true, deleted: false };
        let live = BTreeMap::from([(0xa, base), (0xb, fork), (0xc, fork)]);
        let held = held_branches("c\n").unwrap();
        assert_eq!(listed(&live, &held), "a\nb a 10\n");
        assert_eq!(held_branches("c x"), Err("no branch: x".to_owned()));
    }

    #[test]
    fn other_command_lines_get_the_usage() {
        let lines: [&[&str]; 6] = [
            &[],
            &["u", "--every"],
            &["u", "--every", "soon"],
            &["u", "--fast", "1"],
            &["u", "--delete-branch", "branch"],
            &["u", "--branches", "--held"],
        ];
        for args in lines {
            assert_eq!(parsed(args), Err(usage()), "{args:?}");
        }
    }
}
