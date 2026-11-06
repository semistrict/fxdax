// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what fxfs-guest-soak's host and guests share (TASK-67): a round's plan, a pure function
//! of the run's seed and the round's number; the bytes its writers write, which the guest that
//! checks them makes again from their names; and the arithmetic of its verdicts, what of the
//! writers' acknowledgements a round may lose (doc-12) and when a quantity measured round after
//! round has grown. No system calls: all of it is tested on any host.

use crate::script::Rng;
use std::collections::BTreeSet;

/// The VMs that run at once, each on a store of its own forked from the base.
pub const SLOTS: usize = 5;

/// The writers of a writer's VM, each with files of its own in its run's directory.
pub const WRITERS: u8 = 2;

/// The runs' directories a writer's VM keeps, the one it makes but for: it drops the older.
pub const KEPT_RUNS: usize = 3;

/// The files the base holds, which every fork reads.
pub const BASE_FILES: u32 = 32;

/// What a VM does through its round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Load {
    /// Files created and fsynced one after another, by [`WRITERS`] writers, and synced whole
    /// (syncfs) every so often: what it was told of, the host holds it to.
    Writer,
    /// fsx-style scripts of operations against a model ([`crate::script`]), one after another.
    Fsx,
    /// pgbench on the base's database.
    Pgbench,
}

impl Load {
    pub const ALL: [Load; 3] = [Load::Writer, Load::Fsx, Load::Pgbench];

    pub fn name(self) -> &'static str {
        match self {
            Load::Writer => "writer",
            Load::Fsx => "fsx",
            Load::Pgbench => "pgbench",
        }
    }

    pub fn named(name: &str) -> Option<Load> {
        Self::ALL.into_iter().find(|load| load.name() == name)
    }
}

/// The faults a run's rounds may have, each of which FXDAX_SOAK_FAULTS may leave out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Fault {
    /// A VM's process killed (SIGKILL) mid-write.
    VmCrash,
    /// The extent daemon killed (SIGKILL) and started again, its VMs booted again.
    DaemonKill,
    /// An upload failing, which stops its store's uploader until the store is attached again.
    UploadFail,
    /// The bucket answering late: uploads lag behind the commits, which a lost host loses.
    Latency,
    /// The host lost: its stores and cache taken away, every branch taken again from the bucket
    /// by a daemon of a later placement.
    HostLoss,
    /// The host's cache of the bucket lost between two daemons.
    CacheLoss,
    /// A VM's store deleted, its branch with it, and forked again from the base.
    Refork,
}

impl Fault {
    pub const ALL: [Fault; 7] = [
        Fault::VmCrash,
        Fault::DaemonKill,
        Fault::UploadFail,
        Fault::Latency,
        Fault::HostLoss,
        Fault::CacheLoss,
        Fault::Refork,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Fault::VmCrash => "vm-crash",
            Fault::DaemonKill => "daemon-kill",
            Fault::UploadFail => "upload-fail",
            Fault::Latency => "latency",
            Fault::HostLoss => "host-loss",
            Fault::CacheLoss => "cache-loss",
            Fault::Refork => "refork",
        }
    }

    /// The faults a list of their names, separated by commas, names: all of them for `all`, none
    /// for `none`.
    pub fn parse(names: &str) -> Result<BTreeSet<Fault>, String> {
        match names.trim() {
            "all" => return Ok(Self::ALL.into_iter().collect()),
            "none" | "" => return Ok(BTreeSet::new()),
            _ => {}
        }
        (names.split(',').map(str::trim))
            .map(|name| {
                Self::ALL
                    .into_iter()
                    .find(|fault| fault.name() == name)
                    .ok_or_else(|| format!("no fault {name:?}"))
            })
            .collect()
    }
}

/// What a VM of the round does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotPlan {
    pub load: Load,
    /// Whether its store is deleted as the round begins, and forked from the base again.
    pub refork: bool,
    /// When its VM's process is killed, in seconds from the round's start.
    pub crash_at: Option<u64>,
}

