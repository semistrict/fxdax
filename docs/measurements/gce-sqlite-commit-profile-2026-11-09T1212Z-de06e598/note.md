One-row commits on sqlite_extents: as gce-sqlite-commit-profile-2026-11-09T1208Z, and logs keep their pages cached, and chunks a commit wrote are read from memory until the window maps them (TASK-301).

Machine: c3-standard-8, in us-east4-a.

Built with the extent store's audit, which recounts its state after each critical section (~8% of a one-row commit's CPU, TASK-301): scripts/lib/example-linux-host.sh built the example across the workspace, whose dev-dependencies turned it on, until 70a12ea15; a release build now refuses it (TASK-303).

Built also with fxdax's failpoints (its dev-dependency on itself, bee7356a6, 2026-11-01), each `fail_point!` a lookup in the fail crate's registry, and extent_store's `testing` (3afa08d64, 2026-11-08), each lock shared beyond one store taking a process-wide one too: the workspace's dev-dependencies turned them on. About 1-3% of a small request's time on extentd-mapbench (ubu1-mapbench-failpoints-2026-11-10); a release build now refuses them (TASK-320).
