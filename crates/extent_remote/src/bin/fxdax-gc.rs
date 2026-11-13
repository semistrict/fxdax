// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's collector (backlog doc-7, Garbage collection): reclaims what no live branch in a bucket
//! can reach, and compacts what is mostly freed. Only it deletes; it coordinates with branches'
//! writers, and with other collectors, through the bucket alone.
//!
//! ```sh
//! fxdax-gc URL [--cell TENANT/CELL] [--mark-only] [--every SECONDS] [--grace SECONDS]
//!     [--compact-below FRACTION] [--at-once PASSES] [--lease SECONDS] [--metrics FILE]
//! fxdax-gc URL [--cell TENANT/CELL] --dry-run [--grace SECONDS] [--compact-below FRACTION]
//! fxdax-gc URL --branches [--cell TENANT/CELL] [--held FILE]
//! fxdax-gc URL --delete-branch TENANT/CELL/ID
//! fxdax-gc URL --usage [--tenant TENANT]
//! ```
//!
//! URL names the bucket (`s3://bucket/prefix`, `gs://bucket/prefix`, `az://container/prefix`),
//! reached with credentials from the environment's `AWS_*`, `GOOGLE_*` or `AZURE_*` variables. It
//! collects once, or every SECONDS, a cell at a time (TASK-395.4): every cell of the bucket, found
//! by delimiter listings a page at a time, listing none of their objects, PASSES of them at once
//! (16 by default, TASK-395.5; `extent_remote::sweep`), or the one `--cell` names, its IDs in
//! hexadecimal. Each cell's pass takes the cell's own lease, lists the cell alone, and deletes there
//! alone, but for the references it retires of its deleted children's under their parents' cells.
//! It holds the lease for `--lease` SECONDS (600 by default), renewing it between its steps and
//! letting it go as it ends: a pass, of this process or of any other collecting the bucket at
//! once, that finds its cell held leaves it and goes on to the next, as it does a cell another
//! pass went over since its sweep began; so processes collecting one bucket at once split its
//! cells. A process gone holds the cells its passes held until their leases expire, and the next
//! sweep after that collects them. A pass that fails says so and stops none of the others; the next
//! pass tries it again. A compacted object goes, and moves no state names, a grace period after (60
//! s by default); objects whose live extents take less than FRACTION of them (0.5) are compacted.
//!
//! Each sweep says on standard error, a line for each pass that deleted, compacted or retired
//! something, or failed, what it did, in how long, and the requests it made of each kind; then two
//! lines of all its passes: the objects and bytes deleted and compacted, the references retired,
//! the cells gone over, those left to passes holding them and to passes that went over them since,
//! and those failed, and the requests of each kind, listing the cells among them; then how long it
//! took, and its slowest passes. With `--metrics`, it writes the same of its last sweep to FILE
//! after each, in Prometheus' text format, for a node exporter's textfile collector: see
//! [`metrics`].
//!
//! `--mark-only` collects deleting nothing (TASK-264): each pass compacts as one that deletes does,
//! and writes what it would delete to its cell's `gc/marks/<epoch>` instead, retiring no
//! reference, and `gc/marking`, which readers watch the cells' marks only while it is there: an
//! operator removes it with the marks. Every reader of the cell
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
//! `--usage` says what each cell takes, and each tenant, every cell's or `--tenant`'s alone, as
//! the collector's last pass over each cell to end found it, writing nothing (TASK-394,
//! TASK-395.8): it reads each cell's collector lease, where that pass recorded it, listing none of
//! the cells' objects, the tenants and their cells listed a page at a time, so that what it holds
//! at once is a page of each, however many tenants, cells and objects the bucket holds. It streams a JSON object a line: one for each cell of a tenant, then one for
//! the tenant, IDs in hexadecimal. A cell's: `{"scope":"cell","tenant":"1","cell":"1/2f",
//! "measured_unix_ms":...,"objects":...,"bytes":...,"kinds":{"segment":{"objects":...,
//! "bytes":...},...}}`, `measured_unix_ms` when the listing it is of began, by the bucket's clock,
//! and all four null for a cell no pass ran over to its end yet. A tenant's: `{"scope":"tenant",
//! "tenant":"1","cells":...,"measured_cells":...,"oldest_measured_unix_ms":...,
//! "newest_measured_unix_ms":...,"objects":...,"bytes":...,"kinds":{...}}`, the sum of its cells
//! measured. A cell's objects are those under its prefix, of each kind: its branches', what the
//! collector compacted of them, its collector's lease and marks, and the references of forks of its
//! branches; the garbage the collector has yet to reclaim among them; bytes as stored. A pass that
//! skips a cell unchanged since the last measures it not again: its usage is as of the last pass
//! that went over it in full, which nothing under its prefix changed since.

use extent_remote::bucket::Meta;
use extent_remote::collector::{Collector, DryRun, Marking, Options, Report};
use extent_remote::layout;
use extent_remote::layout::Cell;
use extent_remote::passed::{self, Sum, Usage};
use extent_remote::record::Record;
use extent_remote::store::{Request, Store, probe};
use extent_remote::sweep::{self, Outcome, Ran, Requests, Sweep, Swept};
use extent_remote::{branch, cell};
use futures::{Stream, StreamExt as _};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn usage() -> String {
    "usage: fxdax-gc URL [--cell TENANT/CELL] [--mark-only] [--every SECONDS] [--grace SECONDS] \
     [--compact-below FRACTION] [--at-once PASSES] [--lease SECONDS] [--metrics FILE] | \
     fxdax-gc URL [--cell TENANT/CELL] --dry-run [--grace SECONDS] \
     [--compact-below FRACTION] | \
     fxdax-gc URL --branches [--cell TENANT/CELL] [--held FILE] | \
     fxdax-gc URL --delete-branch TENANT/CELL/ID | fxdax-gc URL --usage [--tenant TENANT] \
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
    Collect { every: Option<Duration>, options: Options, mode: Mode, cell: Option<Cell>, run: Run },
    /// Lists the live branches, of every cell or one, but those a file names.
    Branches { held: Option<PathBuf>, cell: Option<Cell> },
    /// Deletes a branch.
    DeleteBranch(layout::Branch),
    /// Says what each cell takes, and each tenant, every tenant's or one's, as recorded (TASK-394,
    /// TASK-395.8).
    Usage { tenant: Option<u64> },
}

