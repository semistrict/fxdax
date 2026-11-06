// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: one snapshot of what the daemon, its VM processes and their guests have counted, each
//! with the commit it was built of (TASK-111.4): the control API's `GetSnapshot`, which
//! `fxdaxctl stats --json` prints.
//!
//! The daemon asks every VM process that opened a store for its stats
//! ([`super::wire::STATS_ASK`]); each says its stage counts and answers with its guest's stats,
//! which its device asks the guest's driver for (backlog doc-4's `STATS`), or with none, if its
//! guest takes no asks ([`super::wire::STATS`]). Once every one answered, or the wait is over, the daemon reads its own
//! counters: the snapshot is as of then, each VM's within the wait before it, which says how long
//! its answer took. Separate reads of `/metrics`, the guests' `/proc/fs/fxfs` and the VM processes'
//! threads, which the sandbox team diffed, were taken at moments apart, and said no build.

use super::codec::{StageCounts, Told};
use super::wire::AsyncSender;
use crate::Errno;
use crate::dax::{Gauges, Live};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};

/// How long a snapshot waits for the VM processes' answers unless told: a guest's answer takes a
/// configuration change and a request, under a millisecond on an idle host.
pub(crate) const WAIT: Duration = Duration::from_secs(2);

/// The most a snapshot waits for them, whatever it is told.
pub(crate) const MOST_WAIT: Duration = Duration::from_secs(60);

/// What the daemon and its VM processes counted, as of [`Snapshot::taken`].
#[derive(Debug)]
pub(crate) struct Snapshot {
    /// When the daemon read its own counters, its VM processes having answered.
    pub taken: SystemTime,
    /// The commit the daemon was built of.
    pub build: &'static str,
    /// Its counters, which only grow, by the names its metrics give them (`Stats::named`, dashes
    /// as underscores).
    pub counters: Vec<(String, u64)>,
    /// Its gauges, by the names its metrics give them, without `fxdax_extentd_`.
    pub gauges: Vec<(&'static str, u64)>,
    /// Its VM processes, those that opened a store, in the order they connected.
    pub vms: Vec<Vm>,
}

/// A VM process, and its guest, as it answered, or as the daemon has it if it didn't.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Vm {
    /// Its connection's number, which the daemon gives each in turn: what tells a store's writer
    /// and followers apart, of one branch.
    pub connection: u64,
    /// Its store's branch.
    pub branch: u64,
    /// Its store's directory, as the daemon has it: without symbolic links.
    pub path: PathBuf,
    /// Whether it follows its store (backlog doc-8), or writes it.
    pub follower: bool,
    /// The commit it said it was built of, if it did ([`super::wire::BUILD`]).
    pub build: Option<String>,
    /// Its window's second stage, as it said ([`super::wire::STAGE_COUNTS`]).
    pub stage: StageCounts,
    /// The runs of pages its second stage keeps installed at most, if it said
    /// ([`super::wire::WINDOW`]).
    pub budget: Option<u64>,
    /// Its window's mappings, the daemon's, if it has a window.
    pub window: Option<Live>,
    /// How long after the ask it answered, if it did.
    pub answered: Option<Duration>,
    /// Its guest's stats, if a guest answered.
    pub guest: Option<GuestStats>,
}

