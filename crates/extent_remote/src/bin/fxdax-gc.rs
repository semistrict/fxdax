// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's collector (backlog doc-7, Garbage collection): reclaims what no live branch in a bucket
//! can reach, and compacts what is mostly freed. Only it deletes; it coordinates with branches'
//! writers, and with other collectors, through the bucket alone.
//!
//! ```sh
//! fxdax-gc URL [--cell TENANT/CELL] [--mark-only] [--every SECONDS] [--grace SECONDS]
//!     [--compact-below FRACTION]
//! fxdax-gc URL [--cell TENANT/CELL] --dry-run [--grace SECONDS] [--compact-below FRACTION]
//! fxdax-gc URL --branches [--cell TENANT/CELL] [--held FILE]
//! fxdax-gc URL --delete-branch TENANT/CELL/ID
//! fxdax-gc URL --usage
//! ```
//!
//! URL names the bucket (`s3://bucket/prefix`, `gs://bucket/prefix`, `az://container/prefix`),
//! reached with credentials from the environment's `AWS_*`, `GOOGLE_*` or `AZURE_*` variables. It
//! collects once, or every SECONDS, a cell at a time (TASK-395.4): every cell of the bucket, found
//! without listing their objects, one after another, or the one `--cell` names, its IDs in
//! hexadecimal. Each cell's pass takes the cell's own lease, lists the cell alone, and deletes there
//! alone, but for the references it retires of its deleted children's under their parents' cells.
//! A pass that fails says so and stops none of the others; the next pass tries it again. A
//! compacted object goes, and moves no state names, a grace period after (60 s by default); objects
//! whose live extents take less than FRACTION of them (0.5) are compacted.
//!
//! `--mark-only` collects deleting nothing (TASK-264): each pass compacts as one that deletes does,
//! and writes what it would delete to its cell's `gc/marks/<epoch>` instead, retiring no
//! reference, which every reader of the cell
//! (extentd, fxdaxctl, writers recovering, this collector) warns of reaching, a bug in the
//! collector; and warns, loudly, of what the pass before marked that it finds kept, the same
//! objects still, another. Nothing marked goes: the bucket grows by what passes would have
//! reclaimed, the objects compacted and their moves both, until an operator collects it for real or
//! removes the marks. `--dry-run` writes nothing at all, not even the collector's lease, which
//! would fence a pass running meanwhile out of its deletes, and compacts nothing: it prints each
//! object a pass would delete, `KEY KIND SIZE delete` a line, and each it would compact, by its
//! listed size, `KEY KIND SIZE compact`, and each reference it would retire, `KEY reference -
//! retire`, then their counts and bytes. It finds fewer than a pass
//! deletes: what a pass deletes only once it compacted it, a grace period after, it can't find
//! doomed, those it would compact among them. A pass of either says what it did on standard
//! error, a line for each cell a mark-only pass ran over, a line for all the cells a pass that
//! deletes ran over, and logs there what warns.
//!
//! The collector keeps what every live branch inherited, whether or not a host holds the branch,
//! which only hosts know. `--branches` lists the live branches, of every cell or of the one
//! `--cell` names, one a line, a fork's after the
//! branch and the segment it forked at, each branch as TENANT/CELL/ID, all in hexadecimal, but
//! those FILE names, as `fxdaxctl stat` calls them, separated by white space: with FILE naming
//! every branch every host holds, those left are no host's, or newer than FILE. `--delete-branch`
//! deletes one, whichever host took it last, which stops writing if there is one, and the next pass
//! frees what only it held (TASK-79.85).
//!
//! `--usage` sums the bucket's objects by who they are of (TASK-394), as one listing finds them,
//! writing nothing: a line `tenant TENANT OBJECTS BYTES` for each tenant, then
//! `cell TENANT/CELL OBJECTS BYTES` for each of its cells, IDs in hexadecimal; then
//! `unowned - OBJECTS BYTES`, what is no cell's (object-ID blocks, probes, keys of no kind), and
//! `total - OBJECTS BYTES`. A cell's objects are those under its prefix: its branches', what the
//! collector compacted of them, its collector's lease and marks, and the references of forks of
//! its branches: what the cell takes, the garbage of it the collector has yet to reclaim among it.