/// How a sweep runs its passes, and what it writes of them (TASK-395.5).
#[derive(Debug, PartialEq)]
struct Run {
    /// How many at once.
    at_once: usize,
    /// How long each holds its cell, in milliseconds.
    lease: u64,
    /// Where it writes its metrics, if anywhere.
    metrics: Option<PathBuf>,
}

impl Default for Run {
    fn default() -> Self {
        Self { at_once: 16, lease: 600_000, metrics: None }
    }
}

/// How many of a sweep's slowest passes it says.
const SLOWEST: usize = 5;

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
        ["--usage"] => Command::Usage { tenant: None },
        ["--usage", "--tenant", tenant] => {
            Command::Usage { tenant: Some(tenant_id(tenant).ok_or_else(usage)?) }
        }
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
    let mut run = Run::default();
    let mut flags = flags.iter();
    while let Some(&flag) = flags.next() {
        match flag {
            "--cell" => {
                let named = flags.next().ok_or_else(usage)?;
                if cell.replace(named.parse().map_err(|_| usage())?).is_some() {
                    return Err(usage());
                }
                continue;
            }
            "--at-once" => {
                let passes = flags.next().ok_or_else(usage)?.parse().map_err(|_| usage())?;
                run.at_once = Some(passes).filter(|&passes| passes > 0).ok_or_else(usage)?;
                continue;
            }
            "--metrics" if run.metrics.is_none() => {
                run.metrics = Some(flags.next().ok_or_else(usage)?.into());
                continue;
            }
            _ => {}
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
            "--lease" => run.lease = (value()? * 1000.0) as u64,
            _ => return Err(usage()),
        }
    }
    // A dry run runs once, a pass at a time, holding no lease and writing nothing.
    if mode == Mode::DryRun && (every.is_some() || run != Run::default()) {
        return Err(usage());
    }
    let options = Options { compact_below, grace: (grace * 1000.0) as u64 };
    Ok(Command::Collect { every, options, mode, cell, run })
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

/// The tenant `text` names, in hexadecimal, as a cell's first part.
fn tenant_id(text: &str) -> Option<u64> {
    let digits =
        !text.is_empty() && text.len() <= 16 && text.bytes().all(|b| b.is_ascii_hexdigit());
    digits.then(|| u64::from_str_radix(text, 16).ok()).flatten()
}

/// How many cells' leases `--usage` reads at once.
const USAGES_AT_ONCE: usize = 32;

/// `kinds`, as `--usage` says them: each kind's objects and bytes, by its name.
fn kinds_json(kinds: &BTreeMap<String, Sum>) -> Value {
    let kind = |sum: &Sum| json!({ "objects": sum.objects, "bytes": sum.bytes });
    Value::Object(kinds.iter().map(|(name, sum)| (name.clone(), kind(sum))).collect())
}

/// What `--usage` says of `cell`, of `usage`, as its last pass to end recorded it, if one did.
fn cell_line(cell: Cell, usage: Option<&Usage>) -> String {
    let total = usage.map(Usage::total);
    json!({
        "scope": "cell",
        "tenant": format!("{:x}", cell.tenant),
        "cell": cell.to_string(),
        "measured_unix_ms": usage.map(|usage| usage.measured),
        "objects": total.map(|total| total.objects),
        "bytes": total.map(|total| total.bytes),
        "kinds": usage.map(|usage| kinds_json(&usage.kinds)),
    })
    .to_string()
}

/// What a tenant's cells take, as `--usage` reads them one after another.
#[derive(Default)]
struct Tenant {
    cells: u64,
    measured: u64,
    /// The oldest and newest of its cells' measurements.
    when: Option<(u64, u64)>,
    kinds: BTreeMap<String, Sum>,
}

impl Tenant {
    /// Adds one of its cells, of `usage`, if a pass recorded it.
    fn add(&mut self, usage: Option<&Usage>) {
        self.cells += 1;
        let Some(usage) = usage else { return };
        self.measured += 1;
        let at = usage.measured;
        self.when = Some(self.when.map_or((at, at), |(old, new)| (old.min(at), new.max(at))));
        for (name, sum) in &usage.kinds {
            *self.kinds.entry(name.clone()).or_default() += *sum;
        }
    }

    /// What `--usage` says of it, `tenant`.
    fn line(&self, tenant: u64) -> String {
        let mut total = Sum::default();
        self.kinds.values().for_each(|sum| total += *sum);
        json!({
            "scope": "tenant",
            "tenant": format!("{tenant:x}"),
            "cells": self.cells,
            "measured_cells": self.measured,
            "oldest_measured_unix_ms": self.when.map(|(oldest, _)| oldest),
            "newest_measured_unix_ms": self.when.map(|(_, newest)| newest),
            "objects": total.objects,
            "bytes": total.bytes,
            "kinds": kinds_json(&self.kinds),
        })
        .to_string()
    }
}

