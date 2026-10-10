Smoke run, 1 iteration, before fixes: the VMMs logged every request at trace level (libkrun let
`RUST_LOG=trace` override the benchmark's level), fio dropped each job's cache (so warm reads were
cold), 512 MiB data on a Fedora root filesystem. Only the comparison survives. Not comparable.
