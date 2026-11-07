// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a script run on two filesystems in one guest at once, step by step (TASK-44.15,
//! TASK-149.12): a reference the kernel has long had (ext4), and the filesystem under test (Fxfs).
//! Each step is done on the reference first, and both are held to the model (`script_run`): the
//! reference failing it is the model's mistake, or the kernel's, its own kind of failure
//! ([`kind::REFERENCE_DIFFERS`]), not taken for the subject's; the subject failing it, the reference
//! passing, is the subject's.
//!
//! What the model doesn't say, the reference does: which of the files' modification and change
//! times a step changed, which the two must agree on, as POSIX and Linux's VFS say they change but
//! no model here does. Those that differ are counted ([`Differences`]), the run going on, and are
//! failures unless [`ALLOWED`] lists them, with why. A store through a shared mapping says nothing
//! of times ([`Step::says_its_times`]).
//!
//! A subject that may refuse (fault injection) leaves a file as far as an operation got, which the
//! model takes; the reference's file is then made the same ([`Run::adopt`]), outside the script.

use crate::script::{self, Failure, Op, Role, kind};
use crate::script_run::{Effect, Ran, Run};
use std::collections::BTreeMap;
use std::fmt;

/// Differences in the times the filesystems change that are theirs to differ in, and why.
pub const ALLOWED: &[(Difference, &str)] = &[];

/// A script's two runs.
pub struct Differential {
    pub reference: Run,
    pub subject: Run,
    /// The seed of the script, which the differences say.
    seed: u64,
    pub differences: Differences,
    /// Whether each step is said on standard error before the subject does it: a run that dies,
    /// as a guest does whose VM went down, said which step it was at.
    pub says_steps: bool,
}

impl Differential {
    pub fn new(seed: u64, reference: Run, subject: Run) -> Self {
        let differences = Differences::default();
        Self { reference, subject, seed, differences, says_steps: false }
    }

    /// Runs `script`, what's left of a script of `len` operations, on both, and checks the files as
    /// it ends.
    pub fn script(&mut self, script: &[(usize, Op)], len: usize) -> Result<(), Failure> {
        for (index, op) in script {
            self.step(*index, op)?;
        }
        self.step(len, &Op::Check)
    }

    /// Does `op`, the script's `index`th, on the reference, then on the subject, and compares.
    pub fn step(&mut self, index: usize, op: &Op) -> Result<(), Failure> {
        let theirs = self.reference.step(index, op).map_err(|failure| Failure {
            kind: kind::REFERENCE_DIFFERS.to_owned(),
            what: format!("{}: {}", failure.kind, failure.what),
            ..failure
        })?;
        if self.says_steps {
            eprintln!("operation {index}: {}", theirs.step);
        }
        let ours = self.subject.step(index, op)?;
        let failure =
            |what: String| Failure { op: Some(index), kind: kind::DIFFERS.to_owned(), what };
        if theirs.step != ours.step {
            // Their models differ, which nothing failing let be: the runs' own mistake.
            let (theirs, ours) = (&theirs.step, &ours.step);
            return Err(failure(format!("`{theirs}` on the reference is `{ours}` here")));
        }
        if ours.refused {
            if let Some(file) = ours.step.changes() {
                self.reference.adopt(file, &self.subject.model.files[file])?;
            }
            return Ok(());
        }
        // Each run held it to the model's outcome for its own filesystem.
        if theirs.outcome != ours.outcome && !ours.step.outcome_is_the_filesystems() {
            let step = &ours.step;
            let (theirs, ours) = (theirs.outcome, ours.outcome);
            return Err(failure(format!("`{step}` came to {theirs:?} on the reference, {ours:?}")));
        }
        self.differences.note(self.seed, index, &theirs, &ours);
        Ok(())
    }

    /// Ends both runs, the files written back.
    pub fn finish(self) -> (Ran, Ran, Differences) {
        (self.reference.finish(), self.subject.finish(), self.differences)
    }
}