/// What a round does, which the run's seed and the round's number say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub round: u64,
    /// The round's own seed, which its VMs' are drawn from.
    pub seed: u64,
    /// How long the VMs' loads run.
    pub load_secs: u64,
    pub slots: Vec<SlotPlan>,
    /// The bucket's latency, in milliseconds, for the round's daemons but the checking one.
    pub latency_ms: u64,
    /// The upload after which one fails, its store's uploader stopping: FAILPOINTS'
    /// `writer::upload` ([`Plan::failpoints`]).
    pub upload_fails_after: Option<u64>,
    /// When the daemon is killed, and started again.
    pub daemon_killed_at: Option<u64>,
    /// When the host is lost, which ends the load.
    pub host_lost_at: Option<u64>,
    /// Whether the cache is lost once the load ends.
    pub cache_lost: bool,
}

/// The longest and shortest loads.
const LOAD_SECS: (u64, u64) = (120, 240);

/// A bucket's latencies a round may have, in milliseconds.
const LATENCIES: [u64; 5] = [0, 0, 25, 250, 1000];

impl Plan {
    /// Round `round` (from 1) of the run of `seed`, with the faults `faults`, and loads of
    /// `load_secs` seconds if given. The first round has none: it makes the base, and the first
    /// measures.
    pub fn new(seed: u64, round: u64, faults: &BTreeSet<Fault>, load_secs: Option<u64>) -> Self {
        let round_seed = mix(seed, round);
        let mut rng = Rng::new(round_seed);
        let mut chance = |percent: u64| rng.next(100) < percent;
        let first = round == 1;
        let on = |fault: Fault| !first && faults.contains(&fault);
        let mut rng = Rng::new(mix(round_seed, 1));
        let load_secs =
            load_secs.unwrap_or_else(|| LOAD_SECS.0 + rng.next(LOAD_SECS.1 - LOAD_SECS.0 + 1));
        // Every load among the first three VMs, in an order of the round's; the others' drawn.
        let mut loads = Load::ALL.to_vec();
        for i in (1..loads.len()).rev() {
            loads.swap(i, rng.next(i as u64 + 1) as usize);
        }
        while loads.len() < SLOTS {
            loads.push(match rng.next(5) {
                0 | 1 => Load::Writer,
                2 | 3 => Load::Fsx,
                _ => Load::Pgbench,
            });
        }
        let within = |rng: &mut Rng| 15 + rng.next(load_secs.saturating_sub(30).max(1));
        let slots = (loads.into_iter())
            .map(|load| SlotPlan {
                load,
                refork: on(Fault::Refork) && rng.next(100) < 10,
                crash_at: (on(Fault::VmCrash) && rng.next(100) < 30).then(|| within(&mut rng)),
            })
            .collect();
        let latency_ms = match on(Fault::Latency) {
            true => LATENCIES[rng.next(LATENCIES.len() as u64) as usize],
            false => 0,
        };
        let upload_fails_after = (on(Fault::UploadFail) && chance(20)).then(|| 5 + rng.next(200));
        let daemon_killed_at = (on(Fault::DaemonKill) && chance(35)).then(|| within(&mut rng));
        let host_lost_at = (on(Fault::HostLoss) && chance(20)).then(|| within(&mut rng));
        let cache_lost = on(Fault::CacheLoss) && chance(15);
        Self {
            round,
            seed: round_seed,
            load_secs,
            slots,
            latency_ms,
            upload_fails_after,
            daemon_killed_at,
            host_lost_at,
            cache_lost,
        }
    }

    /// The `fail` crate's FAILPOINTS for the round's daemons but the checking one: the upload
    /// after `upload_fails_after` fails, once.
    pub fn failpoints(&self) -> Option<String> {
        self.upload_fails_after.map(|after| format!("writer::upload={after}*off->1*return"))
    }

    /// The seed of slot `slot`'s VM's boot `boot` of the round, which its load is drawn from.
    pub fn vm_seed(&self, slot: usize, boot: u32) -> u64 {
        mix(mix(self.seed, slot as u64 + 2), boot as u64)
    }

