# shellcheck shell=bash
# Disposable GCE hosts for scripts/test-gce.sh and scripts/bench-gce.sh, sourced
# by them. A host carries the labels purpose=$purpose,lifecycle=temporary, and
# nothing here operates on one without them. It deletes itself $lifetime after
# it was created, whatever becomes of the shell that made it, and powers off
# after a day without a command from here (scripts/lib/gce-host-startup.sh).
#
# The sourcing script sets project, zone (the zone to try first), instance,
# machine, purpose, lifetime and results, and passes any of its own creation
# flags in create_flags.

cloud=(gcloud --quiet --project="$project")

# gce_create makes the host in $zone, or where that zone has no room for its
# machine, in the next zone of the same region that does.
gce_create() {
    local region candidates candidate log
    region=${zone%-*}
    candidates=("$zone")
    while read -r candidate; do
        [[ "$candidate" == "$zone" ]] || candidates+=("$candidate")
    done < <("${cloud[@]}" compute zones list --filter="region:$region" --format='value(name)' | sort)
    log=$(mktemp)
    for candidate in "${candidates[@]}"; do
        if gce_create_in "$candidate" 2> "$log"; then
            zone=$candidate
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
        --no-service-account --no-scopes \
        --metadata=block-project-ssh-keys=TRUE \
        --metadata-from-file="startup-script=$repo/scripts/lib/gce-host-startup.sh" \
        --labels="purpose=$purpose,lifecycle=temporary" \
        --maintenance-policy=TERMINATE --no-restart-on-failure \
        --max-run-duration="$lifetime" --instance-termination-action=DELETE
}

# gce_locate points zone at the zone the host is in, which gce_create may have
# chosen: every other action finds the host by its name.
gce_locate() {
    local found
    found=$("${cloud[@]}" compute instances list --filter="name=$instance" --format='value(zone.basename())')
    if [[ -n "$found" ]]; then zone=$found; fi
}

gce_exists() {
    [[ -n $("${cloud[@]}" compute instances list --filter="name=$instance" --format='value(name)') ]]
}

gce_check_owner() {
    local labels
    labels=$("${cloud[@]}" compute instances describe "$instance" --zone="$zone" \
        --format='value(labels.purpose,labels.lifecycle)')
    [[ "$labels" == "$purpose"$'\ttemporary' ]] || {
        echo "Refusing to operate on $instance, which lacks the $purpose labels." >&2
        return 1
    }
}

gce_delete() {
    if gce_exists; then
        gce_check_owner
        "${cloud[@]}" compute instances delete "$instance" --zone="$zone" --delete-disks=all
    fi
    if gce_exists; then
        echo "Instance still exists: $instance" >&2
        return 1
    fi
    [[ -z $("${cloud[@]}" compute disks list --filter="name=$instance" --format='value(name)') ]] || {
        echo "Boot disk still exists: $instance" >&2
        return 1
    }
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
# libkrun's included, without Git internals or build caches, to RUN/source on
# the host, and records what it copied and the host it copied it to. With
# FXDAX_GUEST_KERNEL naming fxdax's guest kernel here, it copies that to
# RUN/guest-kernel too (gce_guest_kernel_env).
gce_push_source() {
    local run=$1 staging
    staging=$(mktemp -d "${TMPDIR:-/tmp}/fxdax-gce-source.XXXXXX")
    python3 - "$repo" "$staging/source.tar.gz" <<'PY'
import io, pathlib, subprocess, sys, tarfile
root = pathlib.Path(sys.argv[1])
def files(directory):
    return subprocess.check_output(['git', '-C', str(directory), 'ls-files', '--cached', '--others', '--exclude-standard', '-z']).split(b'\0')
submodule = 'vendor/libkrun'
names = {name.decode() for name in files(root) if name} - {submodule}
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
    {
        echo "revision $(git -C "$repo" rev-parse HEAD)"
        echo "libkrun $(git -C "$repo/vendor/libkrun" rev-parse HEAD)"
        git -C "$repo" status --porcelain=v1 | sed 's/^/changed /'
        (cd "$staging" && shasum -a 256 source.tar.gz)
        if [[ -n ${FXDAX_GUEST_KERNEL:-} ]]; then
            echo "guest kernel $(shasum -a 256 < "$FXDAX_GUEST_KERNEL" | cut -d' ' -f1)"
        fi
    } > "$results/source.txt"
    "${cloud[@]}" compute instances describe "$instance" --zone="$zone" \
        --format='json(name,zone,machineType,cpuPlatform,scheduling,advancedMachineFeatures)' \
        > "$results/instance.json"
    "${cloud[@]}" compute scp --zone="$zone" "$staging/source.tar.gz" "$instance:source.tar.gz"
    rm -r -- "$staging"
    gce_remote "set -eu; mkdir -p $run/source $run/results; tar -xzf source.tar.gz -C $run/source"
    if [[ -n ${FXDAX_GUEST_KERNEL:-} ]]; then
        "${cloud[@]}" compute scp --zone="$zone" "$FXDAX_GUEST_KERNEL" "$instance:$run/guest-kernel"
    fi
}

# gce_guest_kernel_env prints the environment, for `sudo env` on the host, that
# names the guest kernel gce_push_source copied for RUN, if it copied one.
gce_guest_kernel_env() {
    if [[ -n ${FXDAX_GUEST_KERNEL:-} ]]; then
        echo "FXDAX_GUEST_KERNEL=\$HOME/$1/guest-kernel"
    fi
}

# gce_fetch copies RUN/results on the host into the results, as one archive:
# but for the guests' root filesystems (rootfs), which aren't results, and some
# of whose files (setuid binaries) no one may read.
gce_fetch() {
    local run=$1
    gce_remote "sudo chown -R \"\$(id -u):\$(id -g)\" $run/results &&
        tar -czf $run/results.tar.gz --exclude=rootfs -C $run/results ." >> "$results/remote.log" 2>&1
    "${cloud[@]}" compute scp --zone="$zone" "$instance:$run/results.tar.gz" "$results/results.tar.gz" &&
        tar -xzf "$results/results.tar.gz" -C "$results" && rm "$results/results.tar.gz"
}

# gce_all runs the given command on a new host and deletes the host whatever
# happens. The trap is set before creation, to cover ambiguous responses.
gce_all() {
    if gce_exists; then
        echo "Instance already exists: $instance" >&2
        exit 1
    fi
    trap 'status=$?; trap - EXIT; if ! gce_delete; then exit 1; fi; exit "$status"' EXIT
    gce_create
    "$@"
}
