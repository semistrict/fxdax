# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Disposable GCE hosts for the scripts that run fxdax on them, sourced by them:
# scripts/test-gce.sh, bench-gce.sh, gets-gce.sh, example-gce.sh,
# mutants-gce.sh and migrate-gce.sh. A host carries the labels
# purpose=$purpose,lifecycle=temporary, and nothing here operates on one
# without them, and maker, the mark of the run that made it: a run deletes, as
# it ends, only a host it made (gce_all), as another run at once may have made
# one of the same name. A host deletes itself $lifetime after it was created,
# whatever becomes of the shell that made it, and powers off after a day
# without a command from here (scripts/lib/gce-host-startup.sh). What a script
# runs there, one of the host scripts (scripts/lib/*-linux-host.sh), runs in
# this machine's environment, but for what sets up this machine's own builds
# and daemons (gce_host_command).
#
# The drivers run these functions under `||` and `if !`, where errexit stops
# nothing, so each checks the status of its every step itself.
#
# The sourcing script sets project, and FXDAX_GCE_STORAGE if it is to be set,
# before it sources this, which draws the run's mark (gce_maker) that the
# script's own names end in: two runs at once share no host, results or prefix
# of a bucket. Then it sets zone (the zone to try first), instance, machine,
# purpose, lifetime, limit (the longest a run may take on the host, timeout's
# DURATION) and results, and passes any of its own creation flags in
# create_flags.
#
# A host has no service account, unless FXDAX_GCE_STORAGE is set: then it has
# the project's default one, with read and write access to Cloud Storage, for
# an extent daemon keeping its stores' branches in a bucket (doc-7).

cloud=(gcloud --quiet --project="$project")
# This run's mark, which the hosts it makes carry as their maker label.
gce_maker=$(od -An -N4 -tx1 /dev/urandom | tr -d ' \n')
if [[ -n ${FXDAX_GCE_STORAGE:-} ]]; then
    account_flags=(--scopes=storage-rw)
else
    account_flags=(--no-service-account --no-scopes)
fi

# gce_create makes the host in $zone, or where that zone has no room for its
# machine, in the next zone of the same region that does. zone names each zone
# as its create is tried: a create that fails may have made the host there all
# the same, which gce_all's trap then finds there.
gce_create() {
    local region zones candidates candidate log
    region=${zone%-*}
    zones=$("${cloud[@]}" compute zones list --filter="region:$region" --format='value(name)') ||
        return
    candidates=("$zone")
    while read -r candidate; do
        [[ "$candidate" == "$zone" ]] || candidates+=("$candidate")
    done < <(sort <<< "$zones")
    log=$(mktemp) || return
    for candidate in "${candidates[@]}"; do
        zone=$candidate
        if gce_create_in "$zone" 2> "$log"; then
            echo "Created $instance in $zone." >&2
            rm -f -- "$log"
            return 0
        fi
        if ! [[ $(< "$log") =~ ZONE_RESOURCE_POOL_EXHAUSTED|does\ not\ have\ enough\ resources ]]; then
            cat "$log" >&2
            rm -f -- "$log"
            return 1
        fi
        echo "$candidate has no room for this machine; trying the next zone." >&2
    done
    cat "$log" >&2
    rm -f -- "$log"
    return 1
}

gce_create_in() {
    "${cloud[@]}" compute instances create "$instance" --zone="$1" \
        --machine-type="$machine" --enable-nested-virtualization ${create_flags[@]+"${create_flags[@]}"} \
        --image=ubuntu-2604-resolute-amd64-v20260918 --image-project=ubuntu-os-cloud \
        --boot-disk-size=200GB --boot-disk-type=pd-balanced --boot-disk-auto-delete \
        "${account_flags[@]}" \
        --metadata=block-project-ssh-keys=TRUE \
        --metadata-from-file="startup-script=$repo/scripts/lib/gce-host-startup.sh" \
        --labels="purpose=$purpose,lifecycle=temporary,maker=$gce_maker" \
        --maintenance-policy=TERMINATE --no-restart-on-failure \
        --max-run-duration="$lifetime" --instance-termination-action=DELETE
}

