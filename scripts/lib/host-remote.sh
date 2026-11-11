# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The host's half of scripts/lib/host.sh: what a run does on the Linux host it runs on, sent there
# over ssh on bash's standard input (host_side) and run as the user ssh logs in as, who may sudo
# without a password:
#
#   bash -s -- prepare DIR REPO COMMIT [LABEL=COMMIT...]
#   bash -s -- launch DIR NAME BENCH MEMORY
#   bash -s -- wait DIR
#   bash -s -- log DIR
#   bash -s -- fetch DIR
#   bash -s -- cancel DIR
#   bash -s -- clean DIR REPO
#
# A run's directory DIR, named for the run (run-..., host_run_name), holds tree, a worktree of
# COMMIT of the host's clone REPO (a path in the home, or absolute), and trees/LABEL, one of each
# other COMMIT; results, the host script's results, and work, its work directory (FXDAX_HOST_WORK),
# where it builds, its target directories its own; what the driver copied there (patch,
# guest-kernel); and job, the job's command, which the driver copied there too, its log, its pid,
# its exit status and its lock, which the job holds as long as it runs.
#
# prepare makes the worktrees, fetching REPO's origin if a commit is not there yet, with
# vendor/firecracker checked out in each and the patch, if any, applied to tree; the commit each
# reports its builds of in its source-revision.txt (scripts/build-commit.sh), as the host scripts'
# copies of the trees have no .git (host_tree); and prints a line for each tree, LABEL (`.` for
# tree), its commit and vendor/firecracker's. It first cleans the runs of three days ago whose job
# is not running: their drivers are gone.
#
# launch runs job/command apart from the SSH session, as a dropped connection or the driver's
# machine sleeping ends nothing, under fxjob (system-setup.d/146-fxdax_cgroups.sh), with --bench if
# BENCH is 1, with --mem MEMORY if one is given; it returns once the job holds its lock. wait waits
# for the lock, saying `waiting` once it has a connection, then `status N`, or `lost` if the job
# ended without its status, as with the host's restart, or `none` if no job was launched. fetch
# writes a gzipped tar of the results to its standard output: but for the guests' root filesystems
# (rootfs, their images, and jails), which aren't results, and some of whose files (setuid
# binaries) no one may read, and parts caches. cancel ends the job, and clean removes the run's
# directory, its worktrees with it, once no job runs there.
set -euo pipefail

# The job's command runs as root, sudo's, with the user's Rust (scripts/lib/host.sh's
# host_command): what it leaves in the user's ~/.cargo and ~/.rustup, the registry's files and a
# toolchain it installed, becomes the user's again, for the user's builds.
give_back() {
    local place
    for place in "$HOME/.cargo" "$HOME/.rustup"; do
        if [[ -d $place ]]; then
            sudo find "$place" ! -user "$(id -u)" -exec chown -h "$(id -u):$(id -g)" {} +
        fi
    done
}

# running DIR succeeds if a job holds DIR's lock: it runs.
running() {
    [[ -e $1/job/lock ]] && ! flock -n "$1/job/lock" true
}

# worktree LABEL prints where the worktree LABEL of prepare's run is: `.`'s, the run's own, is
# DIR/tree.
worktree() {
    if [[ $1 == . ]]; then echo "$dir/tree"; else echo "$dir/trees/$1"; fi
}

