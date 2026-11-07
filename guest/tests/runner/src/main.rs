// Copyright The libkrun Authors.
// Copyright 2026 The fxdax Authors: changed for fxdax's guest tests.
// SPDX-License-Identifier: Apache-2.0

use anyhow::Context;
use clap::Parser;
use nix::sys::resource::{Resource, getrlimit, setrlimit};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::panic::catch_unwind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempdir::TempDir;
use test_cases::{Report, ShouldRun, Tag, Test, TestCase, TestOutcome, TestSetup, test_cases};

struct TestResult {
    name: String,
    outcome: TestOutcome,
    log_path: Option<PathBuf>,
    /// fxdax: what the test ran, its outcome as data: what it took, the checks it failed, and
    /// what it chose that replays it (TASK-160.31). None if it didn't run.
    ran: Option<Ran>,
}

/// fxdax: what a test that ran says of itself besides its outcome.
struct Ran {
    /// How long it took, and its limit.
    took: std::time::Duration,
    limit: std::time::Duration,
    /// The checks its guest said failed (`failed_checks`).
    failed_checks: Vec<String>,
    /// What it chose that a replay needs, as the knobs that choose it: FXDAX_SEED, of a test that
    /// keeps its seed in its directory's file `seed` (fxfs-guest-checks', fxfs-guest-script's).
    chosen: BTreeMap<String, String>,
    /// What a test that ran out of time had to add (`Test::timed_out`).
    said: Option<String>,
}

/// fxdax: the checks a test's guest said failed, as the checks suites' guest says them as it ends,
/// a line `FAILED: a, b`, or as one outruns its budget, `FAILED: a, still running after …`: the
/// names of checks, identifiers, the rest of such a line left out.
fn failed_checks(stdout: &str) -> Vec<String> {
    let identifier = |word: &&str| {
        !word.is_empty()
            && word.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    let lines = stdout.lines().filter_map(|line| line.strip_prefix("FAILED: "));
    lines.flat_map(|names| names.split(", ").filter(identifier)).map(str::to_owned).collect()
}

/// fxdax: what a test chose that a replay needs, from what it left in its directory `dir`.
fn chosen(dir: &Path) -> BTreeMap<String, String> {
    let seed = fs::read_to_string(dir.join("seed")).ok();
    seed.map(|seed| ("FXDAX_SEED".to_owned(), seed.trim().to_owned())).into_iter().collect()
}

fn get_test(name: &str) -> anyhow::Result<Box<dyn Test>> {
    let tests = test_cases();
    tests
        .into_iter()
        .find(|t| t.name() == name)
        .with_context(|| format!("No such test: {name}"))
        .map(|t| t.test)
}

fn start_vm(test_setup: TestSetup) -> anyhow::Result<()> {
    // Raise soft fd limit up to the hard limit
    let (_soft_limit, hard_limit) =
        getrlimit(Resource::RLIMIT_NOFILE).context("getrlimit RLIMIT_NOFILE")?;
    setrlimit(Resource::RLIMIT_NOFILE, hard_limit, hard_limit)
        .context("setrlimit RLIMIT_NOFILE")?;

    let test = get_test(&test_setup.test_case)?;
    test.start_vm(test_setup.clone()).with_context(|| format!("testcase: {test_setup:?}"))?;
    Ok(())
}

/// Kill background processes registered for cleanup via
/// [`TestSetup::register_cleanup_pid`]. Sends SIGTERM first, waits up to 5s
/// for graceful exit, then SIGKILL any survivors.
fn kill_cleanup_pids(test_dir: &Path) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::io::BufRead;

    let Ok(file) = File::open(test_dir.join("cleanup.pids")) else {
        return;
    };

    let mut pids = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        if let Ok(raw) = line.trim().parse::<i32>() {
            pids.push(Pid::from_raw(raw));
        }
    }

    for &pid in &pids {
        let _ = kill(pid, Signal::SIGTERM);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        pids.retain(|&pid| kill(pid, None).is_ok());
        if pids.is_empty() || std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    for &pid in &pids {
        eprintln!("WARNING: cleanup process {} did not exit after SIGTERM, sending SIGKILL", pid);
        let _ = kill(pid, Signal::SIGKILL);
    }
}

