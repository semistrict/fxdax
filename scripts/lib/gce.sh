# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Kept GCE hosts for the scripts that run fxdax on them, sourced by them:
# scripts/test-gce.sh, bench-gce.sh, gets-gce.sh, example-gce.sh,
# mutants-gce.sh, fork-time-gce.sh and migrate-gce.sh. A host is kept, not made
# for a run: a run reuses one that fits it, stopped or idle, and stops it as it
# ends, whatever happens, so that its builds (its target directories, its guest
# kernels', its toolchain and packages) carry over to the next run, which builds
# only what changed, and nothing of a fresh host's setup can fail the run. A
# stopped host holds no vCPU quota, only its disk. A host is deleted only by
# hand (a driver's `delete`), when it is broken beyond repair.
#
# A host is named for what it is, fxdax-keep-MACHINE-ZONE-N, and carries the
# labels purpose=fxdax,lifecycle=kept, and nothing here operates on one without
# them, and kind, a digest of what it was made with (gce_shape): its machine
# type, CPU platform, image, disk and service account. A run fits a host of its
# own kind alone. A host is a run's once the run claims it: a symbolic link in
# the host's /run, made atomically, which names its holder, and which the host's
# stop, emptying its /run, a tmpfs, releases (gce_claim). No two runs hold one
# host at once. A host left running, as by a driver killed, powers off, which
# stops it, after two hours without a command from here and without a job
# running (scripts/lib/gce-host-startup.sh). What a script runs there, one of
# the host scripts (scripts/lib/*-linux-host.sh), runs in this machine's
# environment, but for what sets up this machine's own builds and daemons
# (gce_host_command).
#
# The drivers run these functions under `||` and `if !`, where errexit stops
# nothing, so each checks the status of its every step itself.
#
# The sourcing script sets project, and FXDAX_GCE_STORAGE if it is to be set,
# before it sources this, which draws the run's mark (gce_maker) that its name
# (gce_run_name) and the script's own names end in: two runs at once share no
# host, results or prefix of a bucket. Then it sets repo, zone (the zone to try
# first), machine, limit (the longest a run may take on the host, timeout's
# DURATION) and results, and passes any of its own creation flags in
# create_flags. A driver's actions are gce_main's.
#
# A host has no service account, unless FXDAX_GCE_STORAGE or FXDAX_RESULTS_BUCKET
# is set: then it has the project's default one, with read and write access to
# Cloud Storage, for an extent daemon keeping its stores' branches in a bucket
# (doc-7), or the guest tests keeping their results in one (test-gce.sh).

cloud=(gcloud --quiet --project="$project")
# This run's mark, which its name and the names of its results end in.
gce_maker=$(od -An -N4 -tx1 /dev/urandom | tr -d ' \n')
[[ ${FXDAX_GCE_RUN:-} =~ ^[a-z0-9]*$ ]] ||
    { echo "FXDAX_GCE_RUN names a run in lower-case letters and digits, such as r1040" >&2; exit 2; }
# The run's name, its directory and job on a host, its prefix of a bucket and the holder of the host
# it claims: FXDAX_GCE_RUN, the run's number as its caller counts runs (such as r1040), if set; when
# it started, to the second; and this driver's mark, so that two drivers started in the same second
# keep apart (TASK-79.86). A run's job files on a host, which keeps them, collide with no other's.
gce_run="run-${FXDAX_GCE_RUN:+$FXDAX_GCE_RUN-}$(date -u +%Y%m%dT%H%M%SZ)-$gce_maker"
gce_run_name() {
    echo "$gce_run"
}
# What a run's name looks like, for those given one (test-gce.sh's resume).
# shellcheck disable=SC2034 # The drivers read it.
gce_run_pattern='^run-([a-z0-9]+-)?[0-9]{8}T[0-9]{6}Z-[0-9a-f]{8}$'
if [[ -n ${FXDAX_GCE_STORAGE:-}${FXDAX_RESULTS_BUCKET:-} ]]; then
    account_flags=(--scopes=storage-rw)
else
    account_flags=(--no-service-account --no-scopes)
fi
# The host this run holds, once it claims one, and its zone, which gce_release stops.
gce_held='' gce_held_zone=''