prepare() {
    local dir=$1 repo=$2 pairs=(".=$3") pair label commit tree fetched=0 old
    shift 3
    pairs+=("$@")
    [[ $(basename "$dir") == run-* ]] || { echo "prepare: $dir is no run's" >&2; return 2; }
    repo=$(cd && cd "$repo" && pwd)
    mkdir -p "$dir/results" "$dir/work" "$dir/job" "$dir/trees"
    # The runs of three days ago, whose job ended.
    while read -r old; do
        if [[ -n $old ]] && ! running "$old"; then
            echo "Removing $old, of three days ago." >&2
            sudo rm -r -- "$old"
        fi
    done < <(find "$(dirname "$dir")" -mindepth 1 -maxdepth 1 -name 'run-*' -mtime +3)
    git -C "$repo" worktree prune
    # One run at a time fetches, and adds worktrees, which share the clone's objects and modules.
    exec 8> "$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)/fxdax-host.lock"
    flock 8
    for pair in "${pairs[@]}"; do
        label=${pair%%=*} commit=${pair#*=}
        if ! git -C "$repo" cat-file -e "$commit^{commit}" 2> /dev/null; then
            if (( ! fetched )); then
                git -C "$repo" fetch -q origin
                fetched=1
            fi
            git -C "$repo" cat-file -e "$commit^{commit}" 2> /dev/null ||
                { echo "$commit is not on origin: push it first" >&2; return 1; }
        fi
        tree=$(worktree "$label")
        git -C "$repo" worktree add -q --detach "$tree" "$commit"
        git -C "$tree" submodule update -q --init vendor/firecracker
    done
    flock -u 8
    if [[ -f $dir/patch ]]; then
        git -C "$dir/tree" apply "$dir/patch"
    fi
    for pair in "${pairs[@]}"; do
        tree=$(worktree "${pair%%=*}")
        # An older commit's tree, of before scripts/build-commit.sh, reports its commit alone.
        if [[ -x $tree/scripts/build-commit.sh ]]; then
            "$tree/scripts/build-commit.sh" "$tree" > "$tree/source-revision.txt"
        else
            git -C "$tree" rev-parse HEAD > "$tree/source-revision.txt"
        fi
        echo "${pair%%=*} $(git -C "$tree" rev-parse HEAD) $(git -C "$tree/vendor/firecracker" rev-parse HEAD)"
    done
}

launch() {
    local dir=$1 name=$2 bench=$3 memory=$4 job=$1/job
    local fxjob=(fxjob --name "$name")
    if [[ $bench == 1 ]]; then fxjob+=(--bench); fi
    if [[ -n $memory ]]; then fxjob+=(--mem "$memory"); fi
    exec 9> "$job/lock"
    flock -n 9 || { echo "a job runs in $dir already" >&2; return 1; }
    rm -f "$job/status" "$job/pid"
    # The lock is fd 9's, which the job's shell inherits and holds until it has written the
    # status, and nothing it runs inherits: a daemon left behind holds no run's lock.
    # shellcheck disable=SC2016 # For the job's shell.
    setsid nohup bash -c "$(declare -f give_back)"'
        job=$1
        shift
        status=0
        "$@" 9>&- || status=$?
        give_back 9>&- || echo "giving back ~/.cargo and ~/.rustup failed" >&2
        echo "$status" > "$job/status"' \
        job "$job" "${fxjob[@]}" -- bash "$job/command" > "$job/log" 2>&1 < /dev/null &
    echo "$!" > "$job/pid"
}

wait_job() {
    local job=$1/job
    echo waiting
    if [[ ! -e $job/pid ]]; then
        echo none
        return 0
    fi
    flock "$job/lock" true
    if [[ -f $job/status ]]; then
        echo "status $(< "$job/status")"
    else
        echo lost
    fi
}

cancel() {
    local dir=$1
    if running "$dir"; then
        # The job's process group (setsid), sudo's command among it, which sudo hands the signal.
        sudo kill -TERM -- "-$(< "$dir/job/pid")"
        flock "$dir/job/lock" true
    fi
}

clean() {
    local dir=$1 repo
    [[ $(basename "$dir") == run-* ]] || { echo "clean: $dir is no run's" >&2; return 2; }
    repo=$(cd && cd "$2" && pwd)
    if running "$dir"; then
        echo "clean: a job runs in $dir" >&2
        return 1
    fi
    if [[ -e $dir ]]; then
        sudo rm -r -- "$dir"
    fi
    git -C "$repo" worktree prune
}

# The script's own text is this shell's standard input, which nothing it runs reads.
case ${1:-} in
    prepare) shift; prepare "$@" ;;
    launch) shift; launch "$@" ;;
    wait) wait_job "$2" ;;
    log) cat "$2/job/log" ;;
    fetch)
        sudo tar --exclude=rootfs --exclude='*rootfs.ext4' --exclude=jail --exclude='*-cache' \
            -czf - -C "$2/results" .
        ;;
    cancel) cancel "$2" ;;
    clean) shift; clean "$@" ;;
    *) echo "host-remote.sh: no such action: ${1:-}" >&2; exit 2 ;;
esac < /dev/null
