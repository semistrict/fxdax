// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what the host does with a script that failed (TASK-149.2): boots a VM of the test for
//! each try of a shrink, the runner's `start-vm`, in a directory of its own, told the seed and the
//! operations to drop, and says the smallest script that failed the same way, which FXDAX_SEED and
//! FXDAX_RANDOM_OPS_DROP replay. FXDAX_SHRINK_TRIES bounds the VMs, as a hang takes the test's
//! timeout each, and 0 turns it off.

use crate::TestSetup;
use crate::script::{self, DROP_VAR, Failure, Mix, SEED_VAR, TRIES, kind, shrink_failure};
use crate::vm::Vm;
use anyhow::Context;
use std::ffi::CString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Sets the guest's script: the run's seed, and what FXDAX_RANDOM_OPS_DROP drops of it.
pub fn pass_script(vm: &mut Vm, seed: u64) -> anyhow::Result<()> {
    vm.env(&CString::new(format!("{SEED_VAR}={seed}"))?);
    if let Ok(drops) = std::env::var(DROP_VAR) {
        vm.env(&CString::new(format!("{DROP_VAR}={drops}"))?);
    }
    Ok(())
}

/// How a process ended, run to a deadline.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    Exited,
    TimedOut,
}

/// Runs `command`, killing it if it outlives `timeout`: a VM's process dies with it.
fn run_to_deadline(command: &mut Command, timeout: Duration) -> anyhow::Result<Ended> {
    let mut child = command.spawn().context("starting a replay")?;
    let deadline = Instant::now() + timeout;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Ok(Ended::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(Ended::Exited)
}

/// The replays of a failed test's script.
pub struct Replays<'a> {
    /// The runner, which boots a VM of the test (`start-vm`).
    pub runner: PathBuf,
    pub test_setup: &'a TestSetup,
    pub seed: u64,
    pub mix: Mix,
    /// What else the replays' environments need.
    pub env: &'a [(&'a str, &'a str)],
    pub timeout: Duration,
    /// How a replay failed, if it did, from its directory and standard output.
    pub verdict: &'a dyn Fn(&Path, &[u8]) -> Option<Failure>,
}

impl Replays<'_> {
    /// Replays the script with `drops` dropped, in `dir`.
    fn attempt(&self, dir: &Path, drops: &str) -> anyhow::Result<Option<Failure>> {
        fs::create_dir_all(dir.join("rootfs"))?;
        let stdout_path = dir.join("stdout.txt");
        let mut command = Command::new(&self.runner);
        command
            .arg("start-vm")
            .arg("--test-case")
            .arg(&self.test_setup.test_case)
            .arg("--tmp-dir")
            .arg(dir)
            .env("FXDAX_SEED", self.seed.to_string())
            .env(DROP_VAR, drops)
            .envs(self.env.iter().copied())
            .stdin(Stdio::null())
            .stdout(File::create(&stdout_path)?)
            .stderr(File::create(dir.join("log.txt"))?);
        Ok(match run_to_deadline(&mut command, self.timeout)? {
            Ended::TimedOut => Some(hang(self.timeout)),
            Ended::Exited => (self.verdict)(dir, &fs::read(&stdout_path)?),
        })
    }

    /// Shrinks the script that failed with `failure` and says the smallest that failed the same
    /// way, in the test's directory too, `shrunk.txt`, by the directory of its VM, `shrunk`.
    pub fn report(&self, failure: &Failure) -> String {
        // A replay of a script shrunk already is for looking at, and says what it is.
        if !failure.shrinks() || std::env::var_os(DROP_VAR).is_some() {
            return format!("{failure}\nFXDAX_SEED={} and {DROP_VAR} replay it", self.seed);
        }
        let ops = script::generate(self.seed, self.mix);
        let tries = std::env::var("FXDAX_SHRINK_TRIES")
            .ok()
            .map_or(TRIES, |tries| tries.parse().expect("FXDAX_SHRINK_TRIES is a count"));
        let shrunk_dir = self.test_setup.tmp_dir.join("shrunk");
        let mut ran = 0;
        let shrunk = shrink_failure(ops.len(), failure, tries, |drops| {
            ran += 1;
            let dir = self.test_setup.tmp_dir.join(format!("replay-{ran}"));
            match self.attempt(&dir, drops) {
                Ok(again) => {
                    let smallest = again.as_ref().is_some_and(|again| again.same_way(failure));
                    keep_or_remove(&dir, smallest.then_some(&shrunk_dir));
                    eprintln!("replay {ran}, dropping {drops}: {again:?}");
                    again
                }
                Err(err) => {
                    eprintln!("replay {ran} failed to run: {err:#}");
                    None
                }
            }
        });
        let seed = self.seed;
        let said = match &shrunk.kept {
            Some(kept) => {
                let drops = script::drops(kept, ops.len());
                let listed: Vec<String> =
                    kept.iter().map(|&index| format!("  {index}: {:?}", ops[index])).collect();
                format!(
                    "{failure}\nFXDAX_SEED={seed} {DROP_VAR}={drops} replays it: {} of {} \
                     operations, in {} tries:\n{}",
                    kept.len(),
                    ops.len(),
                    shrunk.tries,
                    listed.join("\n")
                )
            }
            None => format!(
                "{failure}\nFXDAX_SEED={seed} replays it, which {} tries of it didn't: it isn't \
                 shrunk",
                shrunk.tries
            ),
        };
        let path = self.test_setup.tmp_dir.join("shrunk.txt");
        if let Err(err) = fs::write(&path, &said) {
            eprintln!("saving {}: {err}", path.display());
        }
        said
    }
}

