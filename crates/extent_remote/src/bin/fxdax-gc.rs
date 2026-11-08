// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's collector (backlog doc-7, Garbage collection): reclaims what no live branch in a bucket
//! can reach, and compacts what is mostly freed. Only it deletes; it coordinates with branches'
//! writers, and with other collectors, through the bucket alone.
//!
//! ```sh
//! fxdax-gc URL [--mark-only] [--every SECONDS] [--grace SECONDS] [--compact-below FRACTION]
//! fxdax-gc URL --dry-run [--grace SECONDS] [--compact-below FRACTION]
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
//! `--mark-only` collects deleting nothing (TASK-264): each pass compacts as one that deletes does,
//! and writes what it would delete to `gc/marks/<epoch>` instead, which every reader of the bucket
//! (extentd, fxdaxctl, writers recovering, this collector) warns of reaching, a bug in the
//! collector; and warns, loudly, of what the pass before marked that it finds kept, the same
//! objects still, another. Nothing marked goes: the bucket grows by what passes would have
//! reclaimed, the objects compacted and their moves both, until an operator collects it for real or
//! removes the marks. `--dry-run` writes nothing at all, not even the collector's lease, which
//! would fence a pass running meanwhile out of its deletes, and compacts nothing: it prints each
//! object a pass would delete, `KEY KIND SIZE delete` a line, and each it would compact, by its
//! listed size, `KEY KIND SIZE compact`, then their counts and bytes. It finds fewer than a pass
//! deletes: what a pass deletes only once it compacted it, a grace period after, it can't find
//! doomed, those it would compact among them. A pass of either says what it did on standard
//! error, and logs there what warns.
//!
//! The collector keeps what every live branch inherited, whether or not a host holds the branch,
//! which only hosts know. `--branches` lists the live branches, one a line, a fork's after the
//! branch and the segment it forked at, all in hexadecimal, but those FILE names, as
//! `fxdaxctl stat` calls them, separated by white space: with FILE naming every branch every host
//! holds, those left are no host's, or newer than FILE. `--delete-branch` deletes one, whichever
//! host took it last, which stops writing if there is one, and the next pass frees what only it
//! held (TASK-79.85).

use extent_remote::branch;
use extent_remote::bucket::Meta;
use extent_remote::collector::{Collector, DryRun, Marking, Options};
use extent_remote::layout;
use extent_remote::record::Record;
use extent_remote::store::{Store, probe};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

fn usage() -> String {
    "usage: fxdax-gc URL [--mark-only] [--every SECONDS] [--grace SECONDS] \
     [--compact-below FRACTION] | fxdax-gc URL --dry-run [--grace SECONDS] \
     [--compact-below FRACTION] | \
     fxdax-gc URL --branches [--held FILE] | fxdax-gc URL --delete-branch ID \
     (--mark-only deletes nothing, writing gc/marks/EPOCH: the bucket grows; \
     --dry-run writes nothing, and finds less than a pass deletes)"
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
    Collect { every: Option<Duration>, options: Options, mode: Mode },
    /// Lists the live branches, but those a file names.
    Branches { held: Option<PathBuf> },
    /// Deletes a branch.
    DeleteBranch(u64),
}

/// What a pass does with what no live branch needs (TASK-264).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Delete,
    /// Marks it, compacting as a pass that deletes does.
    MarkOnly,
    /// Prints it, writing nothing, once.
    DryRun,
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

/// A pass's `flags`: a dry run's run once.
fn parse_collect(flags: &[&str]) -> Result<Command, String> {
    let (mut every, mut grace, mut compact_below) = (None, 60.0, 0.5);
    let mut mode = Mode::Delete;
    let mut flags = flags.iter();
    while let Some(&flag) = flags.next() {
        let mut value = || -> Result<f64, String> {
            flags.next().ok_or_else(usage)?.parse().map_err(|_| usage())
        };
        match flag {
            "--mark-only" | "--dry-run" if mode != Mode::Delete => return Err(usage()),
            "--mark-only" => mode = Mode::MarkOnly,
            "--dry-run" => mode = Mode::DryRun,
            "--every" => every = Some(Duration::from_secs_f64(value()?)),
            "--grace" => grace = value()?,
            "--compact-below" => compact_below = value()?,
            _ => return Err(usage()),
        }
    }
    if mode == Mode::DryRun && every.is_some() {
        return Err(usage());
    }
    let options = Options { compact_below, grace: (grace * 1000.0) as u64 };
    Ok(Command::Collect { every, options, mode })
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
        .map(|(branch, record)| match record.origin() {
            Some(origin) => format!("{branch:x} {:x} {:x}\n", origin.parent, origin.at),
            None => format!("{branch:x}\n"),
        })
        .collect()
}

