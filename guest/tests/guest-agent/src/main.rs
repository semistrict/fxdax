// Copyright The libkrun Authors.
// Copyright 2026 The fxdax Authors: changed for fxdax's guest tests.
// SPDX-License-Identifier: Apache-2.0

use anyhow::Context;
use std::env::args;
use test_cases::{TestCase, test_cases};

fn run_guest_agent(test_name: &str) -> anyhow::Result<()> {
    let tests = test_cases();
    let test_case = tests
        .into_iter()
        .find(|t| t.name() == test_name)
        .context("No such test!")?;
    let TestCase { test, .. } = test_case;
    test.in_guest();
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut cli_args = args();
    let _exec_name = cli_args.next();
    let test_name = cli_args.next().context("Missing test name argument")?;
    // The guest's init, under fxdax's Firecracker, which runs the test and resets the VM, whatever
    // becomes of the test.
    #[cfg(target_os = "linux")]
    if std::process::id() == 1 {
        let init = test_cases::guest_init::set_up();
        if let Ok(Err(err)) = std::panic::catch_unwind(|| run_guest_agent(&test_name)) {
            eprintln!("{err:#}");
        }
        init.reset();
    }
    run_guest_agent(&test_name)?;
    Ok(())
}
