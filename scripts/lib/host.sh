# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Runs fxdax's host scripts (scripts/lib/*-linux-host.sh) on a Linux host of one's own, over ssh,
# for the drivers that source this: scripts/test-host.sh, bench-host.sh, bench-ab-host.sh,
# gets-host.sh, mutants-host.sh, fork-time-host.sh and migrate-host.sh; and through test-host.sh,
# perf-gate-host.sh, soak-host.sh, soak-workloads-host.sh, guest-mutants-host.py and dax-gate.sh.
#
# The host, FXDAX_HOST as ssh names it (default ubu1), is an x86-64 Linux host with KVM, set up
# once by its own setup, not by a run (ubu1's ~/docs/system-setup.d: 140-fxdax_dev.sh's packages,
# those scripts/lib/linux-host-setup.sh lists, and the user's Rust, in ~/.cargo, with
# cargo-nextest; 141's sudo without a password; 143's ext4 scratch disk, /mnt/fxdax-scratch; 146's
# fxjob; 147's cargo-mutants, perf and bpftrace), and whose clone of fxdax, FXDAX_HOST_REPO (a path
# in its home, default src/fxdax), fetches from fxdax's origin. Others' runs share it.
#
# A run is a directory on the host, RUN under FXDAX_HOST_SCRATCH (default
# /mnt/fxdax-scratch/fxdax-host), RUN the run's name (host_run): a worktree of the commit run, which
# origin has (host_commit), its vendor/firecracker checked out; the results; and the work
# directory; the host script builds in the worktree, its target directories its own
# (scripts/lib/host-remote.sh, which does what a run does there). Its job, the host script
# (host_command), runs apart from the SSH session, under fxjob, in a cgroup of its own and within
# FXDAX_HOST_MEM (default 12G), and with fxjob --bench, alone and on CPUs of its own, if it is a
# measurement (host_bench=1); as root, as its tests need, with the user's Rust; its builds, and
# nextest's tests, at most FXDAX_HOST_JOBS at once (default 4), a non-measurement's at niceness
# FXDAX_HOST_NICE (default 19); its guest kernels taken from the host's cache of those built from
# the same sources (host_kernel, scripts/lib/linux-host-setup.sh); and nothing of it left running
# once it ends, root's processes too (host-remote.sh's reap). The driver waits for the job, reconnecting,
# fetches the results, and removes the run's directory, its worktrees with it, whatever happens:
# but for a job whose driver stopped waiting for it, which runs on, for the driver's `resume` to
# wait for again and fetch, or its `cancel` to end (host_actions).
#
# The sourcing script sets repo, the tree whose scripts run, before it sources this, which names
# the run; then results, the local directory the results come back to, and limit, the longest the
# job may take (timeout's DURATION; FXDAX_HOST_LIMIT, if set, in its place), and calls host_start,
# host_job and host_fetch. FXDAX_HOST_RUN is the run's number, as its caller counts runs (such as
# r1040), which its name carries. FXDAX_HOST_COMMIT names the commit run (default HEAD, of a tree
# that changes nothing it tracks), and FXDAX_HOST_PATCH a patch the run applies to it (as
# scripts/guest-mutants-host.py's mutants). FXDAX_GUEST_KERNEL names a guest kernel here, which the
# run copies to the host and boots.
#
# The drivers run these functions under `||`, where errexit stops nothing, so each checks the
# status of its every step itself.

host=${FXDAX_HOST:-ubu1}
[[ $host =~ ^[A-Za-z0-9._@-]+$ ]] || { echo "FXDAX_HOST names a host, as ssh does: $host" >&2; exit 2; }
host_repo=${FXDAX_HOST_REPO:-src/fxdax}
host_scratch=${FXDAX_HOST_SCRATCH:-/mnt/fxdax-scratch/fxdax-host}
host_library=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# This run's mark, which its name and the names of its results end in.
host_mark=$(od -An -N4 -tx1 /dev/urandom | tr -d ' \n')
[[ ${FXDAX_HOST_RUN:-} =~ ^[a-z0-9]*$ ]] ||
    { echo "FXDAX_HOST_RUN names a run in lower-case letters and digits, such as r1040" >&2; exit 2; }