impl Vm {
    /// Its counters, which only grow, by name: its window's stage operations, as
    /// `Stats::stage_ops` and the others count them of every VM process.
    pub fn counters(&self) -> [(&'static str, u64); 4] {
        let [ops, ns, overwrites, leave_outs] = self.stage.costs;
        [
            ("stage_ops", ops),
            ("stage_ns", ns),
            ("stage_overwrites", overwrites),
            ("stage_leave_outs", leave_outs),
        ]
    }

    /// Its gauges, by name: the runs of pages its second stage has installed in its memory, each a
    /// mapping the host's `vm.max_map_count` limits, and the most it keeps (TASK-111.5).
    pub fn gauges(&self) -> Vec<(&'static str, u64)> {
        let mut gauges = vec![("host_mappings", self.stage.installed)];
        gauges.extend(self.budget.map(|budget| ("host_mapping_budget", budget)));
        gauges
    }

    /// Its window's mappings, by name, if it has a window: as the daemon's metrics name all
    /// windows' together, without `window_`.
    pub fn window(&self) -> Option<[(&'static str, u64); 4]> {
        self.window.map(|live| {
            [
                ("mappings", live.mappings),
                ("mapped_bytes", live.mapped_bytes),
                ("pages", live.pages),
                ("private_pages", live.private_pages),
            ]
        })
    }
}

/// What a guest's Fxfs counted, as its driver answered ([`super::wire::STATS`]): what its
/// `/proc/fs/fxfs/TAG` says, and its build.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GuestStats {
    /// The commit it was built of (its `commit` line), if it said.
    pub build: Option<String>,
    /// Its counters and gauges, by name.
    pub counters: BTreeMap<String, u64>,
    /// Its histograms, by name, without `_hist`: each bucket's count, in order.
    pub histograms: BTreeMap<String, Vec<u64>>,
}

impl GuestStats {
    /// What `text` says, lines of a name and a value: `commit` and a build; or a number; or a
    /// histogram's counts between commas, its name ending in `_hist`. A line that is none of those
    /// is logged and left out: the guest and the daemon disagree on what it says.
    pub fn parse(text: &str) -> Self {
        let mut stats = Self::default();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let parsed = line.split_once(' ').and_then(|(name, value)| {
                if name == "commit" {
                    stats.build = Some(value.to_owned());
                    return Some(());
                }
                if let Some(histogram) = name.strip_suffix("_hist") {
                    let counts = value.split(',').map(str::parse).collect::<Result<_, _>>().ok()?;
                    stats.histograms.insert(histogram.to_owned(), counts);
                    return Some(());
                }
                stats.counters.insert(name.to_owned(), value.parse().ok()?);
                Some(())
            });
            if parsed.is_none() {
                log::warn!("extentd: a guest's stats line not understood, left out: {line:?}");
            }
        }
        stats
    }
}

/// The asks made so far, each a number of its own ([`super::wire::STATS_ASK`]).
static ASKS: AtomicU64 = AtomicU64::new(0);

/// What the daemon and its VM processes counted, as of now: the VM processes asked first, and
/// waited for, for `wait` at most ([`MOST_WAIT`] at most).
pub(crate) async fn take(wait: Duration) -> Snapshot {
    let ask = ASKS.fetch_add(1, Ordering::Relaxed) + 1;
    let deadline = tokio::time::Instant::now() + wait.min(MOST_WAIT);
    let asked: Vec<_> = (reports().into_iter())
        .filter_map(|report| {
            let answer = report.ask(ask, Instant::now())?;
            Some((report, answer))
        })
        .collect();
    let mut vms = Vec::with_capacity(asked.len());
    for (report, answer) in asked {
        let vm = match tokio::time::timeout_at(deadline, answer).await {
            Ok(Ok(vm)) => Some(vm),
            // Not answered in time, or its connection went without answering: as the daemon has
            // it, the ask forgotten.
            Ok(Err(_)) | Err(_) => {
                report.forget(ask);
                report.vm()
            }
        };
        vms.extend(vm);
    }
    of_daemon(vms)
}

/// The daemon's counters and gauges, as of now, and `vms`.
fn of_daemon(vms: Vec<Vm>) -> Snapshot {
    let counters = (crate::stats::stats().named().into_iter())
        .map(|(name, value)| (name.replace('-', "_"), value))
        .chain([("window_open_maps_refused".to_owned(), crate::extents::open_maps_refused())])
        .collect();
    let gauges = super::metrics::gauges().into_iter().map(|(name, _, value)| (name, value));
    Snapshot {
        taken: SystemTime::now(),
        build: crate::BUILD_COMMIT,
        counters,
        gauges: gauges.collect(),
        vms,
    }
}

/// The VM processes connected that opened a store, as the daemon has them now, in the order they
/// connected (`metrics`, TASK-111.5).
pub(super) fn vms() -> Vec<Vm> {
    reports().iter().filter_map(|report| report.vm()).collect()
}

/// The builds the VM processes connected said, by their stores' branches, each with how many said
/// it (`metrics`, TASK-111.7): a writer and its followers are of one branch. In order of branch and
/// commit.
pub(super) fn vm_builds() -> Vec<((u64, String), usize)> {
    let mut counted = BTreeMap::new();
    for vm in vms() {
        if let Some(build) = vm.build {
            *counted.entry((vm.branch, build)).or_insert(0) += 1;
        }
    }
    counted.into_iter().collect()
}