    /// What it does, on a line.
    pub fn describe(&self) -> String {
        let slots: Vec<String> = (self.slots.iter().enumerate())
            .map(|(slot, plan)| {
                let mut said = format!("{slot}:{}", plan.load.name());
                if plan.refork {
                    said.push_str(" reforked");
                }
                if let Some(at) = plan.crash_at {
                    said.push_str(&format!(" crashed@{at}s"));
                }
                said
            })
            .collect();
        let mut faults = Vec::new();
        if self.latency_ms > 0 {
            faults.push(format!("latency {} ms", self.latency_ms));
        }
        if let Some(points) = self.failpoints() {
            faults.push(points);
        }
        if let Some(at) = self.daemon_killed_at {
            faults.push(format!("daemon killed@{at}s"));
        }
        if let Some(at) = self.host_lost_at {
            faults.push(format!("host lost@{at}s"));
        }
        if self.cache_lost {
            faults.push("cache lost".to_owned());
        }
        format!(
            "round {} (seed {}): {} s; {}; {}",
            self.round,
            self.seed,
            self.load_secs,
            slots.join(", "),
            if faults.is_empty() { "no daemon faults".to_owned() } else { faults.join(", ") }
        )
    }
}

/// `a` and `b` mixed into a seed (SplitMix64's finalizer of their combination).
pub fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.wrapping_add(0x9E37_79B9_7F4A_7C15).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// FNV-1a of `bytes`.
pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The name of slot's run of boot `boot` of round `round`, of its store's generation `generation`
/// (how many times it was forked again): in the order they were made.
pub fn run_name(generation: u32, round: u64, boot: u32) -> String {
    format!("g{generation:03}r{round:04}b{boot}")
}

/// The key of writer `writer`'s file `index` of the run `run` of slot `slot`.
pub fn file_key(slot: usize, run: &str, writer: u8, index: u32) -> u64 {
    mix(mix(fnv(run.as_bytes()), slot as u64), ((writer as u64) << 32) | index as u64)
}

/// The key of the base's file `index`, of the run of `seed`.
pub fn base_key(seed: u64, index: u32) -> u64 {
    mix(mix(seed, 0xba5e), index as u64)
}