/// A time of a file's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Time {
    Modification,
    Change,
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Time::Modification => "mtime",
            Time::Change => "ctime",
        })
    }
}

/// How the filesystems' times differed: of what kind of operation ([`crate::script::Step::kind`]),
/// which time of which of its files, and whether each changed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Difference {
    pub op: &'static str,
    pub role: Role,
    pub time: Time,
    pub reference: bool,
    pub subject: bool,
}

impl Difference {
    /// Why it's allowed, if it is.
    pub fn allowed(&self) -> Option<&'static str> {
        ALLOWED.iter().find(|(allowed, _)| allowed == self).map(|(_, why)| *why)
    }
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let changes = |changed: bool| if changed { "changes" } else { "leaves" };
        let Self { op, role, time, reference, subject } = self;
        write!(
            f,
            "{op} {} the {time} of {role} on the reference, and {} it here",
            changes(*reference),
            changes(*subject)
        )
    }
}

/// Where a difference was first seen, and how often.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub count: usize,
    /// The seed of the script, the operation's index in it, and the operation, as it resolved.
    pub first: (u64, usize, String),
}

/// Every difference seen, and how often the times compared changed on both: none would say the
/// comparison looked at nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Differences(pub BTreeMap<Difference, Seen>, pub usize);

impl Differences {
    /// Notes how the times `theirs` and `ours` changed differ, if the step says its times.
    fn note(&mut self, seed: u64, index: usize, theirs: &Effect, ours: &Effect) {
        let step = &ours.step;
        if !step.says_its_times() {
            return;
        }
        for (file, (theirs, ours)) in theirs.changed.iter().zip(&ours.changed).enumerate() {
            let times = [
                (Time::Modification, theirs.mtime, ours.mtime),
                (Time::Change, theirs.ctime, ours.ctime),
            ];
            for (time, reference, subject) in times {
                if reference == subject {
                    self.1 += usize::from(reference);
                    continue;
                }
                let difference =
                    Difference { op: step.kind(), role: step.role(file), time, reference, subject };
                let seen = self
                    .0
                    .entry(difference)
                    .or_insert_with(|| Seen { count: 0, first: (seed, index, step.to_string()) });
                seen.count += 1;
            }
        }
    }

    /// Adds `other`'s, of a later script.
    pub fn add(&mut self, other: Differences) {
        self.1 += other.1;
        for (difference, seen) in other.0 {
            let ours = self.0.entry(difference).or_insert(Seen { count: 0, first: seen.first });
            ours.count += seen.count;
        }
    }

    /// Those not allowed.
    pub fn unallowed(&self) -> impl Iterator<Item = (&Difference, &Seen)> {
        self.0.iter().filter(|(difference, _)| difference.allowed().is_none())
    }

