#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run fxdax's guest tests (guest/tests) on this host, Linux with KVM (on macOS, use
# scripts/test-e2e-linux.sh, which runs this in Lima). Arguments select the test cases, by name or
# by suite, `--tag SUITE` (the guest tests' runner's `list`, whose --help names the suites;
# default: --tag e2e), which boot fxdax's Firecracker (vendor/firecracker,
# with its fxdax feature), which this builds, and the guest kernel
# FXDAX_GUEST_KERNEL names (scripts/build-guest-kernel.sh builds it). Each test's
# logs are kept under $FXDAX_E2E_RESULTS (default: a new temporary directory).
# FXDAX_E2E_ROUNDS runs each case that many times (default 1), as a soak: it stops
# at a case's first failing round, whose logs it keeps, in ROUND-K under the
# case's, and drops the passing rounds' but for what their guests said, in
# round-K.log there, which measurements repeated round by round need (pgbench's
# tps). Each round's outcome is kept as data, in round-K.json there, and the run's in summary.json
# and run.json beside the cases' (scripts/guest_results.py, which says what each holds), uploaded
# under FXDAX_RESULTS_BUCKET, a prefix of a bucket, if set, as each round ends. Firecracker runs under its production seccomp filter (FXDAX_FIRECRACKER_SECCOMP, real by
# default; log, or none), and FXDAX_FIRECRACKER_JAIL=1 in its jailer's jail, as an unprivileged
# user (see below).
#
# fxdax: FXDAX_E2E_BUILD_ONLY=DIR builds what the cases run, and copies it into DIR, as its
# artifacts: the runner, the guest agent, the extent daemon, fxdaxctl, fxdax-gc and Firecracker;
# it runs no test, and, under a seccomp filter, Firecracker's jailer. FXDAX_E2E_PREBUILT=DIR runs
# the cases with DIR's artifacts, building nothing, nor running the VMM's tests first: so that an
# A/B of two trees (scripts/bench-ab-gce.sh) alternates between their builds, each made once, its
# rounds the cases' time alone (TASK-276), and jobs of a matrix share one build, jailed as
# FXDAX_FIRECRACKER_JAIL says (the nightly's, TASK-284). Neither takes
# FXDAX_FIRECRACKER_SECCOMP=log, whose filter is built per run.
# FXDAX_E2E_ROOT names the tree whose code it builds and boots, and whose helpers it runs (default:
# this script's), so that this script's modes serve a tree of before them, an A/B's base.
#
# The guests' extent stores are served by fxdax's extent daemon, which this
# starts, as FXDAX_EXTENTD tells the tests; its control API is beside it, at
# extentd.control. FXDAX_EXTENTD_MEMORY caps its memory, in bytes. LIBCLANG_PATH names libclang's
# directory where llvm-config cannot, and DAX needs access to /dev/userfaultfd
# (see the README).
set -euo pipefail
root=$(cd "${FXDAX_E2E_ROOT:-$(dirname "$0")/..}" && pwd)
harness="$root/guest/tests"
selectors=("$@")
if (( ${#selectors[@]} == 0 )); then
    selectors=(--tag e2e)
fi
results=${FXDAX_E2E_RESULTS:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-e2e.XXXXXX")}
mkdir -p "$results"
results=$(cd "$results" && pwd)
build_only=${FXDAX_E2E_BUILD_ONLY:-} prebuilt=${FXDAX_E2E_PREBUILT:-}
if [[ -n $build_only && -n $prebuilt ]]; then
    echo "FXDAX_E2E_BUILD_ONLY and FXDAX_E2E_PREBUILT: one or the other" >&2
    exit 2
fi
if [[ -n $build_only$prebuilt && ${FXDAX_FIRECRACKER_SECCOMP:-} == log ]]; then
    echo "prebuilt artifacts take no FXDAX_FIRECRACKER_SECCOMP=log" >&2
    exit 2
fi
if [[ -n $prebuilt && ! -f $prebuilt/artifacts.txt ]]; then
    echo "$prebuilt holds no artifacts (FXDAX_E2E_BUILD_ONLY makes them)" >&2
    exit 2
fi
if [[ -n $prebuilt && -n ${FXDAX_FIRECRACKER_JAIL:-} && ! -x $prebuilt/jailer ]]; then
    echo "$prebuilt holds no jailer, built with no seccomp filter: FXDAX_FIRECRACKER_JAIL needs one" >&2
    exit 2
fi

if [[ $(uname -s) != Linux ]]; then
    echo "fxdax's guests run on Linux only: use scripts/test-e2e-linux.sh" >&2
    exit 2
fi
if [[ -z ${LIBCLANG_PATH:-} ]] && command -v llvm-config > /dev/null; then
    LIBCLANG_PATH=$(llvm-config --libdir)
fi
if [[ -n ${LIBCLANG_PATH:-} ]]; then export LIBCLANG_PATH; fi

# The directory cargo builds into, run in directory $1 as `$2` (default cargo): its workspace's
# target directory, CARGO_TARGET_DIR's if set, as cargo metadata says, not a path guessed here, so
# that a host's target directory of its own (CARGO_TARGET_DIR) is where binaries are taken from
# (TASK-305). Each build below runs in its workspace's directory, as this does, so that cargo
# reads the configuration there.
target_dir() {
    (cd "$1" && "${2:-cargo}" metadata --format-version 1 --no-deps) |
        python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
}

# The pjdfstest (BSD-2-Clause) commit pjdfstest-fxfs-guest's cases build, which the runner and the
# guest agent take as they are built: without it, they skip.
source "$root/scripts/lib/pjdfstest.sh"

# The harness's runner, which boots a VM for each test, and says which cases the arguments select,
# a selection naming no case failing before anything else is built (TASK-160.29).
if [[ -n $prebuilt ]]; then
    runner="$prebuilt/runner"
else
    (cd "$harness" && cargo build -q -p runner)
    harness_target=$(target_dir "$harness")
    runner="$harness_target/debug/runner"
fi
tests=()
if [[ -z $build_only ]]; then
    listed=$("$runner" list "${selectors[@]}")
    mapfile -t tests <<< "$listed"
    rounds=${FXDAX_E2E_ROUNDS:-1}
    [[ $rounds =~ ^[1-9][0-9]*$ ]] || { echo "FXDAX_E2E_ROUNDS is a count" >&2; exit 2; }
    # The run as data, its knobs as it was given them, before this sets any (TASK-160.31).
    "$root/scripts/guest_results.py" start "$results" "$rounds" "${tests[@]}"
fi

# With frame pointers, as every build here has them (.cargo/config.toml), which the bench's perf
# call graphs walk (FXDAX_BENCH_PERF). With FXDAX_FAILPOINTS, a daemon whose failpoints it sets (FAILPOINTS, the fail crate's): requests
# failing or waiting as it says, such as extentd::map=1%return(5) (TASK-44.10); with FXDAX_FAULT_SEED, those of a
# fork's steps its seed says (fxdax::faults); with FXDAX_EXTENTD_FAILPOINTS=1, one built with them,
# none set, for cases that set their own daemons' (fxfs-guest-soak, TASK-67): in the sim profile
# (release with debug assertions), a test's build, as a release build, a measurement's, refuses
# failpoints (extent_remote, TASK-320).
extentd_features=() extentd_profile=release
seccomp=${FXDAX_FIRECRACKER_SECCOMP:-real}
if [[ -n $prebuilt ]]; then
    extentd_bin="$prebuilt/fxdax-extentd"
    export FXDAXCTL_BIN="$prebuilt/fxdaxctl" FXDAX_GC_BIN="$prebuilt/fxdax-gc"
    export FXDAX_FIRECRACKER="$prebuilt/firecracker"
    export FXDAX_GUEST_AGENT="$prebuilt/guest-agent"
    if [[ -n ${FXDAX_FIRECRACKER_JAIL:-} ]]; then
        export FXDAX_FIRECRACKER_JAILER="$prebuilt/jailer"
    fi
fi
if [[ -z $prebuilt && -n ${FXDAX_FAILPOINTS:-}${FXDAX_FAULT_SEED:-}${FXDAX_EXTENTD_FAILPOINTS:-} ]]; then
    extentd_features=(--features failpoints) extentd_profile=sim
fi
# The run's directory, by which a case that goes on from round to round keys what it keeps
# (fxfs-guest-soak).
export FXDAX_E2E_RUN="$results"
if [[ -z $prebuilt ]]; then
(cd "$root" && cargo build --profile "$extentd_profile" -p fxdax --bin fxdax-extentd \
    --bin fxdaxctl ${extentd_features[@]+"${extentd_features[@]}"})
root_target=$(target_dir "$root")
extentd_bin="$root_target/$extentd_profile/fxdax-extentd"
# fxdax: fxdaxctl, beside it, for cases that run it against the daemon (fxfs-guest-stats,
# TASK-111.4); and the bucket's collector, fxdax-gc, for those that collect their bucket
# (fxfs-guest-soak).
export FXDAXCTL_BIN="$root_target/$extentd_profile/fxdaxctl"
(cd "$root" && cargo build --release -p extent_remote --bin fxdax-gc)
export FXDAX_GC_BIN="$root_target/release/fxdax-gc"
# As fxdax builds it, with its fxdax feature (scripts/firecracker-cargo.sh), in the copy of
# vendor/firecracker that builds in, whose .cargo/config.toml sets its target directory, unless
# CARGO_TARGET_DIR does, and build environment. FXDAX_FIRECRACKER_SECCOMP builds Firecracker as production does, for musl, which compiles in its
# filter (TASK-50.1): `real`, the default, runs under that; `log` under a copy of it that allows
# what it would refuse, logging it, which $results/seccomp-log.txt lists afterwards, by thread and
# system call; `none` for the host's target, with no filter of its own, allowing every system
# call. Real by default: copy-on-write DAX's private memory needed three system calls the filter
# lacked, and only jailed runs, which ask for it, would have shown it (TASK-164).
firecracker_cargo="$root/scripts/firecracker-cargo.sh"
firecracker_dir=$("$firecracker_cargo" --tree)
firecracker_target=$(target_dir "$root" "$firecracker_cargo")
case "$seccomp" in
    none)
        "$firecracker_cargo" build --release -p firecracker --features fxdax
        export FXDAX_FIRECRACKER="$firecracker_target/release/firecracker"
        ;;
    real | log)
        musl="$(uname -m)-unknown-linux-musl"
        # musl-gcc's headers lack the kernel's, which userfaultfd-sys includes: the host's, after
        # musl's own (Firecracker's container has them in musl's tree).
        musl_cflags="CFLAGS_${musl//-/_}=-idirafter /usr/include -idirafter /usr/include/$(uname -m)-linux-gnu"
        env "$musl_cflags" "$firecracker_cargo" build --release -p firecracker --features fxdax \
            --target "$musl"
        export FXDAX_FIRECRACKER="$firecracker_target/$musl/release/firecracker"
        # Its jailer, for jailed runs of this build, or of its artifacts.
        if [[ -n $build_only || -n ${FXDAX_FIRECRACKER_JAIL:-} ]]; then
            env "$musl_cflags" "$firecracker_cargo" build --release -p jailer --target "$musl"
            export FXDAX_FIRECRACKER_JAILER="$firecracker_target/$musl/release/jailer"
        fi
        if [[ $seccomp == log ]]; then
            # The filter the build compiled in: upstream's, with fxdax's rules before each thread's
            # own, as its build.rs puts them (resources/fxdax/README.md).
            python3 - "$firecracker_dir/resources/seccomp/$musl.json" \
                "$firecracker_dir/resources/fxdax/seccomp/$musl.json" "$results/seccomp-log.json" <<'PY'
import json, sys
filters = json.load(open(sys.argv[1]))
for thread, rules in json.load(open(sys.argv[2])).items():
    filters[thread]["filter"][:0] = rules["filter"]
for thread in filters.values():
    thread["default_action"] = "log"
json.dump(filters, open(sys.argv[3], "w"), indent=1)
PY
            "$firecracker_cargo" run --release -q -p seccompiler --bin seccompiler-bin -- \
                --target-arch "$(uname -m)" --input-file "$results/seccomp-log.json" \
                --output-file "$results/seccomp-log.bpf"
            export FXDAX_FIRECRACKER_SECCOMP_FILTER="$results/seccomp-log.bpf"
            # Every refusal it logs, not the first few of each burst.
            sysctl -q kernel.printk_ratelimit=0
            dmesg --clear
        fi
        ;;
    *) echo "FXDAX_FIRECRACKER_SECCOMP is real, log or none" >&2; exit 2 ;;
esac
fi
# FXDAX_FIRECRACKER_JAIL=1 runs each Firecracker as production does (TASK-50.1, TASK-50.4): in
# the jail Firecracker's jailer makes, as an unprivileged user (64000), with /dev/userfaultfd in
# the jail and vm.unprivileged_userfaultfd off; under a seccomp filter, for musl.
if [[ -n ${FXDAX_FIRECRACKER_JAIL:-} && -z $build_only ]]; then
    [[ $seccomp != none ]] || { echo "FXDAX_FIRECRACKER_JAIL needs a seccomp filter" >&2; exit 2; }
    export FXDAX_JAIL_UID=64000 FXDAX_JAIL_GID=64000
    sysctl -q vm.unprivileged_userfaultfd=0
    # What the tests make, images and all, the jailed user reaches; but the daemons' sockets,
    # their user's alone (TASK-155), which it is given once they serve.
    umask 000
fi
# FXDAX_MAX_MAP_COUNT sets vm.max_map_count, of which a VM process's DAX window keeps half its
# runs of pages installed (dax::uffd's budget), leaving out those past it (TASK-91.16).
if [[ -n ${FXDAX_MAX_MAP_COUNT:-} ]]; then
    sysctl -q vm.max_map_count="$FXDAX_MAX_MAP_COUNT"
    echo "vm.max_map_count: $(sysctl -n vm.max_map_count)"
fi
# FXDAX_GUEST_HUGE_PAGES=2M backs each VM's memory with the host's 2 MiB huge pages, as many
# reserved as FXDAX_GUEST_HUGE_PAGES_MIB (default 16384) take (TASK-91.16): the hypervisor's
# second-stage mappings of guest memory then 2 MiB each, what refilling the guest's TLB after a
# flush walks the fewer.
if [[ ${FXDAX_GUEST_HUGE_PAGES:-} == 2M ]]; then
    sysctl -q vm.nr_hugepages=$(( ${FXDAX_GUEST_HUGE_PAGES_MIB:-16384} / 2 ))
    echo "huge pages: $(sysctl -n vm.nr_hugepages) of 2 MiB reserved"
elif [[ -n ${FXDAX_GUEST_HUGE_PAGES:-} ]]; then
    echo "FXDAX_GUEST_HUGE_PAGES is 2M or unset" >&2; exit 2
fi
# fxfs-guest-docker-dax's Docker, its static binaries (Apache-2.0), downloaded once, and BusyBox,
# its containers' image, the host's static one (busybox-static); fxfs-guest-docker-build-dax's
# too, and Docker's buildx plugin (Apache-2.0), which builds with BuildKit (TASK-311); and
# fxfs-guest-sandboxes', whose builders build so (TASK-330.1).
builds=" fxfs-guest-docker-build-dax fxfs-guest-sandboxes "
selected() { # CASE...: whether the run selects one of the CASEs
    local case
    for case in "$@"; do
        [[ " ${tests[*]} " != *" $case "* ]] || return 0
    done
    return 1
}
# shellcheck disable=SC2086 # the cases, a word each
if [[ -z ${FXDAX_DOCKER_STATIC:-} ]] && selected fxfs-guest-docker-dax $builds; then
    docker_version=29.9.1
    case $(uname -m) in
        x86_64) docker_sha256=c158ecc55adb3afb01bbd3539ed3b997d20c26dbdc418080506cc1bf5243ff57 ;;
        aarch64) docker_sha256=3e93cc6d1a686456a3b18eefdde26467a837a7a2b257af22e27cba1dbc1b2018 ;;
    esac
    docker_dir="${XDG_CACHE_HOME:-$HOME/.cache}/fxdax/docker-$docker_version"
    if [[ ! -x $docker_dir/dockerd ]]; then
        mkdir -p "$docker_dir"
        curl -fsSL -o "$docker_dir.tgz" \
            "https://download.docker.com/linux/static/stable/$(uname -m)/docker-$docker_version.tgz"
        echo "$docker_sha256  $docker_dir.tgz" | sha256sum --check --quiet
        tar -xzf "$docker_dir.tgz" -C "$docker_dir" --strip-components=1
        rm "$docker_dir.tgz"
    fi
    export FXDAX_DOCKER_STATIC="$docker_dir" FXDAX_BUSYBOX="${FXDAX_BUSYBOX:-/bin/busybox}"