/// What the daemon reports of a connection's VM process, which the connection keeps up to date
/// (TASK-111.4, TASK-111.5): what the daemon's metrics and snapshots hold, rather than the
/// connection, the last to let go of which closes its store.
pub(super) struct Report {
    /// The connection's number, the count of connections made as it was.
    id: u64,
    /// Where what the connection tells its VM process goes, as long as the connection is.
    sender: Weak<AsyncSender>,
    /// Its store, once it opened one.
    opened: OnceLock<Opened>,
    /// The build its VM process said ([`super::wire::BUILD`]), once.
    build: OnceLock<String>,
    /// What its VM process last said its stage operations cost it, and its second stage has
    /// installed ([`super::wire::STAGE_COUNTS`]).
    costs: [AtomicU64; 4],
    installed: AtomicU64,
    /// Its window's gauges, and the runs of pages its VM process said its second stage keeps
    /// installed at most, 0 if it didn't ([`super::wire::WINDOW`]), once it has one.
    window: OnceLock<(Arc<Gauges>, u64)>,
    /// The asks for its VM process's stats it has yet to answer ([`super::wire::STATS_ASK`]).
    asked: Mutex<Vec<Asked>>,
}

/// The store a connection opened.
struct Opened {
    /// Its directory, without symbolic links.
    path: PathBuf,
    branch: u64,
    /// Whether the connection follows it (backlog doc-8), or writes it.
    follower: bool,
}

/// An ask a VM process has yet to answer ([`Report::ask`]): its number, when it was made, and
/// where its answer goes.
struct Asked {
    ask: u64,
    at: Instant,
    answered: tokio::sync::oneshot::Sender<Vm>,
}

/// The connections' reports, as they are made, each as long as its connection, or a snapshot,
/// holds it.
static REPORTS: Mutex<Vec<Weak<Report>>> = Mutex::new(Vec::new());

/// The connections' numbers, given in turn ([`Report::id`]).
static IDS: AtomicU64 = AtomicU64::new(0);

/// How many asks each connection's VM process has yet to answer, in the order they were made.
#[cfg(test)]
pub(super) fn asks_waiting() -> Vec<usize> {
    reports().iter().map(|report| report.asked.lock().unwrap().len()).collect()
}

/// The reports there are now, in the order their connections were made.
fn reports() -> Vec<Arc<Report>> {
    let mut reports = REPORTS.lock().unwrap();
    reports.retain(|report| report.strong_count() > 0);
    reports.iter().filter_map(Weak::upgrade).collect()
}

impl Report {
    /// The report of a new connection, which tells its VM process through `sender`, numbered next.
    pub fn new(sender: &Arc<AsyncSender>) -> Arc<Self> {
        let report = Arc::new(Self {
            id: IDS.fetch_add(1, Ordering::Relaxed) + 1,
            sender: Arc::downgrade(sender),
            opened: OnceLock::new(),
            build: OnceLock::new(),
            costs: Default::default(),
            installed: AtomicU64::new(0),
            window: OnceLock::new(),
            asked: Mutex::new(Vec::new()),
        });
        REPORTS.lock().unwrap().push(Arc::downgrade(&report));
        report
    }

    /// The connection opened the store of `branch` at `path`, which it follows if `follower`, once
    /// for good.
    pub fn opened(&self, path: PathBuf, branch: u64, follower: bool) {
        let opened = self.opened.set(Opened { path, branch, follower });
        assert!(opened.is_ok(), "a connection opened a second store");
    }

    /// The connection made its window, whose gauges are `gauges`, its VM process keeping `budget`
    /// runs of pages installed at most, 0 if it didn't say.
    pub fn windowed(&self, gauges: Arc<Gauges>, budget: u64) {
        let windowed = self.window.set((gauges, budget));
        assert!(windowed.is_ok(), "a connection made a second window");
    }

    /// Records the build its VM process says, `commit` ([`super::wire::BUILD`], TASK-111.7):
    /// `EEXIST` said again.
    pub fn said_build(&self, commit: &str) -> Result<(), Errno> {
        self.build.set(commit.to_owned()).map_err(|_| Errno::EEXIST)
    }

    /// Counts what its VM process says its stage operations have cost it so far, `counts` as
    /// [`super::wire::STAGE_COUNTS`] has them, as the daemon's (`Stats::stage_ops`, TASK-91.16):
    /// what it didn't say before. A count only grows: one said again, or smaller, adds nothing.
    /// And keeps the runs of pages it says it has installed, as said last, in order, on the one
    /// task receiving its messages (TASK-111.5).
    pub fn stage_counted(&self, counts: StageCounts) {
        use crate::stats::Counter;
        let [ops, ns, overwrites, leave_outs] = counts.costs;
        let said = [
            (ops, Counter::StageOps),
            (ns, Counter::StageNs),
            (overwrites, Counter::StageOverwrites),
            (leave_outs, Counter::StageLeaveOuts),
        ];
        for (before, (count, counter)) in self.costs.iter().zip(said) {
            let added = count.saturating_sub(before.fetch_max(count, Ordering::Relaxed));
            crate::stats::add(counter, added);
        }
        self.installed.store(counts.installed, Ordering::Relaxed);
    }