# gce_locate points zone at the zone the host is in, which gce_create may have
# chosen: every other action finds the host by its name.
gce_locate() {
    local found
    found=$("${cloud[@]}" compute instances list --filter="name=$instance" \
        --format='value(zone.basename())') || return
    if [[ -n "$found" ]]; then zone=$found; fi
}

# gce_listed KIND prints the name of the host, with KIND instances, or of its
# boot disk, with KIND disks, if there is one. It fails if the listing does,
# which says nothing of whether there is one.
gce_listed() {
    "${cloud[@]}" compute "$1" list --filter="name=$instance" --format='value(name)'
}

# gce_check_owner [mine] succeeds if the host carries this script's labels;
# with `mine`, only if this run made it too, which no host without a maker
# label, as those made before hosts had one, was. gcloud prints a label the
# host lacks as empty, after its tab.
gce_check_owner() {
    local labels
    labels=$("${cloud[@]}" compute instances describe "$instance" --zone="$zone" \
        --format='value(labels.purpose,labels.lifecycle,labels.maker)') || return
    case $labels in
        "$purpose"$'\t'temporary$'\t'*) ;;
        *)
            echo "Refusing to operate on $instance, which lacks the $purpose labels." >&2
            return 1
            ;;
    esac
    if [[ ${1:-} == mine && $labels != "$purpose"$'\t'temporary$'\t'"$gce_maker" ]]; then
        echo "Refusing to delete $instance, which another run made." >&2
        return 1
    fi
}

# gce_delete [mine] deletes the host and its boot disk, if it is there and
# carries this script's labels (gce_check_owner; with `mine`, if this run made
# it), and succeeds once the listings say both are gone.
gce_delete() {
    local listed
    listed=$(gce_listed instances) || return
    if [[ -n $listed ]]; then
        gce_check_owner "$@" || return
        "${cloud[@]}" compute instances delete "$instance" --zone="$zone" --delete-disks=all || return
    fi
    listed=$(gce_listed instances) || return
    if [[ -n $listed ]]; then
        echo "Instance still exists: $instance" >&2
        return 1
    fi
    listed=$(gce_listed disks) || return
    if [[ -n $listed ]]; then
        echo "Boot disk still exists: $instance" >&2
        return 1
    fi
    echo "Verified deletion of $instance and its boot disk." >&2
}

# gce_remote runs a command on the host, renewing its lease. Keepalives, as a
# command may run for hours without output.
gce_remote() {
    "${cloud[@]}" compute ssh "$instance" --zone="$zone" \
        --ssh-flag='-o ConnectTimeout=15' --ssh-flag='-o ServerAliveInterval=60' \
        --ssh-flag='-o ServerAliveCountMax=10' \
        --command="sudo touch /var/lib/fxdax-host/lease 2>/dev/null; $*" < /dev/null
}

# gce_remote_detached runs COMMAND on the host apart from the SSH session, as job NAME, so that a
# dropped connection, or this machine sleeping, does not stop it, and waits for it, reconnecting:
# its output, kept in the host's ~/NAME.job/log, is printed once it ends, and its exit status is
# this function's.
#
# Usage: gce_remote_detached NAME COMMAND...
gce_remote_detached() {
    local name=$1 job status= listed
    shift
    job="\$HOME/$name.job"
    gce_remote "mkdir -p $job && rm -f $job/status && setsid nohup bash -c \
        $(printf %q "$*; echo \$? > $job/status") > $job/log 2>&1 < /dev/null &" || return
    while [[ -z $status ]]; do
        sleep 30
        status=$(gce_remote "cat $job/status 2>/dev/null" 2> /dev/null) || status=
        # Gone only if the listing says so: a listing that fails says nothing.
        if [[ -z $status ]] && listed=$(gce_listed instances) && [[ -z $listed ]]; then
            echo "$instance is gone: $name did not finish." >&2
            return 1
        fi
    done
    gce_remote "cat $job/log"
    return "$status"
}