fi
# shellcheck disable=SC2086 # the cases, a word each
if [[ -z ${FXDAX_DOCKER_BUILDX:-} ]] && selected $builds; then
    buildx_version=0.38.2
    case $(uname -m) in
        x86_64) buildx_arch=amd64 buildx_sha256=7992ec09413b80ff57ba4e081f56fc833d7bac1d5e71143c6c809e7e6065f6d6 ;;
        aarch64) buildx_arch=arm64 buildx_sha256=46c0f334772de3cb0b341cc415caf7339fb5cfc8b8438ebcb5a10e7f23800d36 ;;
    esac
    buildx="${XDG_CACHE_HOME:-$HOME/.cache}/fxdax/docker-buildx-$buildx_version"
    if [[ ! -x $buildx ]]; then
        mkdir -p "$(dirname "$buildx")"
        curl -fsSL -o "$buildx.download" "https://github.com/docker/buildx/releases/download/v$buildx_version/buildx-v$buildx_version.linux-$buildx_arch"
        echo "$buildx_sha256  $buildx.download" | sha256sum --check --quiet
        chmod +x "$buildx.download"
        mv "$buildx.download" "$buildx"
    fi
    export FXDAX_DOCKER_BUILDX="$buildx"
fi
# The harness's guest agent, a static binary each guest runs as its init. pjdfstest's cases take
# its repository and commit as they build.
guest_target="$(uname -m)-unknown-linux-musl"
if [[ -z $build_only$prebuilt ]]; then
    # The harness's own tests, scripts/test.sh's guest part, which as another user than root says
    # it leaves out those that need root (TASK-307); and the VMM's extent device's tests, and its
    # vCPUs' exits', fxdax's, which no other suite reaches (TASK-79.102, TASK-187): its firecracker
    # part, which CI runs too (TASK-220).
    FXDAX_TEST_PARTS="guest firecracker" "$root/scripts/test.sh"