/// fxdax: how the runner isolates a case: in a network namespace of its own, its loopback alone,
/// so that cases on one host share no ports (TASK-149.20.6).
#[derive(Debug, PartialEq, Eq)]
enum Isolation {
    /// `unshare --net`, as root: root maps its own IDs.
    Net,
    /// `buildah unshare`, mapping the user's subordinate IDs, then `unshare --net`: as any other user.
    MappedNet,
    /// None, and why.
    None(&'static str),
}

/// fxdax: what [`isolation`] goes by.
struct Host {
    linux: bool,
    root: bool,
    /// FXDAX_NO_UNSHARE: set, the cases run as they are.
    no_unshare: bool,
    /// FXDAX_EXTENTD_REMOTE, the bucket cases' own extent daemons keep their stores in.
    remote: Option<String>,
    has: fn(&str) -> bool,
}

impl Host {
    fn this() -> Self {
        Self {
            linux: cfg!(target_os = "linux"),
            root: nix::unistd::geteuid().is_root(),
            no_unshare: std::env::var_os("FXDAX_NO_UNSHARE").is_some(),
            remote: std::env::var("FXDAX_EXTENTD_REMOTE").ok().filter(|url| !url.is_empty()),
            has: on_path,
        }
    }
}

/// fxdax: whether `command` is an executable file in a directory of PATH. Not whether it runs with
/// `--version`, which `ip` refuses: the hosts ran every case unisolated, `ip` taken for missing
/// (GCE r961).
fn on_path(command: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| {
        let metadata = std::fs::metadata(dir.join(command));
        metadata.is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

/// fxdax: how a case runs on `host`. Root, as on the GCE hosts, unshares its network itself:
/// `buildah unshare` as root, with no subordinate IDs, entered user namespace after user namespace
/// till its memfd's name, which grows with each, was past the kernel's 249 bytes (`memfd_create:
/// invalid argument`), which once had the hosts run every case unisolated (FXDAX_NO_UNSHARE). A case
/// whose own extent daemons keep their stores in a bucket over the network, as a GCS run's do,
/// runs in the host's network.
fn isolation(host: &Host) -> Isolation {
    let networked = |url: &str| !url.starts_with("file://") && !url.starts_with("memory://");
    if !host.linux {
        Isolation::None("network namespaces are Linux's")
    } else if host.no_unshare {
        Isolation::None("FXDAX_NO_UNSHARE is set")
    } else if host.remote.as_deref().is_some_and(networked) {
        Isolation::None("FXDAX_EXTENTD_REMOTE names a bucket over the network")
    } else if !(host.has)("unshare") || !(host.has)("ip") {
        Isolation::None("unshare or ip is missing")
    } else if host.root {
        Isolation::Net
    } else if (host.has)("buildah") {
        Isolation::MappedNet
    } else {
        Isolation::None("buildah, which maps a user's IDs, is missing")
    }
}

fn run_single_test(
    test_case: &TestCase,
    base_dir: &Path,
    keep_all: bool,
    max_name_len: usize,
) -> anyhow::Result<TestResult> {
    eprint!(
        "[{}] {:.<width$} ",
        test_case.name,
        "",
        width = max_name_len - test_case.name.len() + 3
    );

    // Check if test should run
    if let ShouldRun::No(reason) = test_case.should_run() {
        eprintln!("SKIP ({})", reason);
        return Ok(TestResult {
            name: test_case.name.to_string(),
            outcome: TestOutcome::Skip(reason),
            log_path: None,
            ran: None,
        });
    }

    let executable = env::current_exe().context("Failed to detect current executable")?;
    let test_dir = base_dir.join(test_case.name);
    fs::create_dir(&test_dir).context("Failed to create test directory")?;

    // Prepare rootfs: build from container image if needed, otherwise create empty dir
    let rootfs_dir = test_dir.join("rootfs");
    if let Some(containerfile) = test_case.rootfs_image() {
        use test_cases::rootfs;
        if let Err(e) = rootfs::prepare_rootfs(containerfile, &rootfs_dir) {
            eprintln!("SKIP ({e})");
            return Ok(TestResult {
                name: test_case.name.to_string(),
                outcome: TestOutcome::Skip("rootfs image build failed"),
                log_path: None,
                ran: None,
            });
        }
    } else {
        fs::create_dir(&rootfs_dir).context("Failed to create rootfs directory")?;
    }

    let log_path = test_dir.join("log.txt");
    let log_file = File::create(&log_path).context("Failed to create log file")?;
    let stdout_path = test_dir.join("stdout.txt");
    let stdout_file = File::create(&stdout_path).context("Failed to create stdout file")?;

    let isolation = isolation(&Host::this());
    // Enforce a per-test timeout. If the child doesn't exit within the
    // deadline, kill it so we don't hang the entire suite.
    let timeout = std::time::Duration::from_secs(test_case.timeout_secs());
    // fxdax: the deadline, for the test's VMs, whose guests say where they are stuck before it.
    let killed_at = (std::time::SystemTime::now() + timeout)
        .duration_since(std::time::UNIX_EPOCH)
        .context("the clock is before the epoch")?
        .as_secs()
        .to_string();
    // The case, in a network of its own, its loopback up, unless it runs as is: `unshare`'s
    // arguments.
    let net = |command: &mut Command| {
        command.args(["--net", "--", "sh", "-c"]).arg(format!(
            "ip addr add 127.0.0.1/8 dev lo && ip link set lo up || exit 1; exec {} start-vm \
             --test-case {} --tmp-dir {}",
            executable.display(),
            test_case.name,
            test_dir.display()
        ));
    };
    let mut command = match &isolation {
        Isolation::Net => {
            let mut command = Command::new("unshare");
            net(&mut command);
            command
        }
        Isolation::MappedNet => {
            let mut command = Command::new("buildah");
            command.args(["unshare", "--", "unshare"]);
            net(&mut command);
            command
        }
        Isolation::None(why) => {
            eprintln!("WARNING: running {} without network isolation: {why}.", test_case.name);
            let mut command = Command::new(&executable);
            command.arg("start-vm").arg("--test-case").arg(test_case.name).arg("--tmp-dir");
            command.arg(&test_dir);
            command
        }
    };
    let child = command
        .env(test_cases::TEST_DEADLINE, &killed_at)
        .stdin(Stdio::piped())
        .stdout(stdout_file)
        .stderr(log_file)
        .spawn()
        .context("Failed to start subprocess for test")?;

    let mut child = child;
    let began = std::time::Instant::now();
    let deadline = began + timeout;
    let exited = loop {
        if let Some(status) = child.try_wait().unwrap_or(None) {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("TIMEOUT ({}s)", timeout.as_secs());
            save_stacks(&test_dir, child.id());
            let _ = child.kill();
            let _ = child.wait();
            kill_cleanup_pids(&test_dir);
            let test_name = test_case.name.to_string();
            let test_dir_clone = test_dir.clone();
            let said = catch_unwind(|| {
                let test = get_test(&test_name).unwrap();
                test.timed_out(TestSetup { test_case: test_name, tmp_dir: test_dir_clone })
            });
            let said = said.ok().flatten();
            if let Some(said) = &said {
                eprintln!("{said}");
            }
            let ran = Ran {
                took: began.elapsed(),
                limit: timeout,
                failed_checks: Vec::new(),
                chosen: chosen(&test_dir),
                said,
            };
            return Ok(TestResult {
                name: test_case.name.to_string(),
                outcome: TestOutcome::Timeout,
                log_path: Some(log_path),
                ran: Some(ran),
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };

    // fxdax: how long it took of its limit, which the verdict says: a test near its limit shows
    // before it times out (TASK-168).
    let elapsed = began.elapsed();
    let took = format!("{} s of {} s", elapsed.as_secs(), timeout.as_secs());
    let stdout = fs::read(&stdout_path).unwrap_or_default();
    let said_failed = failed_checks(&String::from_utf8_lossy(&stdout));

    let test_name = test_case.name.to_string();
    let checks_console = get_test(&test_name).unwrap().checks_console();
    let test_dir_clone = test_dir.clone();
    let outcome = match catch_unwind(|| {
        let test = get_test(&test_name).unwrap();
        let test_setup = TestSetup { test_case: test_name, tmp_dir: test_dir_clone };
        test.check(stdout, test_setup)
    }) {
        Ok(outcome) => outcome,
        Err(_) => TestOutcome::Fail("test.check() panicked".to_string()),
    };
    // fxdax: a test whose host side failed, its error in the log, or died, fails, though its
    // guests said what they should: start_vm's errors went unseen, as did its process's aborting.
    let outcome = match outcome {
        TestOutcome::Pass if !exited.success() => {
            TestOutcome::Fail(format!("the test's host side exited with {exited}; see its log"))
        }
        outcome => outcome,
    };
    // fxdax: a test that passed fails if a kernel of its VMs complained (TASK-44.8).
    let outcome = match outcome {
        TestOutcome::Pass if checks_console => match consoles_complaints(&test_dir) {
            Ok(()) => TestOutcome::Pass,
            Err(err) => TestOutcome::Fail(format!("{err:#}")),
        },
        outcome => outcome,
    };

    // Kill any background processes registered for cleanup (e.g. gvproxy).
    // Runs after check() regardless of outcome, so leaked processes are
    // cleaned up even if the test crashed.
    kill_cleanup_pids(&test_dir);
    // fxdax: before a test that passed has its directory removed. The checks it failed only if it
    // failed: an overrun run passes on its check's failing (TASK-168).
    let failed = if matches!(outcome, TestOutcome::Fail(_)) { said_failed } else { Vec::new() };
    let ran = Ran {
        took: elapsed,
        limit: timeout,
        failed_checks: failed,
        chosen: chosen(&test_dir),
        said: None,
    };

    match &outcome {
        TestOutcome::Pass => {
            eprintln!("OK ({took})");
            if !keep_all {
                let _ = fs::remove_dir_all(&test_dir);
            }
        }
        TestOutcome::Fail(reason) => {
            eprintln!("FAIL ({took}):");
            eprintln!("{reason}");
        }
        TestOutcome::Skip(reason) => {
            eprintln!("SKIP ({})", reason);
        }
        TestOutcome::Timeout => {
            eprintln!("TIMEOUT");
        }
        TestOutcome::Report(report) => {
            eprintln!("REPORT ({took})");
            eprintln!("{:2}", report.text());
            if !keep_all {
                let _ = fs::remove_dir_all(&test_dir);
            }
        }
    }

    Ok(TestResult {
        name: test_case.name.to_string(),
        outcome,
        log_path: Some(log_path),
        ran: Some(ran),
    })
}

/// fxdax: saves, before a test that hung is killed, the stacks of every thread of its processes,
/// `pid`'s and its descendants', and those it registered for cleanup, to `stacks-<pid>.txt` in its
/// directory `dir`: what waits for what, which a timeout alone kept nothing of (TASK-44.13). With
/// elfutils' eu-stack; without it, a note says so.
fn save_stacks(dir: &Path, pid: u32) {
    let mut pids = vec![pid];
    let mut at = 0;
    while at < pids.len() {
        let tasks = fs::read_dir(format!("/proc/{}/task", pids[at])).into_iter().flatten();
        for task in tasks.flatten() {
            let children = fs::read_to_string(task.path().join("children")).unwrap_or_default();
            pids.extend(children.split_whitespace().filter_map(|child| child.parse::<u32>().ok()));
        }
        at += 1;
    }
    let cleanup = fs::read_to_string(dir.join("cleanup.pids")).unwrap_or_default();
    pids.extend(cleanup.lines().filter_map(|line| line.trim().parse::<u32>().ok()));
    pids.sort_unstable();
    pids.dedup();
    for pid in pids {
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        match Command::new("eu-stack").args(["-p", &pid.to_string()]).output() {
            Ok(output) => {
                let mut saved = format!("{pid} {}\n", comm.trim()).into_bytes();
                saved.extend(output.stdout);
                saved.extend(output.stderr);
                let path = dir.join(format!("stacks-{pid}.txt"));
                if let Err(err) = fs::write(&path, saved) {
                    eprintln!("saving {}: {err}", path.display());
                }
            }
            Err(err) => {
                eprintln!("no stacks of the hung test's processes: eu-stack: {err}");
                return;
            }
        }
    }
    eprintln!("the hung test's stacks: {}/stacks-*.txt", dir.display());
}

/// fxdax: fails with what the kernels of a test's VMs complained of on their consoles, the files
/// `console.txt` and `<name>.console.txt` in its directory `dir` (`vm::complaints`).
fn consoles_complaints(dir: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|name| name.to_string_lossy().ends_with("console.txt")) {
            test_cases::vm::complaints(&path).with_context(|| format!("in {}", path.display()))?;
        }
    }
    Ok(())
}

fn write_github_summary(
    results: &[TestResult],
    num_pass: usize,
    num_fail: usize,
    num_skip: usize,
    num_report: usize,
) -> anyhow::Result<()> {
    let summary_path = env::var("GITHUB_STEP_SUMMARY")
        .context("GITHUB_STEP_SUMMARY environment variable not set")?;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&summary_path)
        .context("Failed to open GITHUB_STEP_SUMMARY")?;

    let num_ran = num_pass + num_fail;
    let status = if num_fail == 0 { "✅" } else { "❌" };
    let mut extra = Vec::new();
    if num_skip > 0 {
        extra.push(format!("{num_skip} skipped"));
    }
    if num_report > 0 {
        extra.push(format!("{num_report} reports"));
    }
    let extra_msg =
        if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) };

    writeln!(file, "## {status} Integration Tests - {num_pass}/{num_ran} passed{extra_msg}\n")?;

    for result in results {
        let (icon, status_text) = match &result.outcome {
            TestOutcome::Pass => ("✅", String::new()),
            TestOutcome::Fail(_) => ("❌", String::new()),
            TestOutcome::Skip(reason) => ("⏭️", format!(" - {}", reason)),
            TestOutcome::Timeout => ("⏳", String::from(" - Timeout")),
            TestOutcome::Report(_) => ("📊", String::new()),
        };

        writeln!(file, "<details>")?;
        writeln!(file, "<summary>{icon} {}{}</summary>\n", result.name, status_text)?;

        if let TestOutcome::Fail(reason) = &result.outcome {
            writeln!(file, "**Error:**\n```\n{reason}\n```\n---\n")?;
        }

        if let TestOutcome::Report(report) = &result.outcome {
            writeln!(file, "{}", report.gh_markdown())?;
        } else if let Some(log_path) = &result.log_path {
            let log_content = fs::read_to_string(log_path).unwrap_or_default();
            writeln!(file, "```")?;
            // Limit log size to avoid huge summaries (2 MiB limit)
            const MAX_LOG_SIZE: usize = 2 * 1024 * 1024;
            let truncated = if log_content.len() > MAX_LOG_SIZE {
                format!(
                    "... (truncated, showing last 1 MiB) ...\n{}",
                    &log_content[log_content.len() - MAX_LOG_SIZE..]
                )
            } else {
                log_content
            };
            writeln!(file, "{truncated}")?;
            writeln!(file, "```")?;
        }

        writeln!(file, "</details>\n")?;
    }

    Ok(())
}

/// fxdax: where `runner test` writes its case's outcome as data, and of which round of which run
/// (TASK-160.31).
struct Record {
    outcome: PathBuf,
    /// scripts/guest_results.py's run.json: its commit, its knobs and its rounds.
    run: Option<PathBuf>,
    round: u32,
}

/// fxdax: `result`'s outcome as data, in round `round` of the run `run` says (`Record::run`), with
/// the knobs that replay it, those of the run and those the case chose, and the command that does.
fn outcome_json(result: &TestResult, run: &serde_json::Value, round: u32) -> serde_json::Value {
    let ran = result.ran.as_ref();
    let (outcome, reason) = match &result.outcome {
        TestOutcome::Pass => ("pass", None),
        TestOutcome::Fail(reason) => ("fail", Some(reason.clone())),
        TestOutcome::Timeout => ("timeout", ran.and_then(|ran| ran.said.clone())),
        TestOutcome::Skip(reason) => ("skip", Some(reason.to_string())),
        TestOutcome::Report(_) => ("report", None),
    };
    let given = run["knobs"].as_object().into_iter().flatten();
    let mut knobs: BTreeMap<String, String> = given
        .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
        .collect();
    knobs.extend(ran.into_iter().flat_map(|ran| ran.chosen.clone()));
    serde_json::json!({
        "case": result.name,
        "round": round,
        "rounds": run["rounds"].as_u64().unwrap_or(1),
        "outcome": outcome,
        "reason": reason,
        "failed_checks": ran.map(|ran| ran.failed_checks.clone()).unwrap_or_default(),
        "took_secs": ran.map(|ran| ran.took.as_secs_f64()),
        "limit_secs": ran.map(|ran| ran.limit.as_secs()),
        "commit": run["commit"],
        "replay": replay(&result.name, &knobs),
        "knobs": knobs,
    })
}

/// fxdax: the command that runs `case` again on a GCE host as it ran, its knobs `knobs`.
fn replay(case: &str, knobs: &BTreeMap<String, String>) -> String {
    let mut words = vec!["FXDAX_WORKSPACE_TESTS=0".to_owned(), format!("FXDAX_E2E_TEST={case}")];
    let others = knobs
        .iter()
        .filter(|(name, _)| !["FXDAX_WORKSPACE_TESTS", "FXDAX_E2E_TEST"].contains(&name.as_str()));
    words.extend(others.map(|(name, value)| format!("{name}={}", shell_word(value))));
    words.push("scripts/test-gce.sh all".to_owned());
    words.join(" ")
}

/// fxdax: `value` as a shell reads it back: as it is if it holds nothing a shell treats specially,
/// in single quotes otherwise.
fn shell_word(value: &str) -> String {
    let plain = |b: u8| b.is_ascii_alphanumeric() || b"_./:=,@%+-".contains(&b);
    if !value.is_empty() && value.bytes().all(plain) {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn run_tests(
    test_case: &str,
    base_dir: Option<PathBuf>,
    keep_all: bool,
    github_summary: bool,
    record: Option<Record>,
) -> anyhow::Result<()> {
    // Create the base directory - either use provided path or create a temp one
    let base_dir = match base_dir {
        Some(path) => {
            fs::create_dir_all(&path).context("Failed to create base directory")?;
            path
        }
        None => TempDir::new_in("/tmp", "fxdax-guest-tests")
            .context("Failed to create temp base directory")?
            .into_path(),
    };

    let mut results: Vec<TestResult> = Vec::new();
    let all_tests = test_cases();

    let tests_to_run: Vec<_> = if test_case == "all" {
        all_tests
    } else {
        all_tests.into_iter().filter(|t| t.name == test_case).collect()
    };

    if tests_to_run.is_empty() {
        anyhow::bail!("No such test: {test_case}");
    }
    anyhow::ensure!(
        record.is_none() || tests_to_run.len() == 1,
        "--outcome is a single case's: name it"
    );

    let max_name_len = tests_to_run.iter().map(|t| t.name.len()).max().unwrap_or(0);

    for tc in &tests_to_run {
        results.push(run_single_test(tc, &base_dir, keep_all, max_name_len).context(tc.name)?);
    }
    if let (Some(record), [result]) = (&record, &results[..]) {
        let run = match &record.run {
            Some(path) => {
                serde_json::from_slice(&fs::read(path).with_context(|| path.display().to_string())?)
                    .with_context(|| path.display().to_string())?
            }
            None => serde_json::Value::Null,
        };
        let mut outcome = serde_json::to_vec_pretty(&outcome_json(result, &run, record.round))?;
        outcome.push(b'\n');
        fs::write(&record.outcome, outcome)
            .with_context(|| record.outcome.display().to_string())?;
    }

    let num_pass = results.iter().filter(|r| matches!(r.outcome, TestOutcome::Pass)).count();
    let num_fail = results
        .iter()
        .filter(|r| matches!(r.outcome, TestOutcome::Fail(_) | TestOutcome::Timeout))
        .count();
    let num_skip = results.iter().filter(|r| matches!(r.outcome, TestOutcome::Skip(_))).count();
    let num_report = results.iter().filter(|r| matches!(r.outcome, TestOutcome::Report(_))).count();
    let num_ran = num_pass + num_fail;

    // Write GitHub Actions summary if requested
    if github_summary {
        write_github_summary(&results, num_pass, num_fail, num_skip, num_report)?;
    }

    let mut extra = Vec::new();
    if num_skip > 0 {
        extra.push(format!("{num_skip} skipped"));
    }
    if num_report > 0 {
        extra.push(format!("{num_report} reports"));
    }
    let extra_msg =
        if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) };

    // fxdax: a case asked for by name that skips fails: it was to run. Fresh GCE hosts without a
    // guest kernel skipped every guest case asked for, and the run passed, round after round.
    let skipped_asked = test_case != "all" && num_skip > 0;
    if num_fail > 0 || skipped_asked {
        eprintln!("(See test artifacts at: {})", base_dir.display());
        println!("\nFAIL - {num_pass}/{num_ran} passed{extra_msg}");
        if skipped_asked {
            println!("{test_case} was asked for, and skipped");
        }
        anyhow::bail!("")
    } else {
        if keep_all {
            eprintln!("(See test artifacts at: {})", base_dir.display());
        }
        eprintln!("\nOK - {num_pass}/{num_ran} passed{extra_msg}");
    }

    Ok(())
}

#[derive(clap::Subcommand, Clone, Debug)]
enum CliCommand {
    Test {
        /// Specify which test to run or "all"
        #[arg(long, default_value = "all")]
        test_case: String,
        /// Base directory for test artifacts
        #[arg(long)]
        base_dir: Option<PathBuf>,
        /// Keep all test artifacts even on success
        #[arg(long)]
        keep_all: bool,
        /// Write test results to GitHub Actions job summary ($GITHUB_STEP_SUMMARY)
        #[arg(long)]
        github_summary: bool,
        /// fxdax: write the case's outcome, as JSON, to this file
        #[arg(long)]
        outcome: Option<PathBuf>,
        /// fxdax: the run the case is of, its commit, knobs and rounds (a run.json of
        /// scripts/guest_results.py's), which its outcome records
        #[arg(long, requires = "outcome")]
        run: Option<PathBuf>,
        /// fxdax: the round of the run it is, which its outcome records
        #[arg(long, default_value_t = 1, requires = "outcome")]
        round: u32,
    },
    StartVm {
        #[arg(long)]
        test_case: String,
        #[arg(long)]
        tmp_dir: PathBuf,
    },
    /// fxdax: prints the names of the cases selected, a line each, as registered: what the
    /// scripts run, by tag, keeping no lists of names (TASK-160.29).
    List {
        /// Every case of the suite TAG
        #[arg(long = "tag", value_name = "TAG", value_parser = tag_parser())]
        tags: Vec<Tag>,
        /// The cases of these names
        names: Vec<String>,
    },
}

/// fxdax: a [`Tag`] by its name, the tags and what each selects in `--help`.
fn tag_parser() -> impl clap::builder::TypedValueParser<Value = Tag> {
    use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};
    let tags = Tag::ALL.iter().map(|tag| PossibleValue::new(tag.name()).help(tag.about()));
    PossibleValuesParser::new(tags).map(|name| Tag::named(&name).expect("a tag clap took"))
}