# gce_wait_ready waits until the host accepts SSH and its startup script ran.
gce_wait_ready() {
    for _ in {1..60}; do
        if gce_remote 'test -e /var/lib/fxdax-host/ready' >> "$results/startup.log" 2>&1; then
            return 0
        fi
        sleep 10
    done
    "${cloud[@]}" compute instances get-serial-port-output "$instance" --zone="$zone" | tail -40 >&2 || true
    echo "$instance did not finish starting." >&2
    return 1
}

# gce_push_source copies the working tree's versioned and non-ignored source,
# Firecracker's included, without Git internals or build caches, to RUN/source on
# the host, and records what it copied and the host it copied it to. With
# FXDAX_GUEST_KERNEL naming fxdax's guest kernel here, it copies that to
# RUN/guest-kernel too (gce_host_command). The archive keeps the checkout's
# file times, which the trees the host builds in do not go by (host_tree,
# scripts/lib/linux-host-setup.sh).
gce_push_source() {
    local run=$1 staging staged=0
    staging=$(mktemp -d "${TMPDIR:-/tmp}/fxdax-gce-source.XXXXXX") || return
    gce_stage_source "$staging" || staged=$?
    rm -r -- "$staging"
    (( staged == 0 )) || return "$staged"
    gce_remote "set -eu; mkdir -p $run/source $run/results; tar -xzf source.tar.gz -C $run/source" ||
        return
    if [[ -n ${FXDAX_GUEST_KERNEL:-} ]]; then
        "${cloud[@]}" compute scp --zone="$zone" "$FXDAX_GUEST_KERNEL" "$instance:$run/guest-kernel"
    fi
}

# gce_stage_source STAGING makes the archive of the source in the directory
# STAGING, records it and the host (gce_push_source), and copies it to the
# host's home.
gce_stage_source() {
    local staging=$1 revision firecracker kernel
    python3 - "$repo" "$staging/source.tar.gz" <<'PY' || return
import io, pathlib, subprocess, sys, tarfile
root = pathlib.Path(sys.argv[1])
def files(directory):
    return subprocess.check_output(['git', '-C', str(directory), 'ls-files', '--cached', '--others', '--exclude-standard', '-z']).split(b'\0')
submodules = ['vendor/firecracker']
names = {name.decode() for name in files(root) if name} - set(submodules)
for submodule in submodules:
    names.update(submodule + '/' + name.decode() for name in files(root / submodule) if name)
revision = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD']).decode().strip() + '-working-tree'
with tarfile.open(sys.argv[2], 'w:gz') as archive:
    for name in sorted(names):
        path = root / name
        if path.is_file() or path.is_symlink():
            archive.add(path, arcname=name, recursive=False)
    data = (revision + '\n').encode()
    entry = tarfile.TarInfo('source-revision.txt')
    entry.size = len(data)
    archive.addfile(entry, io.BytesIO(data))
PY
    revision=$(git -C "$repo" rev-parse HEAD) || return
    firecracker=$(git -C "$repo/vendor/firecracker" rev-parse HEAD) || return
    {
        echo "revision $revision"
        echo "firecracker $firecracker"
        git -C "$repo" status --porcelain=v1 | sed 's/^/changed /' || return
        (cd "$staging" && shasum -a 256 source.tar.gz) || return
        if [[ -n ${FXDAX_GUEST_KERNEL:-} ]]; then
            kernel=$(shasum -a 256 < "$FXDAX_GUEST_KERNEL") || return
            echo "guest kernel ${kernel%% *}"
        fi
    } > "$results/source.txt" || return
    "${cloud[@]}" compute instances describe "$instance" --zone="$zone" \
        --format='json(name,zone,machineType,cpuPlatform,scheduling,advancedMachineFeatures)' \
        > "$results/instance.json" || return
    "${cloud[@]}" compute scp --zone="$zone" "$staging/source.tar.gz" "$instance:source.tar.gz"
}

