#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# PostgreSQL on this host itself, no VM, as fxfs-guest-pgbench runs it in its VMs
# (test_fxfs_guest_pgbench.rs): the baseline the guest's pgbench is measured
# against. In a postgres:17 container, data on this host's disk, held to the
# guest's 2 CPUs and 4 GiB unless FXDAX_PGBENCH_NATIVE_UNCAPPED is set; then
# pgbench -i -q -s 50, and five 60 s runs of -c 4 -j 2, its client in the
# container too, as the guest's is in its VM. Prints the load's seconds and
# each run's tps.
#
#   scripts/pgbench-native.sh [data-directory]

set -euo pipefail

data=${1:-/var/lib/fxdax-test/pgbench-native}
name=fxdax-pgbench-native
caps=(--cpus 2 --memory 4g)
[[ -n ${FXDAX_PGBENCH_NATIVE_UNCAPPED:-} ]] && caps=()

sudo podman rm -f "$name" > /dev/null 2>&1 || true
sudo rm -rf "$data"
sudo mkdir -p "$data"
sudo podman run -d --name "$name" "${caps[@]}" -e POSTGRES_HOST_AUTH_METHOD=trust \
    -v "$data:/var/lib/postgresql/data" docker.io/library/postgres:17 \
    -c shared_buffers=256MB > /dev/null
trap 'sudo podman rm -f "$name" > /dev/null 2>&1 || true' EXIT
# Over TCP: the image initializes the data on a server of the socket alone, then restarts it.
until sudo podman exec "$name" pg_isready -h 127.0.0.1 -U postgres -q; do sleep 1; done
in_pg() { sudo podman exec "$name" "$@"; }
echo "postgres: $(in_pg postgres --version) caps: ${caps[*]:-none}"
in_pg createdb -U postgres bench
start=$(date +%s.%N)
in_pg pgbench -U postgres -i -q -s 50 bench > /dev/null 2>&1
echo "load: $(awk -v start="$start" -v end="$(date +%s.%N)" 'BEGIN { printf "%.2f", end - start }') s"
for run in 1 2 3 4 5; do
    tps=$(in_pg pgbench -U postgres -c 4 -j 2 -T 60 bench 2>/dev/null |
        sed -n 's/^tps = \([0-9.]*\).*/\1/p')
    echo "run $run: $tps tps"
done