    /// Each, a line: `times: ` what differs, how often, where first, and why it's allowed if it is.
    pub fn lines(&self) -> Vec<String> {
        (self.0.iter())
            .map(|(difference, Seen { count, first: (seed, index, step) })| {
                let allowed = match difference.allowed() {
                    Some(why) => format!("{}{why})", script::ALLOWED),
                    None => String::new(),
                };
                format!(
                    "{}{difference}: {count} times, first at seed {seed}, operation {index}: \
                     {step}{allowed}",
                    script::TIMES
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{HOSTILE, Place, Step, WIDE};
    use crate::script_run::Changed;
    use crate::script_run::tests::Loop;
    use crate::script_run::{Options, ext4_largest, fill};
    use std::fs;
    use std::os::unix::fs::FileExt;
    use tempdir::TempDir;

    fn effect(step: Step, changed: Vec<Changed>) -> Effect {
        Effect { step, outcome: Ok(0), refused: false, changed }
    }

    const NEITHER: Changed = Changed { mtime: false, ctime: false };
    const BOTH: Changed = Changed { mtime: true, ctime: true };
    const CTIME: Changed = Changed { mtime: false, ctime: true };

    /// A step's times that differ are noted by its kind, the file's role and the time, once each,
    /// counted, the first seen kept.
    #[test]
    fn differing_times_are_noted_by_kind_role_and_time() {
        let mut differences = Differences::default();
        let clone = Step::CloneRange { from: 0, src: 0, to: 1, dst: 0, len: 0 };
        let theirs = effect(clone.clone(), vec![NEITHER, BOTH, NEITHER]);
        let ours = effect(clone.clone(), vec![CTIME, CTIME, NEITHER]);
        differences.note(7, 12, &theirs, &ours);
        differences.note(8, 40, &theirs, &ours);
        let first = (7, 12, clone.to_string());
        let difference = |role, time, reference, subject| Difference {
            op: "clone range",
            role,
            time,
            reference,
            subject,
        };
        assert_eq!(
            differences.0,
            BTreeMap::from([
                (
                    difference(Role::Source, Time::Change, false, true),
                    Seen { count: 2, first: first.clone() }
                ),
                (
                    difference(Role::Destination, Time::Modification, true, false),
                    Seen { count: 2, first }
                ),
            ])
        );
        assert_eq!(
            differences.lines()[0],
            "times: clone range leaves the ctime of its source on the reference, and changes it \
             here: 2 times, first at seed 7, operation 12: clone 0x0 at 0x0 of file 0 onto 0x0 of \
             file 1"
        );
        assert_eq!(differences.unallowed().count(), 2);
        // The source's mtime both left, the destination's ctime both changed, and the third file's
        // both left: one change agreed on, each time.
        assert_eq!(differences.1, 2);
    }

    /// A store through a shared mapping says nothing of times: whether it faults is the cache's.
    #[test]
    fn a_shared_stores_times_are_not_compared() {
        let mut differences = Differences::default();
        let store = Step::Store { file: 0, at: 0, data: vec![1], child: false };
        differences.note(1, 1, &effect(store.clone(), vec![BOTH]), &effect(store, vec![NEITHER]));
        let private = Step::MapStore { file: 0, at: 0, data: vec![1], private: true };
        differences.note(
            1,
            2,
            &effect(private.clone(), vec![NEITHER]),
            &effect(private, vec![BOTH]),
        );
        assert_eq!(differences.0.len(), 2);
        assert!(
            differences
                .0
                .keys()
                .all(|difference| difference.op == "store through a private mapping")
        );
    }

    #[test]
    fn differences_of_later_scripts_add_up() {
        let mut all = Differences::default();
        let write = Step::Write { file: 0, at: 0, data: vec![1] };
        for seed in [3, 4] {
            let mut differences = Differences::default();
            differences.note(
                seed,
                5,
                &effect(write.clone(), vec![BOTH]),
                &effect(write.clone(), vec![CTIME]),
            );
            all.add(differences);
        }
        let seen = all.0.values().next().unwrap();
        assert_eq!((seen.count, seen.first.0), (2, 3));
    }

    /// The files of a run in `dir`.
    fn names(dir: &std::path::Path) -> Vec<String> {
        (0..WIDE.files).map(|n| dir.join(format!("fsx-{n}")).to_str().unwrap().to_owned()).collect()
    }

    /// A differential run on two ext4 filesystems of the host's, reference and subject, each on a
    /// loop device of its own, mounted as fxfs-guest-fsx's reference is (`Loop::reference`),
    /// copying clones, which ext4 can't make: not the host's temporary
    /// directory's, whose filesystem may not zero ranges (tmpfs: EOPNOTSUPP, GCE r693). Fields
    /// drop in order, the runs' files first, the directory last.
    struct Host {
        differential: Differential,
        reference: Loop,
        subject: Loop,
        _dir: TempDir,
    }

    fn on_the_host() -> Host {
        let dir = TempDir::new("differential").unwrap();
        let ext4 = |name: &str| {
            let at = dir.path().join(name);
            fs::create_dir(&at).unwrap();
            Loop::reference(&at)
        };
        let (reference, subject) = (ext4("reference"), ext4("subject"));
        let largest = ext4_largest(reference.mountpoint.to_str().unwrap());
        let options = Options { copies_clones: true, seeks: true, largest, ..Options::default() };
        let theirs = Run::new(&names(&reference.mountpoint), options);
        let ours = Run::new(&names(&subject.mountpoint), options);
        Host { differential: Differential::new(1, theirs, ours), reference, subject, _dir: dir }
    }

    /// The wide mix's script, on two of one filesystem, passes, the models alike. Not its times,
    /// which a host's filesystem may keep coarse (before 6.13, or not multigrain): two changes in a
    /// tick leave one, which the other run's may not have had in one.
    #[test]
    fn a_wide_script_on_one_filesystem_twice_agrees() {
        // Bound whole: a pattern's `..` would drop the filesystems, unmounting them.
        let host = on_the_host();
        let mut differential = host.differential;
        let (script, len) = crate::script_run::numbered(1, WIDE, "");
        differential
            .script(&script, len)
            .unwrap_or_else(|failure| panic!("{failure}; {}", differential.subject.history()));
        let (reference, subject, _) = differential.finish();
        assert_eq!(reference.model, subject.model);
        drop((host.reference, host.subject, host._dir));
    }

    /// fxdax: and the hostile mix's (TASK-149.16), its far steps each held to its own filesystem's
    /// largest file.
    #[test]
    fn a_hostile_script_on_one_filesystem_twice_agrees() {
        let host = on_the_host();
        let mut differential = host.differential;
        let (script, len) = crate::script_run::numbered(1, HOSTILE, "");
        differential
            .script(&script, len)
            .unwrap_or_else(|failure| panic!("{failure}; {}", differential.subject.history()));
        let (reference, subject, _) = differential.finish();
        assert_eq!(reference.model, subject.model);
        drop((host.reference, host.subject, host._dir));
    }

    /// A byte the subject gets wrong is the subject's failure; the reference's, the model's.
    #[test]
    fn a_wrong_byte_is_the_subjects_or_the_references() {
        let write = Op::Write { file: 0, place: Place::At(0), len: 100, data: 1 };
        for (wrong, expected) in [(1, kind::DIFFERS), (0, kind::REFERENCE_DIFFERS)] {
            let mut host = on_the_host();
            host.differential.step(0, &write).unwrap();
            let dir = [&host.reference.mountpoint, &host.subject.mountpoint][wrong];
            fs::OpenOptions::new()
                .write(true)
                .open(dir.join("fsx-0"))
                .unwrap()
                .write_at(&[0], 50)
                .unwrap();
            let failure = host.differential.step(1, &Op::Check).unwrap_err();
            assert_eq!((failure.op, failure.kind.as_str()), (Some(1), expected));
        }
    }

    /// A subject with no room left refuses, as it may, and the reference's file is made what the
    /// subject's was left, the two going on alike to the script's end. Root's, as script_run's tests
    /// on filesystems of their own are.
    #[test]
    fn refusals_are_adopted_by_the_reference() {
        let (theirs, ours) = (TempDir::new("reference").unwrap(), TempDir::new("subject").unwrap());
        let room = Loop::new(theirs.path(), 300 << 20, "mkfs.ext4", &["-q", "-F"]);
        let ext4 = Loop::new(ours.path(), 8 << 20, "mkfs.ext4", &["-q", "-F", "-m", "0"]);
        let options = Options { copies_clones: true, seeks: true, ..Options::default() };
        let reference = Run::new(&names(&room.mountpoint), options);
        let subject = Run::new(&names(&ext4.mountpoint), Options { full: true, ..options });
        fill(ext4.mountpoint.join("ballast").to_str().unwrap(), 1 << 18);
        let mut differential = Differential::new(2, reference, subject);
        let (script, len) = crate::script_run::numbered(2, WIDE, "");
        differential
            .script(&script, len)
            .unwrap_or_else(|failure| panic!("{failure}; {}", differential.subject.history()));
        let (reference, subject, _) = differential.finish();
        assert!(subject.refused > 0, "nothing refused");
        assert_eq!(reference.refused, 0);
        assert_eq!(reference.model, subject.model);
    }
}