# gce_host_command RUN [NAME=VALUE...] SCRIPT [ARG...] prints the command, for
# gce_remote or gce_remote_detached, that runs the host script
# scripts/lib/SCRIPT of the source gce_push_source copied to RUN, as root and
# for at most $limit, with RUN/source, RUN/results and the ARGs.
#
# The script runs in this machine's environment: every FXDAX_* and PJDFSTEST_*
# variable set here, but those that stay here (gce_env_local); with
# FXDAX_GUEST_KERNEL set, the copy gce_push_source made; and each NAME=VALUE in
# place of the variable NAME. A value starting with ~/ names a path in the
# host's home. An empty value sends nothing, leaving NAME unset there, which its
# readers take for their default (an empty FXDAX_ZSTD_LEVEL is an error).
gce_host_command() {
    local run=$1 given=() assignments=(sudo env) name value i j
    shift
    for name in ${!FXDAX_@} ${!PJDFSTEST_@}; do
        if ! gce_env_local "$name"; then
            given+=("$name=${!name}")
        fi
    done
    if [[ -n ${FXDAX_GUEST_KERNEL:-} ]]; then
        # shellcheck disable=SC2088 # The host's home.
        given+=("FXDAX_GUEST_KERNEL=~/$run/guest-kernel")
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
    printf '%s' "${assignments[*]}"
    printf ' %q' timeout --signal=TERM --kill-after=30s "$limit" bash \
        "$run/source/scripts/lib/$script" "$run/source" "$run/results" "$@"
    echo
}

# gce_env_local NAME succeeds for the variables that stay here, though
# gce_host_command would send them by their names: those this machine sets up
# its own builds, daemons and runs with, and which the host scripts, or what
# they run, would take as they are. Set here, they name what this machine built
# or fetched, its own daemon's sockets, address and directories, and where its
# own runs write; sent, every build, test and daemon on the host would use them
# in place of its own. Those the host scripts always set themselves, such as
# FXDAX_FIRECRACKER and FXDAX_EXTENTD (scripts/test-e2e.sh), need no place
# here. The others go: knobs a driver's caller sets for the host, what runs and
# how, and where its stores keep their branches and its daemons what they read
# from them (FXDAX_EXTENTD_REMOTE, FXDAX_S3_URL, FXDAX_EXTENTD_CACHE), or a
# driver's own, which nothing there reads.
gce_env_local() {
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

# gce_fetch copies RUN/results on the host into the results, as one archive:
# but for the guests' root filesystems (rootfs, and their images), which aren't
# results, and some of whose files (setuid binaries) no one may read. The
# results become the SSH user's first, so that tar can read what root wrote,
# and tar's status says whether it did. A tar that fails may still have written
# the archive of every other file (1: a file changed as it read it, as the logs
# of a daemon a killed host script left running do; 2: it could not read one),
# and gce_all deletes the host next: so the archive is copied and unpacked
# however tar ended, and the first failure is this function's.
gce_fetch() {
    local run=$1 archived=0 fetched=0
    gce_remote "sudo chown -R \"\$(id -u):\$(id -g)\" $run/results;
        tar -czf $run/results.tar.gz --exclude=rootfs --exclude='*rootfs.ext4' -C $run/results ." \
        >> "$results/remote.log" 2>&1 || archived=$?
    { "${cloud[@]}" compute scp --zone="$zone" "$instance:$run/results.tar.gz" "$results/results.tar.gz" &&
        tar -xzf "$results/results.tar.gz" -C "$results" && rm "$results/results.tar.gz"; } || fetched=$?
    return $(( archived ? archived : fetched ))
}

# gce_all runs the given command on a new host and deletes the host whatever
# happens, if this run made it. The trap is set before creation, to cover
# ambiguous responses: a host made though its create failed is this run's too.
gce_all() {
    local listed
    listed=$(gce_listed instances) || exit
    if [[ -n $listed ]]; then
        echo "Instance already exists: $instance" >&2
        exit 1
    fi
    trap 'status=$?; trap - EXIT; if ! gce_delete mine; then exit 1; fi; exit "$status"' EXIT
    gce_create || exit
    "$@"
}