/// fxdax: `runner list`'s, which fails if it selects nothing: a run of no cases passes.
fn list(names: &[String], tags: &[Tag]) -> anyhow::Result<()> {
    let cases = test_cases::select(test_cases(), names, tags).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(!cases.is_empty(), "no test case selected: name cases, or --tag a suite");
    for case in cases {
        println!("{}", case.name);
    }
    Ok(())
}

impl Default for CliCommand {
    fn default() -> Self {
        Self::Test {
            test_case: "all".to_string(),
            base_dir: None,
            keep_all: false,
            github_summary: false,
            outcome: None,
            run: None,
            round: 1,
        }
    }
}

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or_default();

    match command {
        CliCommand::StartVm { test_case, tmp_dir } => start_vm(TestSetup { test_case, tmp_dir }),
        CliCommand::List { tags, names } => list(&names, &tags),
        CliCommand::Test { test_case, base_dir, keep_all, github_summary, outcome, run, round } => {
            let record = outcome.map(|outcome| Record { outcome, run, round });
            run_tests(&test_case, base_dir, keep_all, github_summary, record)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Host, Isolation, isolation, on_path};

    /// A Linux host as root with every command, FXDAX_EXTENTD_REMOTE `remote`.
    fn host(remote: Option<&str>) -> Host {
        let remote = remote.map(str::to_owned);
        Host { linux: true, root: true, no_unshare: false, remote, has: |_| true }
    }

    /// A command is found on PATH, executable, whatever it says to `--version`.
    #[test]
    fn commands_are_found_on_the_path() {
        assert!(on_path("sh"));
        assert!(!on_path("no-such-command-anywhere"));
    }

    /// TASK-149.20.6: root unshares its network itself, as `buildah unshare` failed as root on the
    /// GCE hosts; another user maps its IDs with buildah first; a case whose daemons reach a bucket
    /// over the network, or a host told not to, isolates nothing.
    #[test]
    fn cases_run_in_a_network_of_their_own_but_where_they_need_one() {
        assert_eq!(isolation(&host(None)), Isolation::Net);
        assert_eq!(isolation(&host(Some("file:///dev/shm/bucket/"))), Isolation::Net);
        assert_eq!(isolation(&host(Some("memory:///vmm"))), Isolation::Net);
        assert_eq!(
            isolation(&host(Some("gs://bucket/run"))),
            Isolation::None("FXDAX_EXTENTD_REMOTE names a bucket over the network")
        );
        assert_eq!(isolation(&Host { root: false, ..host(None) }), Isolation::MappedNet);
        assert_eq!(
            isolation(&Host { root: false, has: |command| command != "buildah", ..host(None) }),
            Isolation::None("buildah, which maps a user's IDs, is missing")
        );
        assert_eq!(
            isolation(&Host { has: |command| command != "ip", ..host(None) }),
            Isolation::None("unshare or ip is missing")
        );
        assert_eq!(
            isolation(&Host { no_unshare: true, ..host(None) }),
            Isolation::None("FXDAX_NO_UNSHARE is set")
        );
        assert_eq!(
            isolation(&Host { linux: false, ..host(None) }),
            Isolation::None("network namespaces are Linux's")
        );
    }

    use super::*;

    /// fxdax: a checks suite's guest's failures as it says them, and nothing else on its lines.
    #[test]
    fn failed_checks_are_those_the_guest_names() {
        let stdout = "OK base\nFAILED: sparse, acls\nFAILED: random_ops, still running after its \
            budget of 1.50s\nFAILED setxattr\n";
        assert_eq!(failed_checks(stdout), ["sparse", "acls", "random_ops"]);
        assert_eq!(failed_checks("OK\n"), Vec::<String>::new());
    }

    #[test]
    fn shell_words_read_back_as_they_are() {
        assert_eq!(shell_word("gs://bucket/prefix"), "gs://bucket/prefix");
        assert_eq!(shell_word("--tag e2e"), "'--tag e2e'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
        assert_eq!(shell_word(""), "''");
    }

    /// fxdax: a round's outcome as data: the run's knobs and those the case chose, which its
    /// replay runs it with, alone, on a GCE host.
    #[test]
    fn an_outcome_says_how_to_replay_it() {
        let ran = Ran {
            took: std::time::Duration::from_millis(41_500),
            limit: std::time::Duration::from_secs(180),
            failed_checks: vec!["sparse".to_owned()],
            chosen: [("FXDAX_SEED".to_owned(), "77".to_owned())].into(),
            said: None,
        };
        let result = TestResult {
            name: "fxfs-guest-checks-dax".to_owned(),
            outcome: TestOutcome::Fail("seed 77: expected exactly \"OK\\n\"".to_owned()),
            log_path: None,
            ran: Some(ran),
        };
        let run = serde_json::json!({
            "commit": "abc123",
            "rounds": 5,
            "knobs": {
                "FXDAX_E2E_TEST": "--tag dax-gate",
                "FXDAX_E2E_ROUNDS": "5",
                "FXDAX_GUEST_CMDLINE": "fxfs.follow_apply_delay_us=200 quiet",
                "FXDAX_WORKSPACE_TESTS": "0",
            },
        });
        assert_eq!(
            outcome_json(&result, &run, 3),
            serde_json::json!({
                "case": "fxfs-guest-checks-dax",
                "round": 3,
                "rounds": 5,
                "outcome": "fail",
                "reason": "seed 77: expected exactly \"OK\\n\"",
                "failed_checks": ["sparse"],
                "took_secs": 41.5,
                "limit_secs": 180,
                "commit": "abc123",
                "replay": "FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=fxfs-guest-checks-dax \
                    FXDAX_E2E_ROUNDS=5 FXDAX_GUEST_CMDLINE='fxfs.follow_apply_delay_us=200 quiet' \
                    FXDAX_SEED=77 scripts/test-gce.sh all",
                "knobs": {
                    "FXDAX_E2E_ROUNDS": "5",
                    "FXDAX_E2E_TEST": "--tag dax-gate",
                    "FXDAX_GUEST_CMDLINE": "fxfs.follow_apply_delay_us=200 quiet",
                    "FXDAX_SEED": "77",
                    "FXDAX_WORKSPACE_TESTS": "0",
                },
            })
        );
    }

    /// fxdax: a case run without a run's record, by hand, and skipped.
    #[test]
    fn an_outcome_of_no_run() {
        let result = TestResult {
            name: "runner-deadlock".to_owned(),
            outcome: TestOutcome::Skip("a deadlock on purpose"),
            log_path: None,
            ran: None,
        };
        let outcome = outcome_json(&result, &serde_json::Value::Null, 1);
        assert_eq!(
            (&outcome["outcome"], &outcome["reason"], &outcome["rounds"], &outcome["commit"]),
            (
                &serde_json::json!("skip"),
                &serde_json::json!("a deadlock on purpose"),
                &serde_json::json!(1),
                &serde_json::Value::Null
            )
        );
        assert_eq!(
            outcome["replay"],
            "FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=runner-deadlock scripts/test-gce.sh all"
        );
    }
}