# What a run's name looks like, for those given one (host_actions).
host_run_pattern='^run-([a-z0-9]+-)?[0-9]{8}T[0-9]{6}Z-[0-9a-f]{8}$'
# Whether the run is a measurement, which fxjob --bench runs alone: a timed driver's always, any
# other's with FXDAX_HOST_BENCH=1 (as scripts/perf-gate-host.sh's runs of test-host.sh).
host_bench=${FXDAX_HOST_BENCH:-0}
[[ $host_bench =~ ^[01]$ ]] || { echo "FXDAX_HOST_BENCH is 0 or 1" >&2; exit 2; }
# Where the run stands: '' (nothing of it on the host), prepared (its directory made), launched
# (its job running, as far as this knows) or ended.
host_state=''
# A function the driver names, which host_actions' resume runs as the driver's run would once the
# results are back, with the run's status, and whose status is the resume's.
host_after=''
host_ssh_options=(-o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=60
    -o ServerAliveCountMax=10)

# host_name RUN names the run RUN: its directory on the host is host_dir.
host_name() {
    host_run=$1
    # shellcheck disable=SC2034 # The drivers read it.
    host_dir=$host_scratch/$host_run
}
# The run's name: FXDAX_HOST_RUN, if set; when it started, to the second; and this driver's mark,
# so that two drivers started in the same second keep apart (TASK-79.86).
host_name "run-${FXDAX_HOST_RUN:+$FXDAX_HOST_RUN-}$(date -u +%Y%m%dT%H%M%SZ)-$host_mark"

# host_label prints the host's name for the names of results: ssh's, without its user.
host_label() {
    echo "${host##*@}"
}

# host_ssh COMMAND... runs COMMAND on the host. Keepalives, as a command may run for hours
# without output.
host_ssh() {
    ssh "${host_ssh_options[@]}" "$host" "$*" < /dev/null
}

# host_side ACTION ARG... does ACTION of scripts/lib/host-remote.sh on the host.
host_side() {
    ssh "${host_ssh_options[@]}" "$host" "bash -s -- $(printf '%q ' "$@")" \
        < "$host_library/host-remote.sh"
}

# host_put FILE PATH copies FILE here to PATH on the host.
host_put() {
    ssh "${host_ssh_options[@]}" "$host" \
        "mkdir -p $(printf %q "$(dirname "$2")") && cat > $(printf %q "$2")" < "$1"
}

# host_commit [REV] prints the commit REV names in repo, in full; with no REV, FXDAX_HOST_COMMIT's,
# or HEAD's, of a tree that changes nothing it tracks, as what it changes would not run.
# shellcheck disable=SC2154 # The sourcing script sets repo.
host_commit() {
    local rev=${1:-${FXDAX_HOST_COMMIT:-}}
    if [[ -z $rev ]]; then
        if [[ -n $(git -C "$repo" status --porcelain --untracked-files=no) ]]; then
            echo "$repo changes what it tracks, which would not run: commit and push it, or name" \
                "a commit with FXDAX_HOST_COMMIT." >&2
            return 2
        fi
        rev=HEAD
    fi
    git -C "$repo" rev-parse --verify --quiet "$rev^{commit}" ||
        { echo "$rev: no commit of $repo's" >&2; return 2; }
}