fi
if [[ -z $prebuilt ]]; then
    (cd "$harness" && cargo build -q --target "$guest_target" -p guest-agent)
    export FXDAX_GUEST_AGENT="$harness_target/$guest_target/debug/guest-agent"
fi
# fxdax: the artifacts, copied for runs to take as they are (FXDAX_E2E_PREBUILT), and what they
# were built from and as; and nothing run.
if [[ -n $build_only ]]; then
    mkdir -p "$build_only"
    artifacts=(runner guest-agent fxdax-extentd fxdaxctl fxdax-gc firecracker)
    cp "$runner" "$FXDAX_GUEST_AGENT" "$extentd_bin" "$FXDAXCTL_BIN" "$FXDAX_GC_BIN" \
        "$FXDAX_FIRECRACKER" "$build_only/"
    if [[ -n ${FXDAX_FIRECRACKER_JAILER:-} ]]; then
        cp "$FXDAX_FIRECRACKER_JAILER" "$build_only/"
        artifacts+=(jailer)
    fi
    {
        echo "source $(cat "$root/source-revision.txt" 2> /dev/null || echo unknown)"
        echo "seccomp $seccomp"
        (cd "$build_only" && sha256sum "${artifacts[@]}")
    } > "$build_only/artifacts.txt"
    echo "Artifacts: $build_only" >&2
    exit 0