use extent_remote::bucket::{Bucket, Meta};
use extent_remote::collector::{Collector, DryRun, Marking, Options, Report};
use extent_remote::layout;
use extent_remote::layout::Cell;
use extent_remote::record::Record;
use extent_remote::store::{Store, probe};
use extent_remote::{branch, cell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

fn usage() -> String {
    "usage: fxdax-gc URL [--cell TENANT/CELL] [--mark-only] [--every SECONDS] [--grace SECONDS] \
     [--compact-below FRACTION] | fxdax-gc URL [--cell TENANT/CELL] --dry-run [--grace SECONDS] \
     [--compact-below FRACTION] | \
     fxdax-gc URL --branches [--cell TENANT/CELL] [--held FILE] | \
     fxdax-gc URL --delete-branch TENANT/CELL/ID | fxdax-gc URL --usage \
     (every cell, or the one --cell names; --mark-only deletes nothing, writing each cell's \
     gc/marks/EPOCH: the bucket grows; --dry-run writes nothing, and finds less than a pass deletes)"
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
    /// Collects, every so long if more than once, every cell or one.
    Collect { every: Option<Duration>, options: Options, mode: Mode, cell: Option<Cell> },
    /// Lists the live branches, of every cell or one, but those a file names.
    Branches { held: Option<PathBuf>, cell: Option<Cell> },
    /// Deletes a branch.
    DeleteBranch(layout::Branch),
    /// Sums the objects of each tenant and each cell (TASK-394).
    Usage,
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
        ["--branches", rest @ ..] => parse_branches(rest)?,
        ["--delete-branch", branch] => Command::DeleteBranch(branch.parse().map_err(|_| usage())?),
        ["--usage"] => Command::Usage,
        flags => parse_collect(flags)?,
    };
    Ok(Args { url, command })
}

/// `--branches`' `flags`, after it: a cell, a file of those held, each at most once.
fn parse_branches(flags: &[&str]) -> Result<Command, String> {
    let (mut held, mut cell) = (None, None);
    let mut flags = flags.iter();
    while let Some(&flag) = flags.next() {
        let value = flags.next().ok_or_else(usage)?;
        match flag {
            "--held" if held.is_none() => held = Some(value.into()),
            "--cell" if cell.is_none() => cell = Some(value.parse().map_err(|_| usage())?),
            _ => return Err(usage()),
        }
    }
    Ok(Command::Branches { held, cell })
}

/// A pass's `flags`: a dry run's run once.
fn parse_collect(flags: &[&str]) -> Result<Command, String> {
    let (mut every, mut grace, mut compact_below) = (None, 60.0, 0.5);
    let mut mode = Mode::Delete;
    let mut cell = None;
    let mut flags = flags.iter();
    while let Some(&flag) = flags.next() {
        if flag == "--cell" {
            let named = flags.next().ok_or_else(usage)?;
            if cell.replace(named.parse().map_err(|_| usage())?).is_some() {
                return Err(usage());
            }
            continue;
        }
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
    Ok(Command::Collect { every, options, mode, cell })
}

/// The branches `text` names, in hexadecimal, separated by white space.
fn held_branches(text: &str) -> Result<BTreeSet<layout::Branch>, String> {
    let branch = |word: &str| word.parse();
    text.split_whitespace().map(branch).collect()
}

/// What `--branches` prints of `live`, but those `held`.
fn listed(live: &BTreeMap<layout::Branch, Record>, held: &BTreeSet<layout::Branch>) -> String {
    let listed = live.iter().filter(|(branch, _)| !held.contains(branch));
    listed
        .map(|(branch, record)| match record.origin() {
            Some(origin) => format!("{branch} {} {:x}\n", origin.parent, origin.at),
            None => format!("{branch}\n"),
        })
        .collect()
}

/// Objects, and their bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sum {
    objects: u64,
    bytes: u64,
}

impl Sum {
    fn add(&mut self, size: u64) {
        self.objects += 1;
        self.bytes += size;
    }
}