# gce_shape prints, a line each, the flags a host is made with that a run needs
# of the host it runs on: what its kind is a digest of (gce_kind).
gce_shape() {
    printf '%s\n' --machine-type="$machine" --enable-nested-virtualization \
        ${create_flags[@]+"${create_flags[@]}"} \
        --image=ubuntu-2604-resolute-amd64-v20260918 --image-project=ubuntu-os-cloud \
        --boot-disk-size=200GB --boot-disk-type=pd-balanced --boot-disk-auto-delete \
        "${account_flags[@]}"
}

# gce_kind prints this run's kind of host, which the hosts it fits carry as their
# kind label.
gce_kind() {
    local digest
    digest=$(gce_shape | shasum -a 256) || return
    echo "${digest:0:12}"
}

# gce_kept prints the kept hosts of this run's kind, NAME ZONE STATUS a line, in
# the order a run tries them: stopped ones first, as a running one is likely
# another run's; and of each, those in $zone first, then those in its region.
gce_kept() {
    local kind hosts
    kind=$(gce_kind) || return
    hosts=$("${cloud[@]}" compute instances list \
        --filter="labels.lifecycle=kept AND labels.kind=$kind" \
        --format='value(name,zone.basename(),status)') || return
    awk -v zone="$zone" '
        $3 == "TERMINATED" || $3 == "RUNNING" {
            region = zone; sub(/-[^-]*$/, "", region)
            print ($3 == "TERMINATED" ? 0 : 1) ($2 == zone ? 0 : index($2, region "-") == 1 ? 1 : 2), $0
        }' <<< "$hosts" | sort | cut -d' ' -f2-
}

# gce_acquire HOLDER points instance and zone at a host of this run's kind that
# HOLDER claims: a kept one, started if it is stopped, that no other run holds;
# or, if none fits or every one that fits is another run's, a new one
# (gce_create). A host it started that does not come up, it stops. Each runs the
# startup script as this tree has it: one started or made boots with it
# (gce_start), and one found running runs it again (gce_refresh).
gce_acquire() {
    local holder=$1 wanted=$zone hosts name found status
    hosts=$(gce_kept) || return
    while read -r -u 3 name found status; do
        [[ -n $name ]] || continue
        instance=$name zone=$found
        if [[ $status == TERMINATED ]]; then
            if ! gce_start; then
                echo "$instance did not start; trying the next host." >&2
                continue
            fi
            if ! gce_wait_ready; then
                gce_stop
                continue
            fi
        fi
        if gce_claim "$holder"; then
            echo "Reusing $instance in $zone." >&2
            if [[ $status == RUNNING ]]; then gce_refresh || return; fi
            return 0
        fi
        echo "$instance is another run's; trying the next host." >&2
    done 3<<< "$hosts"
    zone=$wanted
    # A create that failed may have made the host all the same.
    if ! gce_create && [[ -z $(gce_status) ]]; then
        return 1
    fi
    if ! gce_wait_ready; then
        gce_stop
        return 1
    fi
    if ! gce_claim "$holder"; then
        echo "$instance, made for this run, is another run's." >&2
        return 1
    fi
}

# gce_claim HOLDER [again] claims the host for HOLDER: it fails if another holds
# it, and with `again`, succeeds if HOLDER does. The claim is a symbolic link to
# HOLDER's name, made atomically, in the host's /run, a tmpfs, which the host's
# stop empties: so a stop releases it, and a stopped host is no one's. A new
# claim deletes the runs on the host untouched for three days, whose results a
# resume has had time to fetch.
gce_claim() {
    local claim=/run/fxdax-host/claim mine=false
    if [[ ${2:-} == again ]]; then mine="test \"\$(readlink $claim)\" = $1"; fi
    gce_remote "sudo mkdir -p /run/fxdax-host &&
        if sudo ln -s $1 $claim 2> /dev/null; then
            find \"\$HOME\" -maxdepth 1 -name 'run-*' -mtime +3 -exec sudo rm -r -- {} + ||
                echo 'Deleting the runs of three days ago failed.' >&2
        else
            $mine
        fi" || return
    gce_held=$instance gce_held_zone=$zone
}