fi
socket="$results/extentd.sock"
control="$results/extentd.control"
if [[ -e $socket || -e $control ]]; then
    echo "$socket exists: a daemon may be running" >&2
    exit 2
fi
# A host keeps stores in a bucket, FXDAX_EXTENTD_REMOTE's or a file bucket in memory, on tmpfs,
# as of placement 1 unless one is named; what it did with it scripts/bench-report.py reports. Not
# one in the daemon's memory: the cases open the stores it made in processes of their own (fxdax's
# Storage), which keep a store's branch in the bucket its description names (TASK-165.6). Its
# kernel takes FXDAX_EXTENTD_MEMORY bytes at most, if set, or 8 GiB, the sandbox team's daemon's
# (guest/tests' common::DAEMON_MEMORY): its own default, 1 GiB, held too little of what VMs read
# again (TASK-227).
# fxdax: what its stores read from the bucket it keeps in a part cache of its own on disk, as
# production's daemon does (FXDAX_EXTENTD_CACHE, doc-7), unless FXDAX_EXTENTD_CACHE says
# otherwise, empty for none; not this process's, which the cases' own daemons would share
# (TASK-164.12).
bucket_dir=
if [[ -z ${FXDAX_EXTENTD_REMOTE:-} ]]; then
    bucket_dir=$(mktemp -d /dev/shm/fxdax-e2e-bucket.XXXXXX)