# host_start [LABEL=REV...] makes the run's directory on the host (host-remote.sh's prepare): the
# worktree of its commit (host_commit), FXDAX_HOST_PATCH applied to it, and one of each REV, which
# the host script finds in host_dir/trees/LABEL; and records each in the results, source.txt, and
# LABEL/source.txt, with the run, the host and the directory there (run.txt). From here, the run's
# directory goes as the driver ends, whatever happens (host_exit).
host_start() {
    local main pairs=() pair sha label firecracker prepared out kernel patch
    main=$(host_commit) || return
    for pair in "$@"; do
        [[ $pair =~ ^[a-z0-9][a-z0-9-]*=.+$ ]] ||
            { echo "$pair: LABEL=COMMIT, LABEL in lower-case letters, digits and -" >&2; return 2; }
        sha=$(host_commit "${pair#*=}") || return
        pairs+=("${pair%%=*}=$sha")
    done
    mkdir -p "$results" || return
    results=$(cd "$results" && pwd) || return
    host_state=prepared
    trap host_exit EXIT
    if [[ -n ${FXDAX_HOST_PATCH:-} ]]; then
        host_put "$FXDAX_HOST_PATCH" "$host_dir/patch" || return
        patch=$(shasum -a 256 < "$FXDAX_HOST_PATCH") || return
    fi
    if [[ -n ${FXDAX_GUEST_KERNEL:-} ]]; then
        host_put "$FXDAX_GUEST_KERNEL" "$host_dir/guest-kernel" || return
        kernel=$(shasum -a 256 < "$FXDAX_GUEST_KERNEL") || return
    fi
    prepared=$(host_side prepare "$host_dir" "$host_repo" "$main" ${pairs[@]+"${pairs[@]}"}) ||
        return
    printf 'run %s\nhost %s\ndirectory %s\n' "$host_run" "$host" "$host_dir" > "$results/run.txt" ||
        return
    while read -r label sha firecracker; do
        out=$results
        if [[ $label != . ]]; then out=$results/$label; fi
        mkdir -p "$out" || return
        {
            echo "revision $sha"
            echo "firecracker $firecracker"
            if [[ $label == . && -n ${patch:-} ]]; then
                echo "patch ${patch%% *} $(basename "$FXDAX_HOST_PATCH")"
            fi
            if [[ $label == . && -n ${kernel:-} ]]; then echo "guest kernel ${kernel%% *}"; fi
        } > "$out/source.txt" || return
    done <<< "$prepared"
}