/// What a run that ran out of `timeout` failed with.
pub fn hang(timeout: Duration) -> Failure {
    let what = format!("still running after {} s", timeout.as_secs());
    Failure { op: None, kind: kind::HANG.to_owned(), what }
}

/// Keeps the directory of a replay as `kept`, in place of what's there, if any, or removes it.
fn keep_or_remove(dir: &Path, kept: Option<&PathBuf>) {
    let done = match kept {
        Some(kept) => (fs::remove_dir_all(kept))
            .or_else(|err| match err.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(err),
            })
            .and_then(|()| fs::rename(dir, kept)),
        None => fs::remove_dir_all(dir),
    };
    if let Err(err) = done {
        eprintln!("cleaning up {}: {err}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempdir::TempDir;

    #[test]
    fn a_process_that_ends_ends() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 3"]);
        let ended = run_to_deadline(&mut command, Duration::from_secs(60)).unwrap();
        assert_eq!(ended, Ended::Exited);
    }

    #[test]
    fn a_process_past_its_deadline_is_killed() {
        let dir = TempDir::new("deadline").unwrap();
        let marker = dir.path().join("pid");
        let mut command = Command::new("sh");
        command.arg("-c").arg(format!("echo $$ > {}; exec sleep 600", marker.display()));
        let ended = run_to_deadline(&mut command, Duration::from_millis(500)).unwrap();
        assert_eq!(ended, Ended::TimedOut);
        let pid = fs::read_to_string(&marker).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists(), "pid {pid} still runs");
    }

    #[test]
    fn a_process_that_cant_start_says_so() {
        let mut command = Command::new("/nonexistent/fxdax-replay");
        let err = run_to_deadline(&mut command, Duration::from_secs(1)).unwrap_err();
        assert_eq!(format!("{err}"), "starting a replay");
    }

    /// A runner that boots nothing: it says the seed, the checks and what it was told to drop, or
    /// if `hangs`, hangs if the operation before the script's end is kept.
    fn fake_runner(dir: &Path, hangs: bool) -> PathBuf {
        let runner = dir.join("runner");
        let hang = if hangs {
            "case \"$FXDAX_RANDOM_OPS_DROP\" in *-3010) ;; *) exec sleep 600;; esac"
        } else {
            ""
        };
        let body = format!(
            "#!/bin/sh\n{hang}\necho \"$FXDAX_SEED $FXDAX_CHECKS $FXDAX_RANDOM_OPS_DROP\"\n"
        );
        fs::write(&runner, body).unwrap();
        fs::set_permissions(&runner, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        runner
    }

    /// The failure of a replay that kept both of `pair`, from what the fake runner said.
    fn planted(dir: &Path, stdout: &[u8], pair: [usize; 2], seed: u64) -> Option<Failure> {
        assert!(dir.join("rootfs").is_dir());
        let said = String::from_utf8_lossy(stdout);
        let mut words = said.split_whitespace();
        assert_eq!(words.next(), Some(seed.to_string().as_str()));
        assert_eq!(words.next(), Some("random_ops"));
        let kept = script::kept(words.next().unwrap_or_default(), 3011).unwrap();
        pair.iter().all(|index| kept.contains(index)).then(|| Failure {
            op: kept.last().copied(),
            kind: kind::DIFFERS.to_owned(),
            what: "planted".to_owned(),
        })
    }

    /// A shrink through replays of a process each, told what the guest would be.
    #[test]
    fn a_report_says_the_smallest_script_that_failed_and_how_to_replay_it() {
        let dir = TempDir::new("replays").unwrap();
        let test_setup = TestSetup { test_case: "case".to_owned(), tmp_dir: dir.path().to_owned() };
        let seed = 77;
        let replays = Replays {
            runner: fake_runner(dir.path(), false),
            test_setup: &test_setup,
            seed,
            mix: script::ONE_FILE,
            env: &[("FXDAX_CHECKS", "random_ops")],
            timeout: Duration::from_secs(60),
            verdict: &|dir, stdout| planted(dir, stdout, [40, 2500], seed),
        };
        let first =
            Failure { op: Some(2500), kind: kind::DIFFERS.to_owned(), what: "first".into() };
        let said = replays.report(&first);
        let ops = script::generate(seed, script::ONE_FILE);
        assert_eq!(
            said,
            format!(
                "{first}\nFXDAX_SEED=77 FXDAX_RANDOM_OPS_DROP=0-39,41-2499,2501-3010 replays it: 2 \
                 of 3011 operations, in 48 tries:\n  40: {:?}\n  2500: {:?}",
                ops[40], ops[2500]
            )
        );
        assert_eq!(fs::read_to_string(dir.path().join("shrunk.txt")).unwrap(), said);
        let stdout = fs::read_to_string(dir.path().join("shrunk/stdout.txt")).unwrap();
        assert_eq!(stdout, "77 random_ops 0-39,41-2499,2501-3010\n");
        let left = fs::read_dir(dir.path()).unwrap().flatten();
        let left = left.filter(|entry| entry.file_name().to_string_lossy().starts_with("replay-"));
        assert_eq!(left.count(), 0, "replays' directories left behind");
    }

    /// A replay that runs out of its time fails as a hang.
    #[test]
    fn a_hang_is_shrunk() {
        let dir = TempDir::new("replays").unwrap();
        let test_setup = TestSetup { test_case: "case".to_owned(), tmp_dir: dir.path().to_owned() };
        let replays = Replays {
            runner: fake_runner(dir.path(), true),
            test_setup: &test_setup,
            seed: 1,
            mix: script::ONE_FILE,
            env: &[],
            timeout: Duration::from_millis(300),
            verdict: &|_, _| None,
        };
        let first = hang(Duration::from_millis(300));
        let said = replays.report(&first);
        assert_eq!(
            said.lines().nth(1),
            Some(
                "FXDAX_SEED=1 FXDAX_RANDOM_OPS_DROP=0-3008,3010 replays it: 1 of 3011 operations, in 26 tries:"
            )
        );
    }

    #[test]
    fn a_hang_is_a_failure_of_its_own_kind() {
        let failure = hang(Duration::from_secs(180));
        assert_eq!(
            failure.to_string(),
            "script failed: operation unknown: hang: still running after 180 s"
        );
    }

    #[test]
    fn the_smallest_replays_directory_replaces_the_one_before() {
        let dir = TempDir::new("replays").unwrap();
        let (first, second, kept) =
            (dir.path().join("1"), dir.path().join("2"), dir.path().join("kept"));
        for (replay, name) in [(&first, "first"), (&second, "second")] {
            fs::create_dir(replay).unwrap();
            fs::write(replay.join(name), name).unwrap();
        }
        keep_or_remove(&first, Some(&kept));
        keep_or_remove(&second, Some(&kept));
        assert_eq!(fs::read_to_string(kept.join("second")).unwrap(), "second");
        assert!(!kept.join("first").exists());
        assert!(!first.exists());
        fs::create_dir(&first).unwrap();
        keep_or_remove(&first, None);
        assert!(!first.exists());
        assert!(kept.exists());
    }
}