fi
cache_dir=
if [[ -z ${FXDAX_EXTENTD_CACHE+set} ]]; then
    cache_dir=$(mktemp -d "${TMPDIR:-/tmp}/fxdax-e2e-cache.XXXXXX")
fi
# fxdax: FXDAX_EXTENTD_MEMORY_MAX runs the daemon in a systemd scope of its own, limited to that
# many bytes of memory (systemd's MemoryMax, no swap), its cgroup killing it past them, and keeps
# the most it held, memory.peak, in extentd-memory-peak.txt as the run ends: what the daemon's
# memory budget is measured against (TASK-317). FXDAX_EXTENTD_WRAP, a command and its arguments,
# runs the daemon under it, such as a heap profiler (`heaptrack -o DIR/heap`).
extentd_scope=
extentd_run=()
if [[ -n ${FXDAX_EXTENTD_MEMORY_MAX:-} ]]; then
    extentd_scope="fxdax-e2e-extentd-$$.scope"
    extentd_run=(systemd-run --scope --quiet --collect --unit="$extentd_scope"
        -p MemoryMax="$FXDAX_EXTENTD_MEMORY_MAX" -p MemorySwapMax=0 --)
fi
read -r -a extentd_wrap <<< "${FXDAX_EXTENTD_WRAP:-}"
# The most the daemon's scope held, kept before the daemon goes, whose scope goes with it; and the
# scope stopped, its every process with it, as a wrapper (heaptrack's script) may outlive the kill.
extentd_peak() {
    [[ -n $extentd_scope ]] || return 0
    systemctl show -P MemoryPeak "$extentd_scope" > "$results/extentd-memory-peak.txt" || true
}
extentd_stop() {
    [[ -n $extentd_scope ]] || return 0
    systemctl stop "$extentd_scope" 2> /dev/null || true
}
${extentd_run[@]+"${extentd_run[@]}"} \
env FXDAX_EXTENTD_REMOTE="${FXDAX_EXTENTD_REMOTE:-file://$bucket_dir}" \
    FXDAX_EXTENTD_CACHE="${FXDAX_EXTENTD_CACHE-$cache_dir}" \
    FXDAX_EXTENTD_TOKEN="${FXDAX_EXTENTD_TOKEN:-1}" \
    FXDAX_EXTENTD_STATS="$results/extentd-stats.txt" FXDAX_EXTENTD_GRPC="$control" \
    FAILPOINTS="${FXDAX_FAILPOINTS:-}" ${extentd_wrap[@]+"${extentd_wrap[@]}"} "$extentd_bin" "$socket" "${FXDAX_EXTENTD_MEMORY:-8589934592}" \
    > "$results/extentd.log" 2>&1 &