/// Collects `bucket`, once, or every `every`, deleting or marking as `mode` says; or prints what a
/// pass would delete, once.
async fn collect(
    bucket: &Store,
    every: Option<Duration>,
    options: Options,
    mode: Mode,
) -> Result<(), String> {
    // Each pass's holder differs from every other's.
    let holder = rand::random::<u64>;
    // What it read of branches' records it reads again only once they change.
    let mut collector = Collector::new(options);
    loop {
        match mode {
            Mode::Delete => match collector.collect(bucket, holder()).await {
                Ok(report) => {
                    eprintln!(
                        "fxdax-gc: deleted {}, compacted {}",
                        report.deleted, report.compacted
                    )
                }
                // Nothing it could not read is deleted: the next pass tries again.
                Err(err) => eprintln!("fxdax-gc: collecting: {err}"),
            },
            Mode::MarkOnly => match collector.mark(bucket, holder()).await {
                Ok(marking) => eprintln!("{}", marked_line(&marking)),
                Err(err) => eprintln!("fxdax-gc: marking: {err}"),
            },
            Mode::DryRun => {
                let dry = collector.dry_run(bucket).await.map_err(|err| err.to_string())?;
                let (doomed, summary) = dry_run(&dry);
                print!("{doomed}");
                eprintln!("{summary}");
                return Ok(());
            }
        }
        let Some(every) = every else { return Ok(()) };
        tokio::time::sleep(every).await;
    }
}

/// What a mark-only pass says of `marking`.
fn marked_line(marking: &Marking) -> String {
    let Some(marks) = &marking.marks else {
        let compacted = marking.report.compacted;
        return format!(
            "fxdax-gc: marked nothing, a later pass having taken the lease, compacted {compacted}"
        );
    };
    let bytes: u64 = marks.marked.iter().map(|marked| marked.size).sum();
    format!(
        "fxdax-gc: marked {} ({bytes} bytes) in {}, deleted 0, compacted {}, resurrected {}",
        marks.marked.len(),
        layout::marks(marks.epoch),
        marking.report.compacted,
        marking.resurrected.len()
    )
}

/// What a dry run prints of `dry`: each object a pass would delete, then each it would compact, a
/// line of its key, kind, size and which, for standard output; and the counts and bytes of them,
/// with what they leave out, for standard error.
fn dry_run(dry: &DryRun) -> (String, String) {
    let line = |meta: &Meta, which| {
        format!("{} {} {} {which}\n", meta.key, layout::kind(&meta.key).name(), meta.size)
    };
    let listed = (dry.doomed.iter().map(|meta| line(meta, "delete")))
        .chain(dry.compactable.iter().map(|meta| line(meta, "compact")))
        .collect();
    let bytes = |metas: &[Meta]| metas.iter().map(|meta| meta.size).sum::<u64>();
    let mut summary = format!(
        "fxdax-gc: dry run: a pass would delete {} objects ({} bytes), and more: it would compact \
         {} ({} bytes), and delete what it compacts a grace period after, which a dry run, \
         compacting nothing, doesn't count",
        dry.doomed.len(),
        bytes(&dry.doomed),
        dry.compactable.len(),
        bytes(&dry.compactable),
    );
    if dry.fenced {
        summary += "; a pass took the collector's lease meanwhile, deleting what this read may \
                    name: run it again";
    }
    (listed, summary)
}

/// fxdax: logs what is as grave as a warning on standard error, a line each, its key-values after
/// its message: what readers of the bucket and the collector warn of (TASK-264).
struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        struct Pairs(String);
        impl<'kvs> log::kv::VisitSource<'kvs> for Pairs {
            fn visit_pair(
                &mut self,
                key: log::kv::Key<'kvs>,
                value: log::kv::Value<'kvs>,
            ) -> Result<(), log::kv::Error> {
                self.0 += &format!(" {key}={value}");
                Ok(())
            }
        }
        if self.enabled(record.metadata()) {
            let mut pairs = Pairs(String::new());
            let _ = record.key_values().visit(&mut pairs);
            eprintln!("{} {}{}", record.level(), record.args(), pairs.0);
        }
    }

    fn flush(&self) {}
}