/// The bytes of the file of `key`: some pages and a part of one, at most 36 KiB, which no other
/// key's repeat.
pub fn file_bytes(key: u64) -> Vec<u8> {
    let len = 4096 * (key % 9) as usize + (key >> 8) as usize % 4096 + 1;
    let mut rng = Rng::new(key);
    let mut bytes = Vec::with_capacity(len + 8);
    while bytes.len() < len {
        bytes.extend_from_slice(&rng.next(u64::MAX).to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

/// `indices`, in order, as ranges: `0-41,43,45-50`, or `-` for none.
pub fn ranges(indices: &BTreeSet<u32>) -> String {
    let mut said: Vec<String> = Vec::new();
    let mut iter = indices.iter().copied().peekable();
    while let Some(first) = iter.next() {
        let mut last = first;
        while iter.peek() == Some(&(last + 1)) {
            last = iter.next().unwrap();
        }
        said.push(if first == last { first.to_string() } else { format!("{first}-{last}") });
    }
    if said.is_empty() { "-".to_owned() } else { said.join(",") }
}

/// The indices `ranges` says.
pub fn parse_ranges(text: &str) -> Result<BTreeSet<u32>, String> {
    if text == "-" {
        return Ok(BTreeSet::new());
    }
    let mut indices = BTreeSet::new();
    for part in text.split(',') {
        let number = |n: &str| n.parse::<u32>().map_err(|_| format!("{text:?}: {n:?}"));
        let (first, last) = match part.split_once('-') {
            Some((first, last)) => (number(first)?, number(last)?),
            None => (number(part)?, number(part)?),
        };
        if first > last {
            return Err(format!("{text:?}: {part:?} runs backwards"));
        }
        indices.extend(first..=last);
    }
    Ok(indices)
}

/// What a writer was told of its files: those up to `synced` fsynced, those up to `durable`
/// synced whole (syncfs), each once its call returned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Acks {
    pub synced: Option<u32>,
    pub durable: Option<u32>,
}

/// What a writer's run holds as a guest checking it found it: its files whole, and those there
/// but not whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub whole: BTreeSet<u32>,
    pub torn: BTreeSet<u32>,
}

/// The files a writer's run that was told `acks` keeps for good as `found` has it, the first of
/// them through the last, or why it may not have come to that (doc-12). Files are made one after
/// another, each made only once the last's fsync returned: what stays of them is a prefix of the
/// fsyncs, and of those, unless the daemon died or the host was lost since the run began
/// (`lossy`), all of them; and every file a syncfs was told of, in any case. A file the prefix
/// leaves out may be there, torn, as what wrote it back before its fsync committed left it; but
/// none after it, which were made after a commit lost.
pub fn kept(acks: Acks, found: &Found, lossy: bool) -> Result<Option<u32>, String> {
    let prefix = |from: u32| (from..).take_while(|index| found.whole.contains(index)).last();
    let kept = prefix(0);
    let Some(synced) = acks.synced else { return Ok(kept) };
    let short = kept.is_none_or(|kept| kept < synced);
    if short && !lossy {
        return Err(format!(
            "fsyncs of 0-{synced} returned, {} kept, and nothing lost the daemon's commits",
            kept.map_or("none".to_owned(), |kept| format!("0-{kept}"))
        ));
    }
    if let Some(durable) = acks.durable
        && kept.is_none_or(|kept| kept < durable)
    {
        return Err(format!(
            "syncfs returned for 0-{durable}, {} kept",
            kept.map_or("none".to_owned(), |kept| format!("0-{kept}"))
        ));
    }
    if short {
        let lost = kept.map_or(0, |kept| kept + 1);
        let after = (found.whole.iter().chain(&found.torn)).find(|&&index| index > lost);
        if let Some(after) = after {
            return Err(format!(
                "file {lost}'s fsync was lost, and file {after}, made after it returned, is there"
            ));
        }
    }
    Ok(kept)
}

/// Whether a quantity measured after each round grew past its bound: twice the most it was in
/// the rounds `warm` (from 1) and `slack` more. The rounds before them make what the soak runs
/// on; the measures after them stay near theirs, or something leaks.
#[derive(Clone, Copy, Debug)]
pub struct Bound {
    pub warm: (usize, usize),
    pub slack: u64,
}

impl Bound {
    /// Why the last of `values`, each round's from the first, is past the bound, if it is.
    pub fn grown(&self, what: &str, values: &[u64]) -> Option<String> {
        let (from, to) = self.warm;
        if values.len() <= to {
            return None;
        }
        let warm = values[from - 1..to].iter().copied().max().unwrap_or(0);
        let limit = warm.saturating_mul(2).saturating_add(self.slack);
        let last = *values.last()?;
        (last > limit).then(|| {
            format!(
                "{what} grew to {last} in round {}: more than twice rounds {from}-{to}'s most, \
                 {warm}, and {} more",
                values.len(),
                self.slack
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> BTreeSet<Fault> {
        Fault::ALL.into_iter().collect()
    }

    /// A round's plan is its seed's and number's alone, its first round's without faults.
    #[test]
    fn plans_are_their_seeds() {
        assert_eq!(Plan::new(7, 3, &all(), None), Plan::new(7, 3, &all(), None));
        assert_ne!(Plan::new(7, 3, &all(), None), Plan::new(7, 4, &all(), None));
        assert_ne!(Plan::new(7, 3, &all(), None), Plan::new(8, 3, &all(), None));
        let first = Plan::new(7, 1, &all(), Some(60));
        assert_eq!(first.load_secs, 60);
        assert!(first.slots.iter().all(|slot| !slot.refork && slot.crash_at.is_none()));
        assert_eq!(
            (first.latency_ms, first.upload_fails_after, first.daemon_killed_at),
            (0, None, None)
        );
        assert_eq!((first.host_lost_at, first.cache_lost), (None, false));
    }

    /// Every round runs every load, and over many, every fault comes, within the load's time;
    /// those left out never.
    #[test]
    fn rounds_run_every_load_and_fault() {
        let plans: Vec<Plan> = (2..400).map(|round| Plan::new(11, round, &all(), None)).collect();
        for plan in &plans {
            assert_eq!(plan.slots.len(), SLOTS);
            for load in Load::ALL {
                assert!(plan.slots.iter().any(|slot| slot.load == load), "{plan:?}");
            }
            assert!((LOAD_SECS.0..=LOAD_SECS.1).contains(&plan.load_secs), "{plan:?}");
            let times = (plan.slots.iter().filter_map(|slot| slot.crash_at))
                .chain(plan.daemon_killed_at)
                .chain(plan.host_lost_at);
            for at in times {
                assert!((15..plan.load_secs).contains(&at), "{plan:?}");
            }
        }
        let some = |test: &dyn Fn(&Plan) -> bool| plans.iter().filter(|plan| test(plan)).count();
        assert!(some(&|plan| plan.slots.iter().any(|slot| slot.crash_at.is_some())) > 100);
        assert!(some(&|plan| plan.slots.iter().any(|slot| slot.refork)) > 50);
        assert!(some(&|plan| plan.daemon_killed_at.is_some()) > 80);
        assert!(some(&|plan| plan.host_lost_at.is_some()) > 40);
        assert!(some(&|plan| plan.upload_fails_after.is_some()) > 40);
        assert!(some(&|plan| plan.latency_ms == 1000) > 40);
        assert!(some(&|plan| plan.cache_lost) > 30);
        let none: Vec<Plan> =
            (2..400).map(|round| Plan::new(11, round, &BTreeSet::new(), None)).collect();
        for plan in &none {
            assert!(plan.describe().contains("no daemon faults"), "{plan:?}");
            assert!(plan.slots.iter().all(|slot| !slot.refork && slot.crash_at.is_none()));
        }
        let only = Fault::parse("host-loss").unwrap();
        let lost: Vec<Plan> = (2..400).map(|round| Plan::new(11, round, &only, None)).collect();
        assert!(lost.iter().all(|plan| plan.daemon_killed_at.is_none() && plan.latency_ms == 0));
        assert!(lost.iter().any(|plan| plan.host_lost_at.is_some()));
    }

    #[test]
    fn faults_are_named() {
        assert_eq!(Fault::parse("all").unwrap(), all());
        assert_eq!(Fault::parse("none").unwrap(), BTreeSet::new());
        assert_eq!(
            Fault::parse("vm-crash, refork").unwrap(),
            [Fault::VmCrash, Fault::Refork].into_iter().collect()
        );
        assert_eq!(Fault::parse("vm-crash,nope"), Err("no fault \"nope\"".to_owned()));
        let plan = Plan { upload_fails_after: Some(17), ..Plan::new(1, 1, &all(), None) };
        assert_eq!(plan.failpoints().as_deref(), Some("writer::upload=17*off->1*return"));
    }

    /// Files' bytes are their keys', of every length up to 9 pages, and no two keys' alike.
    #[test]
    fn files_are_their_keys() {
        let keys: Vec<u64> = (0..200).map(|index| file_key(1, "g000r0002b0", 1, index)).collect();
        assert_eq!(file_bytes(keys[3]), file_bytes(keys[3]));
        let lens: BTreeSet<usize> = keys.iter().map(|&key| file_bytes(key).len() / 4096).collect();
        assert_eq!(lens, (0..9).collect());
        assert_ne!(file_bytes(keys[1])[..1], file_bytes(keys[2])[..1]);
        assert_ne!(file_key(1, "a", 0, 0), file_key(2, "a", 0, 0));
        assert_ne!(file_key(1, "a", 0, 0), file_key(1, "a", 1, 0));
        assert_ne!(base_key(1, 0), base_key(2, 0));
        assert!(run_name(1, 2, 0) < run_name(1, 2, 1) && run_name(1, 9, 3) < run_name(1, 10, 0));
        assert!(run_name(1, 999, 0) < run_name(2, 1, 0));
    }

    #[test]
    fn ranges_round_trip() {
        let set = |indices: &[u32]| indices.iter().copied().collect::<BTreeSet<u32>>();
        for indices in [&[][..], &[0], &[0, 1, 2, 5, 7, 8], &[3, 4, 9]] {
            let said = ranges(&set(indices));
            assert_eq!(parse_ranges(&said), Ok(set(indices)), "{said}");
        }
        assert_eq!(ranges(&set(&[0, 1, 2, 5, 7, 8])), "0-2,5,7-8");
        assert_eq!(ranges(&set(&[])), "-");
        assert!(parse_ranges("3-1").is_err() && parse_ranges("x").is_err());
    }

    fn found(whole: &[u32], torn: &[u32]) -> Found {
        Found { whole: whole.iter().copied().collect(), torn: torn.iter().copied().collect() }
    }

    fn acks(synced: Option<u32>, durable: Option<u32>) -> Acks {
        Acks { synced, durable }
    }

    /// What nothing lost keeps every file whose fsync returned.
    #[test]
    fn rounds_that_lose_nothing_keep_every_fsync() {
        let told = acks(Some(4), Some(2));
        assert_eq!(kept(told, &found(&[0, 1, 2, 3, 4], &[]), false), Ok(Some(4)));
        // Made after the last fsync told of, whole or torn: not held to.
        assert_eq!(kept(told, &found(&[0, 1, 2, 3, 4, 5], &[6]), false), Ok(Some(5)));
        assert!(kept(told, &found(&[0, 1, 2, 3], &[4]), false).is_err());
        assert!(kept(told, &found(&[0, 1, 3, 4], &[]), false).is_err());
        assert_eq!(kept(acks(None, None), &found(&[], &[0]), false), Ok(None));
    }

    /// What lost the daemon's commits keeps a prefix of the fsyncs, with every file syncfs was
    /// told of, and nothing made after a commit lost.
    #[test]
    fn rounds_that_lose_commits_keep_a_prefix() {
        let told = acks(Some(6), Some(2));
        assert_eq!(kept(told, &found(&[0, 1, 2, 3], &[]), true), Ok(Some(3)));
        assert_eq!(kept(told, &found(&[0, 1, 2, 3], &[4]), true), Ok(Some(3)));
        assert_eq!(kept(told, &found(&[0, 1, 2], &[]), true), Ok(Some(2)));
        assert!(kept(told, &found(&[0, 1], &[]), true).is_err(), "a syncfs's file lost");
        assert!(kept(told, &found(&[0, 1, 2, 3, 5], &[]), true).is_err(), "5 after 4 lost");
        assert!(kept(told, &found(&[0, 1, 2, 3], &[4, 5]), true).is_err(), "5 after 4 lost");
        assert!(kept(told, &found(&[0, 1, 2, 3], &[7]), true).is_err(), "7 after 4 lost");
        assert_eq!(kept(acks(Some(3), None), &found(&[], &[0]), true), Ok(None));
        assert!(kept(acks(Some(3), None), &found(&[], &[1]), true).is_err());
    }

    #[test]
    fn growth_past_twice_the_warm_rounds_fails() {
        let bound = Bound { warm: (2, 4), slack: 10 };
        assert_eq!(bound.grown("x", &[1000, 50, 60, 55]), None);
        assert_eq!(bound.grown("x", &[1000, 50, 60, 55, 130, 120]), None);
        assert_eq!(
            bound.grown("x", &[1000, 50, 60, 55, 131]),
            Some(
                "x grew to 131 in round 5: more than twice rounds 2-4's most, 60, and 10 more"
                    .to_owned()
            )
        );
    }
}