daemon=$!
trap 'extentd_peak; kill "$daemon" 2> /dev/null; extentd_stop; rm -f "$socket" "$control"; [[ -z $bucket_dir ]] || rm -r "$bucket_dir"; [[ -z $cache_dir ]] || rm -r "$cache_dir"' EXIT
while [[ ! -S $socket || ! -S $control ]]; do
    if ! kill -0 "$daemon" 2> /dev/null; then
        echo "the extent daemon exited: see $results/extentd.log" >&2
        exit 1
    fi
    sleep 0.1
done
# The jailed Firecrackers use the default tenant's socket, as production's use a tenant's.
if [[ -n ${FXDAX_FIRECRACKER_JAIL:-} ]]; then
    chown "$FXDAX_JAIL_UID:$FXDAX_JAIL_GID" "$socket"
fi
export FXDAX_EXTENTD="$socket"
# fxdax: the daemon's binary, for cases that start daemons of their own.
export FXDAX_EXTENTD_BIN="$extentd_bin"

status=0
for test in "${tests[@]}"; do
    for (( round = 1; round <= rounds; round++ )); do
        # A single round's logs in the case's directory, where scripts/pgbench-gate.py reads them;
        # each of several rounds' in one of its own.
        base="$results/$test"
        (( rounds == 1 )) || base+="/round-$round"
        passed=1
        "$runner" test --test-case "$test" --base-dir "$base" --keep-all \
            --outcome "$results/$test/round-$round.json" --run "$results/run.json" \
            --round "$round" || passed=0
        if (( passed && rounds > 1 )); then
            find "$base" -name log.txt -exec cp {} "$results/$test/round-$round.log" \;
            rm -r "$base"
        fi
        "$root/scripts/guest_results.py" ended "$results" "$test"
        if (( ! passed )); then
            echo "$test: failed in round $round of $rounds: see $base" >&2
            status=1
            break
        fi
    done
    if (( passed && rounds > 1 )); then
        echo "$test: $rounds rounds passed" >&2
    fi