fn main() -> Result<(), String> {
    let Args { url, command } = parse(std::env::args().skip(1))?;
    log::set_logger(&Stderr).map_err(|err| format!("a log: {err}"))?;
    log::set_max_level(log::LevelFilter::Warn);
    let bucket = Store::from_url(&url)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("a runtime: {err}"))?;
    runtime.block_on(async {
        // A dry run writes nothing, and relies on no condition of a write's.
        if !matches!(command, Command::Collect { mode: Mode::DryRun, .. }) {
            probe(&bucket, rand::random()).await?;
        }
        match command {
            Command::Collect { every, options, mode } => {
                collect(&bucket, every, options, mode).await
            }
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
    use extent_remote::bucket::Version;
    use extent_remote::layout::Origin;
    use extent_remote::record::Kind;

    fn parsed(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    /// It collects once, with a grace period of a minute, compacting objects less than half live,
    /// unless its flags say otherwise; a grace period in seconds is kept in milliseconds.
    #[test]
    fn flags_override_the_defaults() {
        let url = "s3://bucket/prefix".to_owned();
        let defaults = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect { every: None, options: defaults, mode: Mode::Delete };
        assert_eq!(parsed(&["s3://bucket/prefix"]), Ok(Args { url: url.clone(), command }));
        let args = [&url, "--every", "30", "--grace", "2.5", "--compact-below", "0.25"];
        let options = Options { compact_below: 0.25, grace: 2500 };
        let every = Some(Duration::from_secs(30));
        let command = Command::Collect { every, options, mode: Mode::Delete };
        assert_eq!(parsed(&args), Ok(Args { url, command }));
    }

    /// TASK-264: a mark-only pass takes every flag a pass does, anywhere among them; a dry run
    /// each but `--every`: it runs once.
    #[test]
    fn passes_mark_only_or_run_dry() {
        let url = "u".to_owned();
        let args = ["u", "--every", "30", "--mark-only", "--grace", "0", "--compact-below", "0.1"];
        let options = Options { compact_below: 0.1, grace: 0 };
        let every = Some(Duration::from_secs(30));
        let command = Command::Collect { every, options, mode: Mode::MarkOnly };
        assert_eq!(parsed(&args), Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect { every: None, options, mode: Mode::MarkOnly };
        assert_eq!(parsed(&["u", "--mark-only"]), Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.25, grace: 1500 };
        let command = Command::Collect { every: None, options, mode: Mode::DryRun };
        let args = parsed(&["u", "--grace", "1.5", "--dry-run", "--compact-below", "0.25"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect { every: None, options, mode: Mode::DryRun };
        assert_eq!(parsed(&["u", "--dry-run"]), Ok(Args { url, command }));
    }

    /// TASK-264: a dry run lists each object a pass would delete, and each it would compact, with
    /// its kind and size, and says their counts and bytes, and that a pass deletes more; and that it
    /// may be stale, if fenced.
    #[test]
    fn dry_runs_list_what_goes_and_say_it_is_less() {
        let meta = |key: String, size| Meta { key, version: Version::default(), size, modified: 0 };
        let doomed = vec![meta(layout::segment(1, 2), 100), meta(layout::lease(3), 20)];
        let compactable = vec![meta(layout::segment(1, 4), 7)];
        let (listed, summary) = dry_run(&DryRun { doomed, compactable, fenced: false });
        let expected = "segments/0000000000000001/0000000000000002 segment 100 delete\n\
                        leases/0000000000000003 lease 20 delete\n\
                        segments/0000000000000001/0000000000000004 segment 7 compact\n";
        assert_eq!(listed, expected);
        assert_eq!(
            summary,
            "fxdax-gc: dry run: a pass would delete 2 objects (120 bytes), and more: it would \
             compact 1 (7 bytes), and delete what it compacts a grace period after, which a dry \
             run, compacting nothing, doesn't count"
        );
        let (listed, summary) = dry_run(&DryRun { fenced: true, ..DryRun::default() });
        assert_eq!(listed, "");
        assert!(summary.ends_with("meanwhile, deleting what this read may name: run it again"));
    }

    /// TASK-264: a mark-only pass says what it marked, where, and what it found resurrected; or
    /// that it marked nothing, fenced.
    #[test]
    fn mark_only_passes_say_what_they_marked() {
        use extent_remote::collector::Report;
        use extent_remote::layout::{Marked, Marks};
        let marked = |key: &str, size| Marked {
            key: key.into(),
            size,
            e_tag: None,
            version: None,
            modified: 0,
        };
        let marks = Marks { epoch: 0x2a, grace: 0, marked: vec![marked("a", 5), marked("b", 6)] };
        let marking = Marking {
            report: Report { deleted: 0, compacted: 3 },
            marks: Some(marks),
            resurrected: vec!["c".into()],
        };
        assert_eq!(
            marked_line(&marking),
            "fxdax-gc: marked 2 (11 bytes) in gc/marks/000000000000002a, deleted 0, compacted 3, \
             resurrected 1"
        );
        let fenced = Marking { report: Report { deleted: 0, compacted: 1 }, ..Marking::default() };
        assert_eq!(
            marked_line(&fenced),
            "fxdax-gc: marked nothing, a later pass having taken the lease, compacted 1"
        );
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
        let base = Kind::Root.of(0);
        let fork = Kind::Fork(Origin { parent: 0xa, at: 0x10 }).of(0);
        let live = BTreeMap::from([(0xa, base), (0xb, fork), (0xc, fork)]);
        let held = held_branches("c\n").unwrap();
        assert_eq!(listed(&live, &held), "a\nb a 10\n");
        assert_eq!(held_branches("c x"), Err("no branch: x".to_owned()));
    }

    #[test]
    fn other_command_lines_get_the_usage() {
        let lines: [&[&str]; 11] = [
            &[],
            &["u", "--every"],
            &["u", "--every", "soon"],
            &["u", "--fast", "1"],
            &["u", "--delete-branch", "branch"],
            &["u", "--branches", "--held"],
            &["u", "--mark-only", "--dry-run"],
            &["u", "--mark-only", "--mark-only"],
            &["u", "--dry-run", "--every", "30"],
            &["u", "--dry-run", "--compact-below"],
            &["u", "--mark-only", "1"],
        ];
        for args in lines {
            assert_eq!(parsed(args), Err(usage()), "{args:?}");
        }
    }
}
