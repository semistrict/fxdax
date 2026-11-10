Where a one-row SQLite commit's time goes on sqlite_extents, through the image and the daemon (TASK-301).

Machine: c3-standard-8, in us-east4-a.

Built with the extent store's audit, which recounts its state after each critical section (~8% of a one-row commit's CPU, TASK-301): scripts/lib/example-linux-host.sh built the example across the workspace, whose dev-dependencies turned it on, until 70a12ea15; a release build now refuses it (TASK-303).