done
if [[ $seccomp == log ]]; then
    # The audit records of the system calls the filter would refuse: thread, call, count.
    dmesg | python3 -c '
import collections, re, sys
seen = collections.Counter()
for line in sys.stdin:
    if "type=1326" not in line:
        continue
    # A name with a space, as vCPU threads have, is hexadecimal, unquoted.
    comm = re.search(r"comm=(?:\"([^\"]*)\"|([0-9A-F]+)\b)", line)
    call = re.search(r"syscall=(\d+)", line)
    if comm and call:
        name = comm.group(1) or bytes.fromhex(comm.group(2)).decode(errors="replace")
        seen[(name, int(call.group(1)))] += 1
for (comm, call), count in sorted(seen.items()):
    print(f"{comm}\t{call}\t{count}")
' > "$results/seccomp-log.txt"
    echo "seccomp: $(wc -l < "$results/seccomp-log.txt") thread and call pairs refused" >&2
fi
# fxdax: the guest stores the DAX windows refused: the guest and the window disagreeing, which fails
# the run (TASK-44.12). Not a kernel worker's, the VM processes' logs saying whose (TASK-150.16).
mapfile -t firecracker_logs < <(find "$results" -name firecracker.log)
if ! "$root/scripts/dax-refusals.py" "$results/extentd.log" "${firecracker_logs[@]}" \
    > "$results/unexplained-refusals.txt"; then
    echo "the DAX window refused a guest's stores: see $results/unexplained-refusals.txt" >&2
    status=1
fi
# fxdax: the bucket's requests that kept failing, as the daemon logged them (TASK-314): each is
# retried until it is answered, holding its guests' commits at the lag bound meanwhile. The run's
# own file bucket, on tmpfs, fails only once that is full, its cases then hanging until the runner
# times them out: the run fails, saying so.
grep -E "object storage: [a-z-]+ requests (failing|still failing)" "$results/extentd.log" \
    > "$results/bucket-failures.txt" || true
if [[ -s $results/bucket-failures.txt ]]; then
    echo "the bucket failed requests, each retried: see $results/bucket-failures.txt; the first:" >&2
    sed -n 1p "$results/bucket-failures.txt" >&2
    if [[ -n $bucket_dir ]]; then
        df -h "$bucket_dir" >&2 || true
        status=1
    fi
else
    rm "$results/bucket-failures.txt"
fi
# fxdax: what each extent daemon cost, against its design's budgets (TASK-330.2): its anonymous
# memory, its uploads' memory, and its disk's writes, each what it says it wrote; and what it wrote
# for each guest byte, by kind. The run's own daemon's bucket, a file bucket on tmpfs, writes no
# disk.
budgets=()
[[ -z $bucket_dir ]] || budgets=(--bucket-in-memory "$results/extentd-stats.txt")
if ! "$root/scripts/budgets.py" ${budgets[@]+"${budgets[@]}"} "$results"; then
    echo "an extent daemon past its budget: see $results/budgets.json" >&2
    status=1
fi
echo "Logs: $results" >&2
"$root/scripts/guest_results.py" finish "$results" "$status"
exit "$status"