/// Writes to `out` what each cell of `bucket` takes, and each tenant, every tenant's or `named`'s, a
/// line each as it reads them (`--usage`): reading each cell's collector lease, a few at once, the
/// tenants and each one's cells listed a page at a time, so that it holds a page of each at most,
/// whatever the bucket holds (TASK-395.8).
async fn say_usage(
    bucket: &Store,
    named: Option<u64>,
    out: &mut impl std::io::Write,
) -> Result<(), String> {
    let tenants = match named {
        Some(tenant) => futures::stream::iter([tenant]).left_stream(),
        None => cell::tenants(bucket).right_stream(),
    };
    let mut tenants = std::pin::pin!(tenants);
    while let Some(tenant) = tenants.next().await {
        let reads = cell::of_tenant(bucket, tenant)
            .map(|cell| async move { (cell, passed::read(bucket, cell).await) })
            .buffered(USAGES_AT_ONCE);
        let mut reads = std::pin::pin!(reads);
        let mut sum = Tenant::default();
        while let Some((cell, read)) = reads.next().await {
            let lease = read.map_err(|err| format!("cell {cell}: {err}"))?;
            let usage = lease.and_then(|lease| lease.passed).map(|passed| passed.usage);
            writeln!(out, "{}", cell_line(cell, usage.as_ref())).map_err(|err| err.to_string())?;
            sum.add(usage.as_ref());
        }
        writeln!(out, "{}", sum.line(tenant)).map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// The cells a command goes over: the one named, or every cell of `bucket`, found by delimiter
/// listings a page at a time, listing none of their objects (TASK-395.4, TASK-395.5).
fn cells(bucket: &Store, named: Option<Cell>) -> impl Stream<Item = Cell> + '_ {
    match named {
        Some(cell) => futures::stream::iter([cell]).left_stream(),
        None => cell::listed(bucket).right_stream(),
    }
}

/// Collects every cell of `bucket`, or the one `named`, a pass over each, once, or every `every`,
/// deleting or marking as `mode` says, as `run` says; or prints what a pass would delete, once.
async fn collect(
    bucket: &Arc<Store>,
    every: Option<Duration>,
    options: Options,
    mode: Mode,
    named: Option<Cell>,
    run: Run,
) -> Result<(), String> {
    let swept_mode = match mode {
        Mode::Delete => sweep::Mode::Delete,
        Mode::MarkOnly => sweep::Mode::MarkOnly,
        Mode::DryRun => return dry_run_cells(bucket, options, named).await,
    };
    let sweep = Sweep {
        options,
        mode: swept_mode,
        at_once: run.at_once,
        lease: run.lease,
        slowest: SLOWEST,
    };
    let mut sweeps = 0;
    loop {
        let mut passes = Histogram::default();
        let observe = |ran: &Ran| {
            passes.observe(ran);
            if let Some(line) = passed_line(ran) {
                eprintln!("{line}");
            }
        };
        let swept = match named {
            Some(cell) => sweep.run_cell(bucket, cell, observe).await,
            None => sweep.run(bucket, observe).await,
        };
        sweeps += 1;
        let (did, took) = swept_lines(mode, &swept);
        eprintln!("{did}\n{took}");
        if let Some(file) = &run.metrics {
            let written = metrics(sweeps, &swept, &passes, peak_resident(), SystemTime::now());
            write_atomically(file, &written).map_err(|err| format!("{}: {err}", file.display()))?;
        }
        let Some(every) = every else { return Ok(()) };
        tokio::time::sleep(every).await;
    }
}

/// What a dry run over every cell of `bucket`, or the one `named`, finds, printed, a cell at a
/// time.
async fn dry_run_cells(
    bucket: &Store,
    options: Options,
    named: Option<Cell>,
) -> Result<(), String> {
    let collector = Collector::new(options);
    let mut dry = DryRun::default();
    let mut cells = std::pin::pin!(cells(bucket, named));
    while let Some(cell) = cells.next().await {
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
    Ok(())
}

/// What the requests `requests` were, of each kind made: `get 3, list 1`; `none` if none.
fn requests_text(requests: &Requests) -> String {
    let made: Vec<String> = (Request::ALL.into_iter())
        .filter(|&request| requests.of(request) > 0)
        .map(|request| format!("{} {}", request.name(), requests.of(request)))
        .collect();
    if made.is_empty() { "none".to_owned() } else { made.join(", ") }
}

/// What a pass that deleted, compacted or retired something, or failed, says of `passed`: none
/// for one that did nothing, skipping its cell or not, or left its cell to another. A mark-only
/// pass says what it marked whatever it did.
fn passed_line(passed: &Ran) -> Option<String> {
    let Ran { cell, outcome, requests, took } = passed;
    let tail = format!("in {:.3} s; requests: {}", took.as_secs_f64(), requests_text(requests));
    match outcome {
        Outcome::Collected(report) if Report { skipped: 0, ..*report } == Report::default() => None,
        // One that skipped its cell, unchanged since its last pass, deleting what that one
        // deferred, or retiring again what it retired (TASK-395.6).
        Outcome::Collected(report) if report.skipped > 0 => {
            Some(format!("fxdax-gc: cell {cell}, unchanged: {}, {tail}", did(report)))
        }
        Outcome::Collected(report) => {
            Some(format!("fxdax-gc: cell {cell}: {}, {tail}", did(report)))
        }
        Outcome::Marked(marking) => Some(format!("{}, {tail}", marked_line(*cell, marking))),
        Outcome::Left(_) => None,
        Outcome::Failed(err) => Some(format!("fxdax-gc: collecting cell {cell}: {err}, {tail}")),
    }
}

/// What passes that delete did, as `report` says.
fn did(report: &Report) -> String {
    let Report { deleted, compacted, retired, deleted_bytes, compacted_bytes, .. } = report;
    format!(
        "deleted {deleted} ({deleted_bytes} bytes), compacted {compacted} ({compacted_bytes} \
         bytes), retired {retired} references"
    )
}

/// What a sweep of `mode` says of `swept`: what its passes did, and the requests it made; then how
/// long it took, and its slowest passes.
fn swept_lines(mode: Mode, swept: &Swept) -> (String, String) {
    let Swept { visited, held, swept: since, failed, report, requests, slowest, took, .. } = swept;
    let did = match mode {
        Mode::MarkOnly => format!(
            "marked {} ({} bytes), compacted {} ({} bytes), resurrected {}",
            swept.marked,
            swept.marked_bytes,
            report.compacted,
            report.compacted_bytes,
            swept.resurrected
        ),
        _ => did(report),
    };
    let did = format!(
        "fxdax-gc: {did}, over {visited} cells ({} unchanged), {held} held by other passes, \
         {since} swept by them since, {failed} failed; requests: {}",
        report.skipped,
        requests_text(requests)
    );
    let slowest: Vec<String> = (slowest.iter())
        .map(|(cell, took)| format!("{cell} {:.3} s", took.as_secs_f64()))
        .collect();
    let slowest = if slowest.is_empty() { "none".to_owned() } else { slowest.join(", ") };
    (did, format!("fxdax-gc: took {:.3} s; slowest: {slowest}", took.as_secs_f64()))
}

/// The bounds, in seconds, of [`Histogram`]'s buckets.
const BOUNDS: [f64; 7] = [0.001, 0.01, 0.1, 1.0, 10.0, 60.0, 600.0];

/// How long a sweep's passes over cells took, those it ran: how many took no longer than each
/// of [`BOUNDS`], and how long they took together.
#[derive(Debug, Default)]
struct Histogram {
    within: [u64; BOUNDS.len()],
    count: u64,
    sum: f64,
}

impl Histogram {
    fn observe(&mut self, passed: &Ran) {
        if matches!(passed.outcome, Outcome::Left(_)) {
            return;
        }
        let took = passed.took.as_secs_f64();
        for (within, bound) in self.within.iter_mut().zip(BOUNDS) {
            *within += u64::from(took <= bound);
        }
        self.count += 1;
        self.sum += took;
    }
}

/// What `--metrics` writes after the process's `sweeps`th sweep, `swept`, whose passes took as
/// `passes` says, ended at `now`, the process having taken `peak` bytes of memory at most so far:
/// in Prometheus' text format, the last sweep's.
///
/// - `fxdax_gc_sweeps_total`: the sweeps the process ran; `fxdax_gc_sweep_end_timestamp_seconds`
///   and `fxdax_gc_sweep_seconds`: when its last ended, and how long it took.
/// - `fxdax_gc_sweep_cells{outcome}`: its cells, by what came of each: `passed` (in full),
///   `skipped` (unchanged since its last pass, TASK-395.6), `failed`, `held` (left to a pass
///   holding it), `swept` (left to a pass that went over it since).
/// - `fxdax_gc_sweep_objects{action}` and `fxdax_gc_sweep_bytes{action}`: what its passes
///   `deleted`, `compacted` and `marked`, as listed; `fxdax_gc_sweep_retired_references` and
///   `fxdax_gc_sweep_resurrected_objects`.
/// - `fxdax_gc_sweep_requests{request}`: the requests it made of each kind
///   (`extent_remote::store::Request::name`), listing the cells among them.
/// - `fxdax_gc_pass_seconds`: a histogram of how long its passes took, those it ran;
///   `fxdax_gc_slowest_pass_seconds{cell}`: its slowest passes, by their cells, TENANT/CELL.
/// - `fxdax_gc_peak_resident_bytes`: the most memory the process held so far.
fn metrics(sweeps: u64, swept: &Swept, passes: &Histogram, peak: u64, now: SystemTime) -> String {
    let mut text = String::new();
    let mut family = |name: &str, kind: &str, help: &str, samples: &[(String, String)]| {
        text += &format!("# HELP {name} {help}\n# TYPE {name} {kind}\n");
        for (labels, value) in samples {
            text += &format!("{name}{labels} {value}\n");
        }
    };
    let one = |value: String| [(String::new(), value)];
    let ended = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
    family("fxdax_gc_sweeps_total", "counter", "Sweeps the process ran.", &one(sweeps.to_string()));
    family(
        "fxdax_gc_sweep_end_timestamp_seconds",
        "gauge",
        "When its last sweep ended.",
        &one(format!("{ended:.3}")),
    );
    family(
        "fxdax_gc_sweep_seconds",
        "gauge",
        "How long its last sweep took.",
        &one(format!("{:.6}", swept.took.as_secs_f64())),
    );
    let skipped = swept.report.skipped;
    let passed = swept.visited - swept.failed - skipped;
    let cells = [
        ("passed", passed),
        ("skipped", skipped),
        ("failed", swept.failed),
        ("held", swept.held),
        ("swept", swept.swept),
    ];
    let labelled = |label: &str, samples: &[(&str, String)]| -> Vec<(String, String)> {
        (samples.iter())
            .map(|(name, value)| (format!("{{{label}=\"{name}\"}}"), value.clone()))
            .collect()
    };
    let cells: Vec<(&str, String)> =
        cells.iter().map(|(name, count)| (*name, count.to_string())).collect();
    family(
        "fxdax_gc_sweep_cells",
        "gauge",
        "The cells of its last sweep, by what came of each.",
        &labelled("outcome", &cells),
    );
    let report = &swept.report;
    let objects = [
        ("deleted", report.deleted.to_string()),
        ("compacted", report.compacted.to_string()),
        ("marked", swept.marked.to_string()),
    ];
    family(
        "fxdax_gc_sweep_objects",
        "gauge",
        "What its last sweep's passes did, in objects.",
        &labelled("action", &objects),
    );
    let bytes = [
        ("deleted", report.deleted_bytes.to_string()),
        ("compacted", report.compacted_bytes.to_string()),
        ("marked", swept.marked_bytes.to_string()),
    ];
    family(
        "fxdax_gc_sweep_bytes",
        "gauge",
        "What its last sweep's passes did, in bytes, as listed.",
        &labelled("action", &bytes),
    );
    family(
        "fxdax_gc_sweep_retired_references",
        "gauge",
        "The references its last sweep's passes retired.",
        &one(report.retired.to_string()),
    );
    family(
        "fxdax_gc_sweep_resurrected_objects",
        "gauge",
        "What its last sweep's mark-only passes found kept that the marks before named.",
        &one(swept.resurrected.to_string()),
    );
    let requests: Vec<(&str, String)> = (Request::ALL.into_iter())
        .map(|request| (request.name(), swept.requests.of(request).to_string()))
        .collect();
    family(
        "fxdax_gc_sweep_requests",
        "gauge",
        "The requests its last sweep made, by kind.",
        &labelled("request", &requests),
    );
    let mut buckets: Vec<(String, String)> = (BOUNDS.iter().zip(passes.within))
        .map(|(bound, within)| (format!("_bucket{{le=\"{bound}\"}}"), within.to_string()))
        .collect();
    buckets.push(("_bucket{le=\"+Inf\"}".to_owned(), passes.count.to_string()));
    buckets.push(("_sum".to_owned(), format!("{:.6}", passes.sum)));
    buckets.push(("_count".to_owned(), passes.count.to_string()));
    family(
        "fxdax_gc_pass_seconds",
        "histogram",
        "How long its last sweep's passes took.",
        &buckets,
    );
    let slowest: Vec<(String, String)> = (swept.slowest.iter())
        .map(|(cell, took)| (format!("{{cell=\"{cell}\"}}"), format!("{:.6}", took.as_secs_f64())))
        .collect();
    family(
        "fxdax_gc_slowest_pass_seconds",
        "gauge",
        "Its last sweep's slowest passes, by cell.",
        &slowest,
    );
    family(
        "fxdax_gc_peak_resident_bytes",
        "gauge",
        "The most memory the process held so far.",
        &one(peak.to_string()),
    );
    text
}

/// The most memory the process held so far, in bytes (`getrusage`'s maximum resident set size).
fn peak_resident() -> u64 {
    // SAFETY: `getrusage` writes the `rusage` it is given, which is plain data, all of it valid
    // zeroed.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let max = u64::try_from(usage.ru_maxrss).unwrap_or(0);
    // In bytes on macOS, in KiB elsewhere.
    if cfg!(target_os = "macos") { max } else { max * 1024 }
}

/// Writes `text` to `file` whole: to a file beside it first, then renamed over it, so that a
/// reader never finds it half written.
fn write_atomically(file: &std::path::Path, text: &str) -> std::io::Result<()> {
    let mut beside = file.as_os_str().to_owned();
    beside.push(".new");
    std::fs::write(&beside, text)?;
    std::fs::rename(&beside, file)
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
    let bucket = Arc::new(Store::from_url(&url)?);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("a runtime: {err}"))?;
    runtime.block_on(async {
        // A dry run and a sum write nothing, and rely on no condition of a write's.
        if !matches!(command, Command::Collect { mode: Mode::DryRun, .. } | Command::Usage { .. }) {
            probe(&*bucket, rand::random()).await?;
        }
        match command {
            Command::Collect { every, options, mode, cell, run } => {
                collect(&bucket, every, options, mode, cell, run).await
            }
            Command::Branches { held, cell } => {
                let held = match held {
                    Some(file) => std::fs::read_to_string(&file)
                        .map_err(|err| format!("{}: {err}", file.display()))?,
                    None => String::new(),
                };
                let held = held_branches(&held)?;
                let mut live = BTreeMap::new();
                let mut cells = std::pin::pin!(cells(&bucket, cell));
                while let Some(cell) = cells.next().await {
                    let found = branch::live(&*bucket, cell).await;
                    live.extend(found.map_err(|err| format!("cell {cell}: {err}"))?);
                }
                print!("{}", listed(&live, &held));
                Ok(())
            }
            // Whichever placement took it last: u64::MAX is no lower than any's token.
            Command::DeleteBranch(id) => branch::delete(&*bucket, id, rand::random(), u64::MAX)
                .await
                .map_err(|err| err.to_string()),
            Command::Usage { tenant } => {
                say_usage(&bucket, tenant, &mut std::io::stdout().lock()).await
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
        let (mode, run) = (Mode::Delete, Run::default());
        assert_eq!(run, Run { at_once: 16, lease: 600_000, metrics: None });
        let command = Command::Collect { every: None, options: defaults, mode, cell: None, run };
        assert_eq!(parsed(&["s3://bucket/prefix"]), Ok(Args { url: url.clone(), command }));
        let args = [&url, "--every", "30", "--grace", "2.5", "--compact-below", "0.25"];
        let options = Options { compact_below: 0.25, grace: 2500 };
        let every = Some(Duration::from_secs(30));
        let run = Run::default();
        let command = Command::Collect { every, options, mode: Mode::Delete, cell: None, run };
        assert_eq!(parsed(&args), Ok(Args { url: url.clone(), command }));
        // TASK-395.5: so many passes at once, each holding its cell so long, writing metrics.
        let args =
            [&url, "--at-once", "64", "--lease", "90.5", "--metrics", "gc.prom", "--grace", "1"];
        let options = Options { compact_below: 0.5, grace: 1000 };
        let run = Run { at_once: 64, lease: 90_500, metrics: Some("gc.prom".into()) };
        let command =
            Command::Collect { every: None, options, mode: Mode::Delete, cell: None, run };
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
        let command = Command::Collect {
            every,
            options,
            mode: Mode::MarkOnly,
            cell: None,
            run: Run::default(),
        };
        assert_eq!(parsed(&args), Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect {
            every: None,
            options,
            mode: Mode::MarkOnly,
            cell: None,
            run: Run::default(),
        };
        assert_eq!(parsed(&["u", "--mark-only"]), Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.25, grace: 1500 };
        let command = Command::Collect {
            every: None,
            options,
            mode: Mode::DryRun,
            cell: None,
            run: Run::default(),
        };
        let args = parsed(&["u", "--grace", "1.5", "--dry-run", "--compact-below", "0.25"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let options = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect {
            every: None,
            options,
            mode: Mode::DryRun,
            cell: None,
            run: Run::default(),
        };
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
            report: Report { compacted: 3, compacted_bytes: 90, ..Report::default() },
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
        let report = Report { compacted: 1, ..Report::default() };
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

    /// TASK-394, TASK-395.8: `--usage` takes no other flag but `--tenant`, in hexadecimal.
    #[test]
    fn usage_goes_over_every_tenant_or_one() {
        let usage = |tenant| Ok(Args { url: "u".into(), command: Command::Usage { tenant } });
        assert_eq!(parsed(&["u", "--usage"]), usage(None));
        assert_eq!(parsed(&["u", "--usage", "--tenant", "2f"]), usage(Some(0x2f)));
        assert_eq!(
            parsed(&["u", "--usage", "--tenant", "ffffffffffffffff"]),
            usage(Some(u64::MAX))
        );
    }

    /// TASK-395.8's AC #2: `--usage` holds a page of a tenant's cells at a time, whatever their
    /// number: each page listed only as the lines of the cells before it are written, the leases
    /// read at most [`USAGES_AT_ONCE`] ahead of them, a tenant of more cells than two pages among
    /// others; each cell's line once, in order, then its tenant's.
    #[test]
    fn usage_lists_a_page_of_cells_at_a_time() {
        use extent_remote::Bucket as _;
        use extent_remote::bucket::{LIST_PAGE, PutMode};
        let bucket = Store::from_url("memory:///").unwrap();
        let many = LIST_PAGE as u64 * 2 + 5;
        let mut cells: Vec<Cell> = (0..many).map(|id| Cell { tenant: 2, id }).collect();
        cells.extend([Cell { tenant: 1, id: 7 }, Cell { tenant: 3, id: 1 }]);
        cells.sort();
        let tokio = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio.block_on(async {
            for &cell in &cells {
                let key = layout::record(layout::Branch { cell, id: 1 });
                bucket.put(&key, vec![1].into(), PutMode::Create).await.unwrap();
            }
        });
        // Each line written, and how many pages of listings by delimiter were asked for by then.
        struct Lines<'a> {
            bucket: &'a Store,
            lines: Vec<(String, u64)>,
            line: Vec<u8>,
        }
        impl std::io::Write for Lines<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                for &byte in bytes {
                    if byte != b'\n' {
                        self.line.push(byte);
                        continue;
                    }
                    let line = String::from_utf8(std::mem::take(&mut self.line)).unwrap();
                    let listed = self.bucket.stats().requests(Request::ListDelimited);
                    self.lines.push((line, listed));
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut out = Lines { bucket: &bucket, lines: Vec::new(), line: Vec::new() };
        tokio.block_on(say_usage(&bucket, None, &mut out)).unwrap();
        let cell_lines: Vec<String> = (out.lines.iter())
            .filter(|(line, _)| line.contains(r#""scope":"cell""#))
            .map(|(line, _)| line.clone())
            .collect();
        let expected: Vec<String> = cells.iter().map(|&cell| cell_line(cell, None)).collect();
        assert_eq!(cell_lines, expected);
        // The lines of tenant 2's cells: the tenants' page and tenant 1's cells' page listed before
        // them, then a page of tenant 2's for each [`LIST_PAGE`] cells read, [`USAGES_AT_ONCE`]
        // ahead of their lines at most.
        let tenant = Value::from("2");
        let of_tenant: Vec<u64> = (out.lines.iter())
            .filter(|(line, _)| {
                let line: Value = serde_json::from_str(line).unwrap();
                line["scope"] == "cell" && line["tenant"] == tenant
            })
            .map(|&(_, listed)| listed)
            .collect();
        assert_eq!(of_tenant.len() as u64, many);
        for (index, &listed) in of_tenant.iter().enumerate() {
            let read = (index + USAGES_AT_ONCE - 1).min(many as usize - 1);
            let pages = read / LIST_PAGE + 1;
            assert_eq!(listed, 2 + pages as u64, "the line of tenant 2's cell {index}");
        }
        // Tenants, then tenant 1's cells, tenant 2's in three pages, tenant 3's.
        assert_eq!(bucket.stats().requests(Request::ListDelimited), 6);
    }

    /// TASK-395.8: `--usage` says a cell's objects and bytes, of each kind and in all, and when they
    /// were measured, a JSON object a line, IDs in hexadecimal; nulls for a cell not measured. A
    /// tenant's line sums its cells measured, and says how many there are, and the oldest and
    /// newest measurement.
    #[test]
    fn usage_says_each_cell_and_tenant_as_measured() {
        let cell = layout::Cell { tenant: 0xa, id: 0x2f };
        let mut usage = Usage { measured: 1_700_000_000_000, ..Usage::default() };
        let usage = {
            usage.kinds.insert("segment".into(), Sum { objects: 2, bytes: 300 });
            usage.kinds.insert("record".into(), Sum { objects: 1, bytes: 40 });
            usage
        };
        assert_eq!(
            cell_line(cell, Some(&usage)),
            r#"{"bytes":340,"cell":"a/2f","kinds":{"record":{"bytes":40,"objects":1},"segment":{"bytes":300,"objects":2}},"measured_unix_ms":1700000000000,"objects":3,"scope":"cell","tenant":"a"}"#
        );
        assert_eq!(
            cell_line(cell, None),
            r#"{"bytes":null,"cell":"a/2f","kinds":null,"measured_unix_ms":null,"objects":null,"scope":"cell","tenant":"a"}"#
        );
        let mut later = Usage { measured: 1_700_000_000_500, ..Usage::default() };
        later.kinds.insert("segment".into(), Sum { objects: 1, bytes: 7 });
        let mut tenant = Tenant::default();
        tenant.add(Some(&later));
        tenant.add(None);
        tenant.add(Some(&usage));
        assert_eq!(
            tenant.line(0xa),
            r#"{"bytes":347,"cells":3,"kinds":{"record":{"bytes":40,"objects":1},"segment":{"bytes":307,"objects":3}},"measured_cells":2,"newest_measured_unix_ms":1700000000500,"objects":4,"oldest_measured_unix_ms":1700000000000,"scope":"tenant","tenant":"a"}"#
        );
        assert_eq!(
            Tenant::default().line(1),
            r#"{"bytes":0,"cells":0,"kinds":{},"measured_cells":0,"newest_measured_unix_ms":null,"objects":0,"oldest_measured_unix_ms":null,"scope":"tenant","tenant":"1"}"#
        );
    }

    /// TASK-395.4: a pass of any mode, and a list of the live branches, go over every cell, or the
    /// one `--cell` names, TENANT/CELL in hexadecimal, anywhere among their flags.
    #[test]
    fn passes_and_lists_go_over_a_cell_or_all() {
        let url = "u".to_owned();
        let cell = Some(layout::Cell { tenant: 0xa, id: 0x1f });
        let options = Options { compact_below: 0.5, grace: 60_000 };
        let command = Command::Collect {
            every: None,
            options,
            mode: Mode::Delete,
            cell,
            run: Run::default(),
        };
        assert_eq!(parsed(&["u", "--cell", "a/1f"]), Ok(Args { url: url.clone(), command }));
        let command = Command::Collect {
            every: None,
            options,
            mode: Mode::DryRun,
            cell,
            run: Run::default(),
        };
        let args = parsed(&["u", "--dry-run", "--cell", "a/1f"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let every = Some(Duration::from_secs(5));
        let command =
            Command::Collect { every, options, mode: Mode::MarkOnly, cell, run: Run::default() };
        let args = parsed(&["u", "--every", "5", "--cell", "a/1f", "--mark-only"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let command = Command::Branches { held: Some("hosts".into()), cell };
        let args = parsed(&["u", "--branches", "--cell", "a/1f", "--held", "hosts"]);
        assert_eq!(args, Ok(Args { url: url.clone(), command }));
        let command = Command::Branches { held: None, cell };
        assert_eq!(parsed(&["u", "--branches", "--cell", "a/1f"]), Ok(Args { url, command }));
    }

    /// TASK-395.4, TASK-395.5: a sweep says what its passes did together, the objects and bytes,
    /// over how many cells, how many it left to other passes, and how many failed, and the requests
    /// of each kind it made; then how long it took, and its slowest passes. A mark-only sweep says
    /// what it marked.
    #[test]
    fn sweeps_say_what_their_passes_did() {
        let report = Report {
            deleted: 3,
            compacted: 2,
            retired: 1,
            deleted_bytes: 3000,
            compacted_bytes: 200,
            skipped: 4,
        };
        let mut requests = Requests::default();
        requests.0[Request::Get as usize] = 40;
        requests.0[Request::ListDelimited as usize] = 9;
        requests.0[Request::DeleteBatch as usize] = 2;
        let cell = |id| Cell { tenant: 0xa, id };
        let swept = Swept {
            visited: 7,
            held: 2,
            swept: 3,
            failed: 1,
            report,
            requests,
            slowest: vec![
                (cell(0x1f), Duration::from_millis(1250)),
                (cell(2), Duration::from_millis(5)),
            ],
            took: Duration::from_millis(4321),
            ..Swept::default()
        };
        assert_eq!(
            swept_lines(Mode::Delete, &swept),
            (
                "fxdax-gc: deleted 3 (3000 bytes), compacted 2 (200 bytes), retired 1 references, \
                 over 7 cells (4 unchanged), 2 held by other passes, 3 swept by them since, 1 \
                 failed; requests: get 40, list-delimited 9, delete-batch 2"
                    .to_owned(),
                "fxdax-gc: took 4.321 s; slowest: a/1f 1.250 s, a/2 0.005 s".to_owned()
            )
        );
        let marking = Swept { marked: 5, marked_bytes: 500, resurrected: 1, ..Swept::default() };
        assert_eq!(
            swept_lines(Mode::MarkOnly, &marking),
            (
                "fxdax-gc: marked 5 (500 bytes), compacted 0 (0 bytes), resurrected 1, over 0 \
                 cells (0 unchanged), 0 held by other passes, 0 swept by them since, 0 failed; \
                 requests: none"
                    .to_owned(),
                "fxdax-gc: took 0.000 s; slowest: none".to_owned()
            )
        );
    }

    /// TASK-395.5: a pass says what it did, in how long and by what requests, if it deleted,
    /// compacted or retired something, or failed; nothing if it did nothing, or left its cell to
    /// another.
    #[test]
    fn passes_say_what_they_did_if_anything() {
        let mut requests = Requests::default();
        requests.0[Request::List as usize] = 2;
        let passed = |outcome| Ran {
            cell: Cell { tenant: 1, id: 2 },
            outcome,
            requests,
            took: Duration::from_millis(12),
        };
        let report = Report { deleted: 2, deleted_bytes: 8192, ..Report::default() };
        assert_eq!(
            passed_line(&passed(Outcome::Collected(report))).as_deref(),
            Some(
                "fxdax-gc: cell 1/2: deleted 2 (8192 bytes), compacted 0 (0 bytes), retired 0 \
                 references, in 0.012 s; requests: list 2"
            )
        );
        let failed = Outcome::Failed(extent_remote::Error::Corrupt("x".into()));
        assert_eq!(
            passed_line(&passed(failed)).as_deref(),
            Some("fxdax-gc: collecting cell 1/2: corrupt: x, in 0.012 s; requests: list 2")
        );
        let retiring = Report { retired: 1, skipped: 1, ..Report::default() };
        assert_eq!(
            passed_line(&passed(Outcome::Collected(retiring))).as_deref(),
            Some(
                "fxdax-gc: cell 1/2, unchanged: deleted 0 (0 bytes), compacted 0 (0 bytes), \
                 retired 1 references, in 0.012 s; requests: list 2"
            )
        );
        assert_eq!(passed_line(&passed(Outcome::Collected(Report::default()))), None);
        let skipped = Report { skipped: 1, ..Report::default() };
        assert_eq!(passed_line(&passed(Outcome::Collected(skipped))), None);
        assert_eq!(passed_line(&passed(Outcome::Left(extent_remote::Left::Held))), None);
    }

    /// TASK-395.5: `--metrics` writes the last sweep's counts in Prometheus' text format: its cells
    /// by what came of each, what its passes did, its requests by kind, how long its passes took,
    /// its slowest, and the process's peak memory.
    #[test]
    fn metrics_say_what_the_last_sweep_did() {
        let mut requests = Requests::default();
        requests.0[Request::Get as usize] = 40;
        let swept = Swept {
            visited: 3,
            held: 1,
            failed: 1,
            report: Report { deleted: 4, deleted_bytes: 4096, ..Report::default() },
            requests,
            slowest: vec![(Cell { tenant: 1, id: 2 }, Duration::from_millis(250))],
            took: Duration::from_millis(1500),
            ..Swept::default()
        };
        let mut passes = Histogram::default();
        for (millis, outcome) in [
            (250, Outcome::Collected(Report::default())),
            (5, Outcome::Collected(Report::default())),
            (20_000, Outcome::Failed(extent_remote::Error::Corrupt("x".into()))),
            (1, Outcome::Left(extent_remote::Left::Held)),
        ] {
            let cell = Cell { tenant: 1, id: 2 };
            let took = Duration::from_millis(millis);
            passes.observe(&Ran { cell, outcome, requests: Requests::default(), took });
        }
        let now = SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_250);
        let text = metrics(2, &swept, &passes, 64 << 20, now);
        let expected = "\
# HELP fxdax_gc_sweeps_total Sweeps the process ran.
# TYPE fxdax_gc_sweeps_total counter
fxdax_gc_sweeps_total 2
# HELP fxdax_gc_sweep_end_timestamp_seconds When its last sweep ended.
# TYPE fxdax_gc_sweep_end_timestamp_seconds gauge
fxdax_gc_sweep_end_timestamp_seconds 1700000000.250
# HELP fxdax_gc_sweep_seconds How long its last sweep took.
# TYPE fxdax_gc_sweep_seconds gauge
fxdax_gc_sweep_seconds 1.500000
# HELP fxdax_gc_sweep_cells The cells of its last sweep, by what came of each.
# TYPE fxdax_gc_sweep_cells gauge
fxdax_gc_sweep_cells{outcome=\"passed\"} 2
fxdax_gc_sweep_cells{outcome=\"skipped\"} 0
fxdax_gc_sweep_cells{outcome=\"failed\"} 1
fxdax_gc_sweep_cells{outcome=\"held\"} 1
fxdax_gc_sweep_cells{outcome=\"swept\"} 0
# HELP fxdax_gc_sweep_objects What its last sweep's passes did, in objects.
# TYPE fxdax_gc_sweep_objects gauge
fxdax_gc_sweep_objects{action=\"deleted\"} 4
fxdax_gc_sweep_objects{action=\"compacted\"} 0
fxdax_gc_sweep_objects{action=\"marked\"} 0
# HELP fxdax_gc_sweep_bytes What its last sweep's passes did, in bytes, as listed.
# TYPE fxdax_gc_sweep_bytes gauge
fxdax_gc_sweep_bytes{action=\"deleted\"} 4096
fxdax_gc_sweep_bytes{action=\"compacted\"} 0
fxdax_gc_sweep_bytes{action=\"marked\"} 0
# HELP fxdax_gc_sweep_retired_references The references its last sweep's passes retired.
# TYPE fxdax_gc_sweep_retired_references gauge
fxdax_gc_sweep_retired_references 0
# HELP fxdax_gc_sweep_resurrected_objects What its last sweep's mark-only passes found kept that the marks before named.
# TYPE fxdax_gc_sweep_resurrected_objects gauge
fxdax_gc_sweep_resurrected_objects 0
# HELP fxdax_gc_sweep_requests The requests its last sweep made, by kind.
# TYPE fxdax_gc_sweep_requests gauge
fxdax_gc_sweep_requests{request=\"get\"} 40
fxdax_gc_sweep_requests{request=\"get-range\"} 0
fxdax_gc_sweep_requests{request=\"head\"} 0
fxdax_gc_sweep_requests{request=\"list\"} 0
fxdax_gc_sweep_requests{request=\"create\"} 0
fxdax_gc_sweep_requests{request=\"update\"} 0
fxdax_gc_sweep_requests{request=\"overwrite\"} 0
fxdax_gc_sweep_requests{request=\"delete\"} 0
fxdax_gc_sweep_requests{request=\"list-delimited\"} 0
fxdax_gc_sweep_requests{request=\"delete-batch\"} 0
# HELP fxdax_gc_pass_seconds How long its last sweep's passes took.
# TYPE fxdax_gc_pass_seconds histogram
fxdax_gc_pass_seconds_bucket{le=\"0.001\"} 0
fxdax_gc_pass_seconds_bucket{le=\"0.01\"} 1
fxdax_gc_pass_seconds_bucket{le=\"0.1\"} 1
fxdax_gc_pass_seconds_bucket{le=\"1\"} 2
fxdax_gc_pass_seconds_bucket{le=\"10\"} 2
fxdax_gc_pass_seconds_bucket{le=\"60\"} 3
fxdax_gc_pass_seconds_bucket{le=\"600\"} 3
fxdax_gc_pass_seconds_bucket{le=\"+Inf\"} 3
fxdax_gc_pass_seconds_sum 20.255000
fxdax_gc_pass_seconds_count 3
# HELP fxdax_gc_slowest_pass_seconds Its last sweep's slowest passes, by cell.
# TYPE fxdax_gc_slowest_pass_seconds gauge
fxdax_gc_slowest_pass_seconds{cell=\"1/2\"} 0.250000
# HELP fxdax_gc_peak_resident_bytes The most memory the process held so far.
# TYPE fxdax_gc_peak_resident_bytes gauge
fxdax_gc_peak_resident_bytes 67108864
";
        assert_eq!(text, expected);
    }

    #[test]
    fn other_command_lines_get_the_usage() {
        let lines: [&[&str]; 33] = [
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
            &["u", "--usage", "--tenant"],
            &["u", "--usage", "--tenant", "x"],
            &["u", "--usage", "--tenant", "1/2"],
            &["u", "--usage", "--tenant", "10000000000000000"],
            &["u", "--tenant", "1", "--usage"],
            &["u", "--at-once", "0"],
            &["u", "--at-once", "many"],
            &["u", "--lease"],
            &["u", "--metrics"],
            &["u", "--metrics", "a", "--metrics", "b"],
            &["u", "--dry-run", "--at-once", "2"],
            &["u", "--dry-run", "--metrics", "a"],
        ];
        for args in lines {
            assert_eq!(parsed(args), Err(usage()), "{args:?}");
        }
    }
}