/// What `--usage` prints of the objects `listed`: each tenant's sum, then each of its cells', then
/// what is no cell's, and the total.
fn usage_of(listed: &[Meta]) -> String {
    let mut tenants: BTreeMap<u64, (Sum, BTreeMap<u64, Sum>)> = BTreeMap::new();
    let (mut unowned, mut total) = (Sum::default(), Sum::default());
    for meta in listed {
        total.add(meta.size);
        match layout::owner(&meta.key) {
            Some(cell) => {
                let (tenant, cells) = tenants.entry(cell.tenant).or_default();
                tenant.add(meta.size);
                cells.entry(cell.id).or_default().add(meta.size);
            }
            None => unowned.add(meta.size),
        }
    }
    let line =
        |scope: &str, id: &str, sum: Sum| format!("{scope} {id} {} {}\n", sum.objects, sum.bytes);
    let mut printed = String::new();
    for (tenant, (sum, cells)) in &tenants {
        printed += &line("tenant", &format!("{tenant:x}"), *sum);
        for (cell, sum) in cells {
            printed += &line("cell", &format!("{tenant:x}/{cell:x}"), *sum);
        }
    }
    printed += &line("unowned", "-", unowned);
    printed + &line("total", "-", total)
}

/// The cells a command goes over: the one named, or every cell of `bucket`, found by delimiter
/// listings, listing none of their objects (TASK-395.4).
async fn cells(bucket: &Store, named: Option<Cell>) -> Vec<Cell> {
    match named {
        Some(cell) => vec![cell],
        None => cell::listed(bucket).await,
    }
}