    /// Its VM process, as the daemon has it now, if it opened a store: not yet answered.
    fn vm(&self) -> Option<Vm> {
        let Opened { path, branch, follower } = self.opened.get()?;
        let costs = self.costs.each_ref().map(|count| count.load(Ordering::Relaxed));
        let installed = self.installed.load(Ordering::Relaxed);
        let window = self.window.get();
        Some(Vm {
            connection: self.id,
            branch: *branch,
            path: path.clone(),
            follower: *follower,
            build: self.build.get().cloned(),
            stage: StageCounts { costs, installed },
            budget: window.and_then(|(_, budget)| (*budget != 0).then_some(*budget)),
            window: window.map(|(gauges, _)| gauges.live()),
            answered: None,
            guest: None,
        })
    }

    /// Asks its VM process for its stats, as ask `ask`, made at `at` ([`super::wire::STATS_ASK`]):
    /// what its answer will say, unless it opened no store, or can't be asked.
    fn ask(&self, ask: u64, at: Instant) -> Option<tokio::sync::oneshot::Receiver<Vm>> {
        self.opened.get()?;
        let sender = self.sender.upgrade()?;
        let (answered, answer) = tokio::sync::oneshot::channel();
        self.asked.lock().unwrap().push(Asked { ask, at, answered });
        if let Err(err) = sender.send(Told::Stats { ask }.encode(), Vec::new(), &[]) {
            log::warn!("extentd: asking a VM process for its stats: {err}");
            self.forget(ask);
            return None;
        }
        Some(answer)
    }

    /// Forgets ask `ask`, which its asker no longer waits for an answer to: a VM process that never
    /// answers, as one whose guest is paused, which its device asks all the same, holds none of
    /// the asks given up on, which grew one a snapshot for as long as it lived.
    fn forget(&self, ask: u64) {
        self.asked.lock().unwrap().retain(|asked| asked.ask != ask);
    }

    /// Its VM process answered ask `ask` ([`super::wire::STATS`]), and those before it, with its
    /// guest's stats, `guest`, if it has a guest that answered: as of now, its stage counts said
    /// just before. `EBADF` before it opened a store.
    pub fn answered(&self, ask: u64, guest: Option<&str>) -> Result<(), Errno> {
        let vm = self.vm().ok_or(Errno::EBADF)?;
        let answered: Vec<Asked> = {
            let mut asked = self.asked.lock().unwrap();
            let (answered, waiting) = asked.drain(..).partition(|asked| asked.ask <= ask);
            *asked = waiting;
            answered
        };
        let guest = guest.map(GuestStats::parse);
        for Asked { at, answered, .. } in answered {
            let vm = Vm { answered: Some(at.elapsed()), guest: guest.clone(), ..vm.clone() };
            // Whoever asked may have stopped waiting.
            let _ = answered.send(vm);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TASK-111.4: a guest's stats, as its Fxfs writes them (`/proc/fs/fxfs/TAG`): counters,
    /// histograms and the build, each line read; a line of neither, left out, the rest kept.
    #[test]
    fn guests_stats_are_their_lines() {
        let text = "device_reads 12\ndevice_reads_ns 3400\ndevice_reads_hist 0,1,2,0\n\
                    commit 0123456789abcdef0123456789abcdef01234567\nwindow_full 0\n\
                    nonsense\nnegative -1\nhalf_hist 1,x\n\n";
        let stats = GuestStats::parse(text);
        assert_eq!(stats.build.as_deref(), Some("0123456789abcdef0123456789abcdef01234567"));
        let counters: Vec<(&str, u64)> =
            stats.counters.iter().map(|(name, value)| (name.as_str(), *value)).collect();
        assert_eq!(counters, [("device_reads", 12), ("device_reads_ns", 3400), ("window_full", 0)]);
        let histograms: Vec<(&str, &[u64])> =
            stats.histograms.iter().map(|(name, counts)| (name.as_str(), &counts[..])).collect();
        assert_eq!(histograms, [("device_reads", &[0, 1, 2, 0][..])]);
    }

    /// The text of no guest's stats says nothing.
    #[test]
    fn no_lines_say_nothing() {
        assert_eq!(GuestStats::parse(""), GuestStats::default());
    }
}