# gce_release stops the host this run holds, if it holds one.
gce_release() {
    if [[ -z $gce_held ]]; then return 0; fi
    instance=$gce_held zone=$gce_held_zone
    gce_stop
}

# gce_holder prints who holds the host's claim (gce_claim), if it is running and claimed.
gce_holder() {
    gce_remote 'readlink /run/fxdax-host/claim'
}

# gce_check_held succeeds if the host is held for a session of runs, by a
# driver's `create` (gce_main): a run named runs on no host but one so held,
# and so on none another's run holds.
gce_check_held() {
    local holder
    holder=$(gce_holder 2> /dev/null) || holder=
    if [[ $holder != held-* ]]; then
        echo "$instance is not held for runs (${holder:-not running, or unclaimed}): hold a host with create." >&2
        return 1
    fi
}

# gce_refresh gives the running host the startup script as this tree has it, and
# runs it, as the host's guest agent runs it at boot: a kept host keeps the
# script it last booted with, and r941's, without main's needrestart settings,
# lost two runs. The script is idempotent.
gce_refresh() {
    "${cloud[@]}" compute instances add-metadata "$instance" --zone="$zone" \
        --metadata-from-file="startup-script=$repo/scripts/lib/gce-host-startup.sh" || return
    if ! gce_remote 'sudo google_metadata_script_runner startup' >> "$results/startup.log" 2>&1; then
        echo "$instance did not run its startup script again: see $results/startup.log" >&2
        return 1
    fi
}

# gce_start starts the stopped host, with the startup script as it is now: a
# kept host runs what this tree's says, as a new host would.
gce_start() {
    "${cloud[@]}" compute instances add-metadata "$instance" --zone="$zone" \
        --metadata-from-file="startup-script=$repo/scripts/lib/gce-host-startup.sh" || return
    "${cloud[@]}" compute instances start "$instance" --zone="$zone" || return
    echo "Started $instance in $zone." >&2
}

# gce_stop stops the host, which releases its claim, and succeeds once its
# listing says it is stopped.
gce_stop() {
    local status
    "${cloud[@]}" compute instances stop "$instance" --zone="$zone" || return
    status=$(gce_status) || return
    if [[ $status != TERMINATED ]]; then
        echo "$instance is not stopped: ${status:-gone}" >&2
        return 1
    fi
    if [[ $gce_held == "$instance" ]]; then gce_held='' gce_held_zone=''; fi
    echo "Stopped $instance in $zone." >&2
}

# gce_status prints the host's status (RUNNING, TERMINATED for a stopped one,
# ...), or nothing if there is no host of its name. It fails if the listing
# does.
gce_status() {
    "${cloud[@]}" compute instances list --filter="name=$instance" --format='value(status)'
}