/// Collects every cell of `bucket`, or the one `named`, a pass over each, once, or every `every`,
/// deleting or marking as `mode` says; or prints what a pass would delete, once.
async fn collect(
    bucket: &Store,
    every: Option<Duration>,
    options: Options,
    mode: Mode,
    named: Option<Cell>,
) -> Result<(), String> {
    // Each pass's holder differs from every other's.
    let holder = rand::random::<u64>;
    // What it read of branches' records it reads again only once they change.
    let mut collector = Collector::new(options);
    loop {
        let cells = cells(bucket, named).await;
        match mode {
            Mode::Delete => {
                let (mut report, mut failed) = (Report::default(), 0);
                for &cell in &cells {
                    match collector.collect_cell(bucket, cell, holder()).await {
                        Ok(done) => report += done,
                        // Nothing it could not read is deleted: the next pass tries again.
                        Err(err) => {
                            failed += 1;
                            eprintln!("fxdax-gc: collecting cell {cell}: {err}");
                        }
                    }
                }
                eprintln!("{}", collected_line(&report, cells.len(), failed));
            }
            Mode::MarkOnly => {
                for &cell in &cells {
                    match collector.mark_cell(bucket, cell, holder()).await {
                        Ok(marking) => eprintln!("{}", marked_line(cell, &marking)),
                        Err(err) => eprintln!("fxdax-gc: marking cell {cell}: {err}"),
                    }
                }
            }
            Mode::DryRun => {
                let mut dry = DryRun::default();
                for &cell in &cells {
                    let found = collector.dry_run_cell(bucket, cell).await;
                    let found = found.map_err(|err| format!("cell {cell}: {err}"))?;
                    dry.doomed.extend(found.doomed);
                    dry.compactable.extend(found.compactable);
                    dry.retired.extend(found.retired);
                    dry.fenced |= found.fenced;
                }
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

/// What a sweep of passes that delete says of `report`, theirs over `cells` cells, `failed` of which
/// failed.
fn collected_line(report: &Report, cells: usize, failed: usize) -> String {
    let Report { deleted, compacted, retired } = report;
    format!(
        "fxdax-gc: deleted {deleted}, compacted {compacted}, retired {retired} references, over \
         {cells} cells, {failed} failed"
    )
}

/// What a mark-only pass over `cell` says of `marking`.
fn marked_line(cell: Cell, marking: &Marking) -> String {
    let Some(marks) = &marking.marks else {
        let compacted = marking.report.compacted;
        return format!(
            "fxdax-gc: cell {cell}: marked nothing, a later pass having taken the lease, compacted \
             {compacted}"
        );
    };
    let bytes: u64 = marks.marked.iter().map(|marked| marked.size).sum();
    format!(
        "fxdax-gc: cell {cell}: marked {} ({bytes} bytes) in {}, deleted 0, compacted {}, \
         resurrected {}",
        marks.marked.len(),
        layout::marks(cell, marks.epoch),
        marking.report.compacted,
        marking.resurrected.len()
    )
}

/// What a dry run prints of `dry`: each object a pass would delete, then each it would compact, a
/// line of its key, kind, size and which, then each reference it would retire, for standard
/// output; and the counts and bytes of them, with what they leave out, for standard error.
fn dry_run(dry: &DryRun) -> (String, String) {
    let line = |meta: &Meta, which| {
        format!("{} {} {} {which}\n", meta.key, layout::kind(&meta.key).name(), meta.size)
    };
    let listed = (dry.doomed.iter().map(|meta| line(meta, "delete")))
        .chain(dry.compactable.iter().map(|meta| line(meta, "compact")))
        .chain(dry.retired.iter().map(|key| format!("{key} reference - retire\n")))
        .collect();
    let bytes = |metas: &[Meta]| metas.iter().map(|meta| meta.size).sum::<u64>();
    let mut summary = format!(
        "fxdax-gc: dry run: a pass would delete {} objects ({} bytes), and more: it would compact \
         {} ({} bytes), and delete what it compacts a grace period after, which a dry run, \
         compacting nothing, doesn't count; and retire {} references",
        dry.doomed.len(),
        bytes(&dry.doomed),
        dry.compactable.len(),
        bytes(&dry.compactable),
        dry.retired.len(),
    );
    if dry.fenced {
        summary += "; a pass took a cell's collector lease meanwhile, deleting what this read may \
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
        // A dry run and a sum write nothing, and rely on no condition of a write's.
        if !matches!(command, Command::Collect { mode: Mode::DryRun, .. } | Command::Usage) {
            probe(&bucket, rand::random()).await?;
        }
        match command {
            Command::Collect { every, options, mode, cell } => {
                collect(&bucket, every, options, mode, cell).await
            }
            Command::Branches { held, cell } => {
                let held = match held {
                    Some(file) => std::fs::read_to_string(&file)
                        .map_err(|err| format!("{}: {err}", file.display()))?,
                    None => String::new(),
                };
                let held = held_branches(&held)?;
                let mut live = BTreeMap::new();
                for cell in cells(&bucket, cell).await {
                    let found = branch::live(&bucket, cell).await;
                    live.extend(found.map_err(|err| format!("cell {cell}: {err}"))?);
                }
                print!("{}", listed(&live, &held));
                Ok(())
            }
            // Whichever placement took it last: u64::MAX is no lower than any's token.
            Command::DeleteBranch(id) => branch::delete(&bucket, id, rand::random(), u64::MAX)
                .await
                .map_err(|err| err.to_string()),
            Command::Usage => {
                let listed = bucket.list("").await.map_err(|err| err.to_string())?;
                print!("{}", usage_of(&listed));
                Ok(())
            }
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
        let command =
            Command::Collect { every: None, options: defaults, mode: Mode::Delete, cell: None };
        assert_eq!(parsed(&["s3://bucket/prefix"]), Ok(Args { url: url.clone(), command }));
        let args = [&url, "--every", "30", "--grace", "2.5", "--compact-below", "0.25"];
        let options = Options { compact_below: 0.25, grace: 2500 };
        let every = Some(Duration::from_secs(30));
        let command = Command::Collect { every, options, mode: Mode::Delete, cell: None };
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
        let command = Command::Collect { every, options, mode: Mode::MarkOnly, cell: None };
        assert_eq!(parsed(&args), Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect { every: None, options, mode: Mode::MarkOnly, cell: None };
        assert_eq!(parsed(&["u", "--mark-only"]), Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.25, grace: 1500 };
        let command = Command::Collect { every: None, options, mode: Mode::DryRun, cell: None };
        let args = parsed(&["u", "--grace", "1.5", "--dry-run", "--compact-below", "0.25"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect { every: None, options, mode: Mode::DryRun, cell: None };
        assert_eq!(parsed(&["u", "--dry-run"]), Ok(Args { url, command }));
    }

    /// TASK-264: a dry run lists each object a pass would delete, and each it would compact, with
    /// its kind and size, and says their counts and bytes, and that a pass deletes more; and that it
    /// may be stale, if fenced.
    #[test]
    fn dry_runs_list_what_goes_and_say_it_is_less() {
        let meta = |key: String, size| Meta { key, version: Version::default(), size, modified: 0 };
        let doomed =
            vec![meta(layout::segment(branch(1), 2), 100), meta(layout::lease(branch(3)), 20)];
        let compactable = vec![meta(layout::segment(branch(1), 4), 7)];
        let parent = layout::Branch { cell: layout::Cell { tenant: 1, id: 9 }, id: 4 };
        let retired = vec![layout::reference(parent, 5, branch(6))];
        let (listed, summary) = dry_run(&DryRun { doomed, compactable, retired, fenced: false });
        let at = "tenants/0000000000000001/cells/0000000000000002/";
        let parents = "tenants/0000000000000001/cells/0000000000000009/";
        let expected = format!(
            "{at}segments/0000000000000001/0000000000000002 segment 100 delete\n\
             {at}leases/0000000000000003 lease 20 delete\n\
             {at}segments/0000000000000001/0000000000000004 segment 7 compact\n\
             {parents}refs/0000000000000004/0000000000000005/\
             0000000000000001.0000000000000002.0000000000000006 reference - retire\n"
        );
        assert_eq!(listed, expected);
        assert_eq!(
            summary,
            "fxdax-gc: dry run: a pass would delete 2 objects (120 bytes), and more: it would \
             compact 1 (7 bytes), and delete what it compacts a grace period after, which a dry \
             run, compacting nothing, doesn't count; and retire 1 references"
        );
        let (listed, summary) = dry_run(&DryRun { fenced: true, ..DryRun::default() });
        assert_eq!(listed, "");
        assert!(summary.ends_with("meanwhile, deleting what this read may name: run it again"));
        assert!(summary.contains("a pass took a cell's collector lease"), "{summary}");
    }

    /// TASK-264: a mark-only pass says what it marked, where, and what it found resurrected; or
    /// that it marked nothing, fenced.
    #[test]
    fn mark_only_passes_say_what_they_marked() {
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
            report: Report { deleted: 0, compacted: 3, retired: 0 },
            marks: Some(marks),
            resurrected: vec!["c".into()],
        };
        let cell = branch(1).cell;
        assert_eq!(
            marked_line(cell, &marking),
            "fxdax-gc: cell 1/2: marked 2 (11 bytes) in \
             tenants/0000000000000001/cells/0000000000000002/gc/marks/000000000000002a, deleted 0, \
             compacted 3, resurrected 1"
        );
        let report = Report { deleted: 0, compacted: 1, retired: 0 };
        let fenced = Marking { report, ..Marking::default() };
        assert_eq!(
            marked_line(cell, &fenced),
            "fxdax-gc: cell 1/2: marked nothing, a later pass having taken the lease, compacted 1"
        );
    }

    /// Branch `id` of tenant 1's cell 2.
    fn branch(id: u64) -> layout::Branch {
        layout::Branch { cell: layout::Cell { tenant: 1, id: 2 }, id }
    }

    /// TASK-79.85: an operator lists what no host holds, and deletes a branch, by IDs as
    /// `fxdaxctl stat` says them: TENANT/CELL/ID, in hexadecimal.
    #[test]
    fn branches_are_listed_and_deleted_by_their_hexadecimal_ids() {
        let url = "u".to_owned();
        let command = Command::Branches { held: None, cell: None };
        assert_eq!(parsed(&["u", "--branches"]), Ok(Args { url: url.clone(), command }));
        let command = Command::Branches { held: Some("hosts".into()), cell: None };
        let args = parsed(&["u", "--branches", "--held", "hosts"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let args = parsed(&["u", "--delete-branch", "1/2/ff"]);
        assert_eq!(args, Ok(Args { url, command: Command::DeleteBranch(branch(0xff)) }));
    }

    /// TASK-79.85: a list names the live branches but those held, a fork's with where it forked.
    #[test]
    fn lists_leave_out_what_is_held() {
        let base = Kind::Root.of(0);
        let fork = Kind::Fork(Origin { parent: branch(0xa), at: 0x10 }).of(0);
        let live = BTreeMap::from([(branch(0xa), base), (branch(0xb), fork), (branch(0xc), fork)]);
        let held = held_branches("1/2/c\n").unwrap();
        assert_eq!(listed(&live, &held), "1/2/a\n1/2/b 1/2/a 10\n");
        let refused = "no branch, TENANT/CELL/ID in hexadecimal: x".to_owned();
        assert_eq!(held_branches("1/2/c x"), Err(refused));
    }

    /// TASK-394: `--usage` takes no other flag, and sums each tenant's objects, then each of its
    /// cells', a cell's compacted objects with it, then what is no cell's, and all of them.
    #[test]
    fn usage_sums_each_tenant_and_cell() {
        assert_eq!(
            parsed(&["u", "--usage"]),
            Ok(Args { url: "u".into(), command: Command::Usage })
        );
        let meta = |key: String, size| Meta { key, version: Version::default(), size, modified: 0 };
        let other = layout::Branch { cell: layout::Cell { tenant: 1, id: 0x1f }, id: 1 };
        let tenant_a = layout::Branch { cell: layout::Cell { tenant: 0xa, id: 2 }, id: 3 };
        let listed = [
            meta(layout::gc_lease(branch(1).cell), 1),
            meta(layout::reference(branch(1), 3, other), 9),
            meta(layout::moves(&layout::moves(&layout::segment(other, 0))), 20),
            meta(layout::moves(&layout::segment(branch(1), 0)), 300),
            meta(layout::objectid_block(0), 2),
            meta(layout::checkpoint(branch(1), 4), 4000),
            meta(layout::cell(branch(1).cell), 50_000),
            meta(layout::segment(branch(7), 4), 600_000),
            meta(layout::segment(other, 0), 7_000_000),
            meta(layout::record(tenant_a), 80_000_000),
            meta("probe/0000000000000001".into(), 4),
        ];
        assert_eq!(
            usage_of(&listed),
            "tenant 1 8 7654330\n\
             cell 1/2 6 654310\n\
             cell 1/1f 2 7000020\n\
             tenant a 1 80000000\n\
             cell a/2 1 80000000\n\
             unowned - 2 6\n\
             total - 11 87654336\n"
        );
        assert_eq!(usage_of(&[]), "unowned - 0 0\ntotal - 0 0\n");
    }

    /// TASK-395.4: a pass of any mode, and a list of the live branches, go over every cell, or the
    /// one `--cell` names, TENANT/CELL in hexadecimal, anywhere among their flags.
    #[test]
    fn passes_and_lists_go_over_a_cell_or_all() {
        let url = "u".to_owned();
        let cell = Some(layout::Cell { tenant: 0xa, id: 0x1f });
        let options = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect { every: None, options, mode: Mode::Delete, cell };
        assert_eq!(parsed(&["u", "--cell", "a/1f"]), Ok(Args { url: url.clone(), command }));
        let command = Command::Collect { every: None, options, mode: Mode::DryRun, cell };
        let args = parsed(&["u", "--dry-run", "--cell", "a/1f"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let every = Some(Duration::from_secs(5));
        let command = Command::Collect { every, options, mode: Mode::MarkOnly, cell };
        let args = parsed(&["u", "--every", "5", "--cell", "a/1f", "--mark-only"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let command = Command::Branches { held: Some("hosts".into()), cell };
        let args = parsed(&["u", "--branches", "--cell", "a/1f", "--held", "hosts"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let command = Command::Branches { held: None, cell };
        assert_eq!(parsed(&["u", "--branches", "--cell", "a/1f"]), Ok(Args { url, command }));
    }

    /// TASK-395.4: a sweep of passes that delete says what they did over how many cells, and how
    /// many failed.
    #[test]
    fn sweeps_say_what_their_passes_did() {
        let report = Report { deleted: 3, compacted: 2, retired: 1 };
        assert_eq!(
            collected_line(&report, 7, 1),
            "fxdax-gc: deleted 3, compacted 2, retired 1 references, over 7 cells, 1 failed"
        );
    }

    #[test]
    fn other_command_lines_get_the_usage() {
        let lines: [&[&str]; 21] = [
            &[],
            &["u", "--every"],
            &["u", "--every", "soon"],
            &["u", "--fast", "1"],
            &["u", "--delete-branch", "branch"],
            &["u", "--delete-branch", "ff"],
            &["u", "--branches", "--held"],
            &["u", "--mark-only", "--dry-run"],
            &["u", "--mark-only", "--mark-only"],
            &["u", "--dry-run", "--every", "30"],
            &["u", "--dry-run", "--compact-below"],
            &["u", "--mark-only", "1"],
            &["u", "--usage", "--dry-run"],
            &["u", "--dry-run", "--usage"],
            &["u", "--cell"],
            &["u", "--cell", "1"],
            &["u", "--cell", "1/2/3"],
            &["u", "--cell", "1/2", "--cell", "1/3"],
            &["u", "--branches", "--cell", "x/2"],
            &["u", "--branches", "--cell", "1/2", "--cell", "1/2"],
            &["u", "--usage", "--cell", "1/2"],
        ];
        for args in lines {
            assert_eq!(parsed(args), Err(usage()), "{args:?}");
        }
    }
}