# host_command [NAME=VALUE...] SCRIPT [ARG...] prints the job's command, which runs the host script
# scripts/lib/SCRIPT of the run's worktree, as root, with the user's Rust, for at most $limit (or
# FXDAX_HOST_LIMIT), with the worktree, the results and the ARGs.
#
# The script runs in this machine's environment: every FXDAX_* and PJDFSTEST_* variable set here,
# but those that stay here (host_env_local); the run's own, its work directory, the host's cache of
# guest kernels and, with FXDAX_GUEST_KERNEL set, the copy host_start made; and each NAME=VALUE in
# place of the variable NAME. A value starting with ~/ names a path in the host's home. An empty
# value sends nothing, leaving NAME unset there, which its readers take for their default (an
# empty FXDAX_ZSTD_LEVEL is an error).
host_command() {
    local jobs=${FXDAX_HOST_JOBS:-4} nice=${FXDAX_HOST_NICE:-19} given=() assignments=() name value
    local i j
    [[ $jobs =~ ^[1-9][0-9]*$ ]] || { echo "FXDAX_HOST_JOBS is a count" >&2; return 2; }
    [[ $nice =~ ^[0-9]+$ ]] || { echo "FXDAX_HOST_NICE is a niceness" >&2; return 2; }
    # A measurement's builds at the niceness its own script chooses (bench-ab-linux-host.sh's).
    if (( host_bench )); then nice=0; fi
    given=("FXDAX_GUEST_KERNEL_JOBS=$jobs")
    for name in ${!FXDAX_@} ${!PJDFSTEST_@}; do
        if ! host_env_local "$name"; then
            given+=("$name=${!name}")
        fi
    done
    given+=(FXDAX_HOST_PROVISIONED=1 FXDAX_HOST_IN_PLACE=1 "FXDAX_HOST_WORK=$host_dir/work"
        "FXDAX_HOST_KERNELS=$host_scratch/kernels")
    if [[ -n ${FXDAX_GUEST_KERNEL:-} ]]; then
        given+=("FXDAX_GUEST_KERNEL=$host_dir/guest-kernel")
    fi
    while (( $# > 0 )) && [[ $1 =~ ^[A-Za-z_][A-Za-z0-9_]*= ]]; do
        given+=("$1")
        shift
    done
    local script=${1:?a host script required}
    shift
    for (( i = 0; i < ${#given[@]}; i++ )); do
        name=${given[i]%%=*} value=${given[i]#*=}
        # A later value of the name replaces this one.
        for (( j = i + 1; j < ${#given[@]}; j++ )); do
            if [[ ${given[j]%%=*} == "$name" ]]; then continue 2; fi
        done
        # shellcheck disable=SC2088,SC2016 # ~/ as it is, and $HOME for the host's shell to expand.
        if [[ -z $value ]]; then
            continue
        elif [[ $value == "~/"* ]]; then
            assignments+=("$(printf '%s="$HOME"/%q' "$name" "${value#"~/"}")")
        else
            assignments+=("$(printf '%s=%q' "$name" "$value")")
        fi
    done
    if (( nice )); then printf 'nice -n %d ' "$nice"; fi
    # shellcheck disable=SC2016 # For the host's shell to expand.
    # Root's git, in the user's worktrees, which it would refuse as another's (safe.directory).
    printf '%s' 'sudo env PATH="$HOME/.cargo/bin:$PATH" HOME="$HOME"' " CARGO_BUILD_JOBS=$jobs" \
        " NEXTEST_TEST_THREADS=$jobs GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory" \
        " GIT_CONFIG_VALUE_0='*'" \
        ${assignments[@]+"${assignments[@]/#/ }"}
    printf ' %q' timeout --signal=TERM --kill-after=30s "${FXDAX_HOST_LIMIT:-$limit}" bash \
        "$host_dir/tree/scripts/lib/$script" "$host_dir/tree" "$host_dir/results" "$@"
    echo
}

# host_env_local NAME succeeds for the variables that stay here, though host_command would send
# them by their names: those this machine sets up its own builds, daemons and runs with, and which
# the host scripts, or what they run, would take as they are. Set here, they name what this machine
# built or fetched, its own daemon's sockets, address and directories, and where its own runs
# write; sent, every build, test and daemon on the host would use them in place of its own. Those
# the host scripts always set themselves, such as FXDAX_FIRECRACKER and FXDAX_EXTENTD
# (scripts/test-e2e.sh), need no place here. The others go: knobs a driver's caller sets for the
# host, what runs and how, and where its stores keep their branches and its daemons what they read
# from them (FXDAX_EXTENTD_REMOTE, FXDAX_S3_URL, FXDAX_EXTENTD_CACHE), or a driver's own, which
# nothing there reads.
host_env_local() {
    case $1 in
        # Docker's static binaries and BusyBox, for fxfs-guest-docker-dax:
        # scripts/test-e2e.sh fetches its own unless they are set.
        FXDAX_DOCKER_STATIC | FXDAX_BUSYBOX) ;;
        # Firecracker's seccomp filter and jailer: scripts/test-e2e.sh builds
        # them in some of its modes alone, and the guest tests use them
        # whenever they are set.
        FXDAX_FIRECRACKER_SECCOMP_FILTER | FXDAX_FIRECRACKER_JAILER) ;;
        # A daemon's control socket, statistics file and metrics address:
        # every extent daemon there reads them, those the tests start too, and
        # all of them would serve at the one socket and address, and write the
        # one file.
        FXDAX_EXTENTD_GRPC | FXDAX_EXTENTD_STATS | FXDAX_EXTENTD_METRICS) ;;
        # The directories a daemon serves stores in and puts its tenants'
        # sockets in: every extent daemon there would take them, those the
        # tests start too, whose stores are in directories of their own, and
        # with none of these roots there, a daemon refuses every store.
        FXDAX_EXTENTD_ROOTS | FXDAX_EXTENTD_TENANTS) ;;
        # Where the benchmark's cases write their measurements, wherever they
        # run; bench-linux-host.sh alone sets it.
        FXDAX_BENCH_RESULTS) ;;
        *) return 1 ;;
    esac
}

# host_job [NAME=VALUE...] SCRIPT [ARG...] runs the host script SCRIPT as the run's job
# (host_command), and waits for it (host_wait): its log, once it ends, is its output, and its exit
# status is this function's. A job whose launching SSH failed is waited for only if a fresh
# connection finds it launched, its pid kept: that SSH may drop once the job runs, as GCE r942's
# did (TASK-235); one that didn't start returns that SSH's status.
host_job() {
    local command launched=0
    command=$(host_command "$@") || return
    host_put <(printf '%s\n' "$command") "$host_dir/job/command" || return
    host_state=launched
    host_side launch "$host_dir" "$host_run" "$host_bench" "${FXDAX_HOST_MEM:-12G}" || launched=$?
    if (( launched )) && ! host_ssh "test -e $(printf %q "$host_dir/job/pid")"; then
        host_state=prepared
        return "$launched"
    fi
    host_wait
}

# host_wait waits for the run's job, as host_job does, for one whose driver stopped waiting for it
# too: reconnecting as a connection drops, and giving up after 20 tries none of which connected,
# the job running on. A job whose shell is gone without its status, as one is when its host
# restarted under it, is lost.
host_wait() {
    local said tries=0
    while true; do
        said=$(host_side wait "$host_dir" 2> /dev/null) || true
        case ${said##*$'\n'} in
            "status "*)
                host_state=ended
                host_side log "$host_dir" || echo "fetching $host_run's log failed" >&2
                return "${said##*status }"
                ;;
            lost)
                host_state=ended
                echo "$host_run on $host is lost: its job ended without its status." >&2
                host_side log "$host_dir" || echo "fetching $host_run's log failed" >&2
                return 1
                ;;
            none)
                host_state=prepared
                echo "$host_run on $host has no job." >&2
                return 1
                ;;
        esac
        # Not connected, or the connection dropped: again, but not without end.
        if [[ $said == waiting* ]]; then tries=0; fi
        tries=$((tries + 1))
        if (( tries >= 20 )); then
            echo "$host does not answer: $host_run's job may run on." >&2
            return 1
        fi
        sleep 30
    done
}

# host_fetch copies the run's results on the host into the results, as one archive (host-remote.sh's
# fetch). A tar that fails there may still have written the archive of every other file (1: a file
# changed as it read it, as the logs of a daemon a killed host script left running do; 2: it could
# not read one): so the archive is unpacked however it ended, and the first failure is this
# function's.
host_fetch() {
    local statuses
    host_side fetch "$host_dir" 2>> "$results/remote.log" | tar -xzf - -C "$results"
    statuses=("${PIPESTATUS[@]}")
    return $(( statuses[0] ? statuses[0] : statuses[1] ))
}

# host_end removes the run's directory from the host, its worktrees with it, once the run has one:
# but for a job that runs on, its driver having stopped waiting for it, which it says how to wait
# for again.
host_end() {
    case $host_state in
        '') return 0 ;;
        launched)
            echo "$host_run runs on, on $host: wait for it and fetch its results with" \
                "\`$0 resume $host_run ${results:-RESULTS}\`, or end it with \`$0 cancel $host_run\`." >&2
            return 0
            ;;
    esac
    if ! host_side clean "$host_dir" "$host_repo"; then
        echo "Removing $host_dir from $host failed." >&2
        return 1
    fi
    host_state=''
}

# host_exit, the trap host_start sets, ends the run (host_end) as the driver exits; a failure to
# fails the driver. A driver that sets a trap of its own calls host_end in it.
host_exit() {
    local status=$?
    trap - EXIT
    host_end || status=1
    exit "$status"
}

# host_actions [ACTION [ARG...]] does the actions a driver takes of a run, which a driver calls
# with its arguments before its own: `resume RUN RESULTS` waits for the job of the run RUN, whose
# driver stopped waiting for it, fetches its results to RESULTS, runs host_after's function, and
# ends the run, as the driver would have; `cancel RUN` ends the job and the run. Each exits; given
# any other arguments, this returns.
host_actions() {
    local status=0
    case ${1:-} in
        resume)
            (( $# == 3 )) || { echo "Usage: $0 resume RUN RESULTS" >&2; exit 2; }
            [[ $2 =~ $host_run_pattern ]] || { echo "Name the run, run-..." >&2; exit 2; }
            host_name "$2"
            results=$3
            mkdir -p "$results"
            results=$(cd "$results" && pwd)
            echo "Results: $results" >&2
            host_state=launched
            trap host_exit EXIT
            host_wait >> "$results/remote.log" 2>&1 || status=$?
            if [[ $host_state == launched ]]; then exit "$status"; fi
            host_fetch || status=$?
            if [[ -n $host_after ]]; then "$host_after" "$status" || status=$?; fi
            exit "$status"
            ;;
        cancel)
            (( $# == 2 )) || { echo "Usage: $0 cancel RUN" >&2; exit 2; }
            [[ $2 =~ $host_run_pattern ]] || { echo "Name the run, run-..." >&2; exit 2; }
            host_name "$2"
            host_side cancel "$host_dir" || exit
            host_state=ended
            host_end || exit
            echo "Ended $host_run on $host." >&2
            exit 0
            ;;
    esac
}