# gce_create makes a host of this run's kind in $zone, or where that zone has no
# room for its machine, in the next zone of the same region that does; and where
# the region is out of quota (its SSD or CPUs), or out of room, in the next region
# of FXDAX_GCE_REGIONS's (TASK-97). The project's own quota, of every region, it
# stops at. zone and instance name each host as its create is tried: a create
# that fails may have made the host there all the same, which gce_acquire then
# takes.
gce_create() {
    local first=$zone region regions zones candidates candidate log
    regions=("${first%-*}")
    for region in ${FXDAX_GCE_REGIONS:-us-east4 us-east1 us-central1 us-west1}; do
        [[ $region == "${first%-*}" ]] || regions+=("$region")
    done
    log=$(mktemp) || return
    for region in "${regions[@]}"; do
        if ! zones=$("${cloud[@]}" compute zones list --filter="region:$region" \
            --format='value(name)'); then
            rm -f -- "$log"
            return 1
        fi
        candidates=()
        [[ $region == "${first%-*}" ]] && candidates=("$first")
        while read -r candidate; do
            [[ -z $candidate || $candidate == "$first" ]] || candidates+=("$candidate")
        done < <(sort <<< "$zones")
        for candidate in ${candidates[@]+"${candidates[@]}"}; do
            zone=$candidate
            if gce_create_in 2> "$log"; then
                cat "$log" >&2
                echo "Created $instance in $zone." >&2
                rm -f -- "$log"
                return 0
            fi
            if [[ $(< "$log") =~ Quota\ \'[A-Z_]+\'\ exceeded.*in\ region ]]; then
                echo "$region is out of quota for this machine; trying the next region." >&2
                continue 2
            fi
            if ! [[ $(< "$log") =~ ZONE_RESOURCE_POOL_EXHAUSTED|does\ not\ have\ enough\ resources ]]; then
                cat "$log" >&2
                rm -f -- "$log"
                return 1
            fi
            echo "$candidate has no room for this machine; trying the next zone." >&2
        done
    done
    cat "$log" >&2
    rm -f -- "$log"
    return 1
}

# gce_create_in makes a host in $zone, named fxdax-keep-MACHINE-ZONE-N, N the
# least that no host has: or the next, if another run made a host of that name
# as this one tried. The name holds the zone, so that names are the project's
# own, as GCE keeps them a zone's alone.
gce_create_in() {
    local taken kind n=1 shape=() flag errors made
    taken=$("${cloud[@]}" compute instances list --filter=labels.lifecycle=kept \
        --format='value(name)') || return
    kind=$(gce_kind) || return
    while read -r flag; do
        shape+=("$flag")
    done < <(gce_shape)
    errors=$(mktemp) || return
    while true; do
        instance=fxdax-keep-$machine-$zone-$n
        n=$((n + 1))
        if [[ $'\n'$taken$'\n' == *$'\n'$instance$'\n'* ]]; then continue; fi
        made=0
        # What it made, to the errors: a driver's output is its own.
        "${cloud[@]}" compute instances create "$instance" --zone="$zone" "${shape[@]}" \
            --metadata=block-project-ssh-keys=TRUE \
            --metadata-from-file="startup-script=$repo/scripts/lib/gce-host-startup.sh" \
            --labels="purpose=fxdax,lifecycle=kept,kind=$kind" \
            --maintenance-policy=TERMINATE --no-restart-on-failure >&2 2> "$errors" ||
            made=$?
        if (( made == 0 )) || ! [[ $(< "$errors") =~ $instance\ already\ exists ]]; then
            cat "$errors" >&2
            rm -f -- "$errors"
            return "$made"
        fi
        echo "Another run made $instance as this one did; trying the next name." >&2
    done
}

# gce_adopt makes the host, stopped, one made before hosts were kept (as the old
# scripts' fxdax-test-...), a kept host of this run's kind, keeping its name, if
# it fits the run: its machine type, CPU platform and service account, as this
# run's hosts are made with (gce_shape); its image and disk are those every
# driver made hosts with. Its schedule's own end, which deleted it, is cleared,
# as it can't be while it runs: a kept host's end is its stop.
gce_adopt() {
    local shape status type cpu account wanted_cpu='' wanted_account flag kind
    for flag in ${create_flags[@]+"${create_flags[@]}"}; do
        if [[ $flag == --min-cpu-platform=* ]]; then wanted_cpu=${flag#*=}; fi
    done
    kind=$(gce_kind) || return
    shape=$("${cloud[@]}" compute instances describe "$instance" --zone="$zone" \
        --format='csv[no-heading](status,machineType.basename(),minCpuPlatform,serviceAccounts[0].email)') ||
        return
    IFS=, read -r status type cpu account <<< "$shape"
    if [[ $status != TERMINATED ]]; then
        echo "Refusing to adopt $instance, which is $status: stop it first." >&2
        return 1
    fi
    # A service account if, and only if, the run's hosts have one (account_flags).
    wanted_account=${FXDAX_GCE_STORAGE:-}${FXDAX_RESULTS_BUCKET:-}
    if [[ $type != "$machine" || ${cpu/#Automatic/} != "$wanted_cpu" ||
        ${account:+1} != "${wanted_account:+1}" ]]; then
        echo "Refusing to adopt $instance, which fits no run of this kind: $type, CPU" \
            "${cpu:-any}, ${account:-no service account}." >&2
        return 1
    fi
    "${cloud[@]}" compute instances set-scheduling "$instance" --zone="$zone" \
        --clear-max-run-duration --clear-instance-termination-action || return
    "${cloud[@]}" compute instances add-labels "$instance" --zone="$zone" \
        --labels="purpose=fxdax,lifecycle=kept,kind=$kind" || return
    echo "Adopted $instance in $zone, a kept host of kind $kind." >&2
}

# gce_locate points zone at the zone the host is in: every other action finds
# the host by its name.
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

# gce_check_owner succeeds if the host carries the labels of a kept host. gcloud
# prints a label the host lacks as empty, after its tab.
gce_check_owner() {
    local labels
    labels=$("${cloud[@]}" compute instances describe "$instance" --zone="$zone" \
        --format='value(labels.purpose,labels.lifecycle)') || return
    if [[ $labels != fxdax$'\t'kept ]]; then
        echo "Refusing to operate on $instance, which lacks the labels of a kept host." >&2
        return 1
    fi
}

# gce_delete deletes the host and its boot disk, if it is there and carries the
# labels of a kept host, and succeeds once the listings say both are gone: by
# hand alone (a driver's `delete`), for a host broken beyond repair.
gce_delete() {
    local listed
    listed=$(gce_listed instances) || return
    if [[ -n $listed ]]; then
        gce_check_owner || return
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
# this function's. A job whose starting SSH failed is waited for only if a fresh connection finds
# it launched, its pid kept in ~/NAME.job/pid: that SSH may drop once the job runs, as r942's did,
# whose results were fetched before it ran (TASK-235); one that didn't start returns that SSH's
# status, not waited for until the host's end.
#
# Usage: gce_remote_detached NAME COMMAND...
gce_remote_detached() {
    local name=$1 job started=0
    shift
    job="\$HOME/$name.job"
    # The launch and its pid alone in the background's list: `A && B & C` puts A and B there too.
    gce_remote "mkdir -p $job && rm -f $job/status $job/pid && { setsid nohup bash -c \
        $(printf %q "$*; echo \$? > $job/status") > $job/log 2>&1 < /dev/null & \
        echo \$! > $job/pid; }" || started=$?
    if ((started)) && ! gce_remote "test -e $job/pid"; then
        return "$started"
    fi
    gce_remote_wait "$name"
}

# gce_remote_wait waits for the job NAME gce_remote_detached started, as it does: for one whose
# waiter was stopped, too. A job whose process is gone without its status, as one is when its host
# was stopped under it and started again, is lost, and so is one whose host is stopped or gone.
#
# Usage: gce_remote_wait NAME
gce_remote_wait() {
    local name=$1 job status='' state look
    job="\$HOME/$name.job"
    # Its status, if it has one: its process may write it and end as this looks.
    look="cat $job/status 2>/dev/null || kill -0 \"\$(cat $job/pid 2>/dev/null)\" 2>/dev/null ||
        cat $job/status 2>/dev/null || echo lost"
    while [[ -z $status ]]; do
        sleep 30
        status=$(gce_remote "$look" 2> /dev/null) || status=
        if [[ $status == lost ]]; then
            echo "$name on $instance is lost: its process ended without its status." >&2
            gce_remote "cat $job/log"
            return 1
        fi
        # Not running only if the listing says so: a listing that fails says nothing.
        if [[ -z $status ]] && state=$(gce_status) && [[ $state != RUNNING ]]; then
            echo "$instance is ${state:-gone}: $name did not finish." >&2
            return 1
        fi
    done
    gce_remote "cat $job/log"
    return "$status"
}

# gce_wait_ready waits until the host accepts SSH and its startup script ran since it last started:
# its mark of that is in /run, which a stop empties.
gce_wait_ready() {
    for _ in {1..60}; do
        if gce_remote 'test -e /run/fxdax-host/ready' >> "$results/startup.log" 2>&1; then
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
# the host, and records what it copied, the host it copied it to, and the run's
# name and host (run.txt), which test-gce.sh's resume takes. With
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
    printf 'run %s\nhost %s\nzone %s\n' "$run" "$instance" "$zone" > "$results/run.txt" || return
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
    local staging=$1 revision firecracker kernel built
    # The commit its builds report, as builds here would: the host's copy has no .git
    # (TASK-111.7).
    built=$("$(dirname "${BASH_SOURCE[0]}")/../build-commit.sh" "$repo") || return
    python3 - "$repo" "$staging/source.tar.gz" "$built" <<'PY' || return
import io, pathlib, subprocess, sys, tarfile
root = pathlib.Path(sys.argv[1])
def files(directory):
    return subprocess.check_output(['git', '-C', str(directory), 'ls-files', '--cached', '--others', '--exclude-standard', '-z']).split(b'\0')
submodules = ['vendor/firecracker']
# A submodule not checked out, as a new worktree's is, is no source: git there is the project's,
# whose files it would list, and the host would build what an earlier run left of it, or fail.
for submodule in submodules:
    if not (root / submodule / '.git').exists():
        sys.exit(f'{submodule} is not checked out: git -C {root} submodule update --init {submodule}')
names = {name.decode() for name in files(root) if name} - set(submodules)
for submodule in submodules:
    names.update(submodule + '/' + name.decode() for name in files(root / submodule) if name)
revision = sys.argv[3]
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
# and gce_all stops the host next: so the archive is copied and unpacked
# however tar ended, and the first failure is this function's.
gce_fetch() {
    local run=$1 archived=0 fetched=0
    gce_remote "sudo chown -R \"\$(id -u):\$(id -g)\" $run/results;
        tar -czf $run/results.tar.gz --exclude=rootfs --exclude='*rootfs.ext4' --exclude=jail \
            --exclude='*-cache' -C $run/results ." \
        >> "$results/remote.log" 2>&1 || archived=$?
    { "${cloud[@]}" compute scp --zone="$zone" "$instance:$run/results.tar.gz" "$results/results.tar.gz" &&
        tar -xzf "$results/results.tar.gz" -C "$results" && rm "$results/results.tar.gz"; } || fetched=$?
    return $(( archived ? archived : fetched ))
}

# gce_all runs the given command on a host it acquires for this run
# (gce_acquire), and stops the host whatever happens. The trap is set before the
# host is acquired, and stops the host once this run claims it.
gce_all() {
    trap 'status=$?; trap - EXIT; if ! gce_release; then exit 1; fi; exit "$status"' EXIT
    gce_acquire "$gce_run" || exit
    "$@"
}

# gce_main [ACTION [ARG...]] does a driver's ACTION, all by default, its run
# that of the driver's function run, which the driver defines:
#
#   all [RESULTS]           acquire a host (gce_acquire), run on it, and stop it
#   create [RESULTS]        acquire a host and hold it for a session of runs,
#                           printing its name, until its stop
#   run INSTANCE [RESULTS]  run on a host a create holds
#   stop INSTANCE           stop a host, which ends its holding
#   delete INSTANCE         delete a host and its disk, broken beyond repair
#   adopt INSTANCE          make a stopped host the old scripts made a kept one
#                           (gce_adopt)
#
# RESULTS takes the place of the results the driver set.
gce_main() {
    local action=${1:-all}
    if (( $# > 0 )); then shift; fi
    case $action in
        all | create)
            results=${1:-$results}
            mkdir -p "$results"
            if [[ $action == all ]]; then
                gce_all run
            else
                gce_acquire "held-$gce_maker" || return
                echo "Holding $instance in $zone: run on it with \`run $instance\`, stop it with \`stop $instance\`." >&2
                echo "$instance"
            fi
            ;;
        run | stop | delete | adopt)
            # A host the old scripts made keeps its name, fxdax-test-... (gce_adopt).
            instance=${1:-}
            if ! [[ $instance =~ ^fxdax-[a-z0-9-]+$ ]]; then
                echo "Name a host, fxdax-..." >&2
                return 2
            fi
            results=${2:-$results}
            mkdir -p "$results"
            gce_locate || return
            case $action in
                run) gce_check_held && run ;;
                stop) gce_check_owner && gce_stop ;;
                delete) gce_delete ;;
                adopt) gce_adopt ;;
            esac
            ;;
        *)
            echo "Usage: $0 [all|create] [results-directory] | run|stop|delete|adopt instance [results-directory]" >&2
            return 2
            ;;
    esac
}
