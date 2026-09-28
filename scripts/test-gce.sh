#!/usr/bin/env bash
# Qualify fxdax on a disposable x86-64 GCE host with nested KVM. It runs the
# workspace's tests and libkrun's virtio-fs guest test, whose DAX window takes
# KVM's faults through userfaultfd. `all` always deletes its host; `create`,
# `run` and `delete` expose the same steps for interrupted runs.
#
# Usage: test-gce.sh [all|create|run|delete] [instance] [results-directory]
#
# FXDAX_GCE_PROJECT is the project (default: gcloud's), FXDAX_GCE_ZONE the zone
# to try first (default: us-east4-a), and FXDAX_E2E_TEST a libkrun test case to
# run instead of scripts/test-e2e.sh's (`all` runs every one).
set -euo pipefail
[[ ${FXDAX_E2E_TEST:-} =~ ^[a-z0-9-]*$ ]] || { echo "FXDAX_E2E_TEST is a test case name" >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
zone=${FXDAX_GCE_ZONE:-us-east4-a}
action=${1:-all}
instance=${2:-fxdax-test-$(date -u +%Y%m%d-%H%M%S)}
results=${3:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-gce.XXXXXX")}
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$instance" == fxdax-test-* ]] || { echo 'Use an fxdax-test- instance name.' >&2; exit 2; }
cloud=(gcloud --quiet --project="$project")
# The host deletes itself after this long, whatever becomes of this shell. The
# run's own limit leaves time to copy the results back.
lifetime=8h limit=6h

# create makes the host in FXDAX_GCE_ZONE, or where that zone has no room for
# its machine, in the next zone of the same region that does.
create() {
    local region candidates candidate log
    region=${zone%-*}
    candidates=("$zone")
    while read -r candidate; do
        [[ "$candidate" == "$zone" ]] || candidates+=("$candidate")
    done < <("${cloud[@]}" compute zones list --filter="region:$region" --format='value(name)' | sort)
    log=$(mktemp)
    for candidate in "${candidates[@]}"; do
        if create_in "$candidate" 2> "$log"; then
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

create_in() {
    "${cloud[@]}" compute instances create "$instance" --zone="$1" \
        --machine-type=n2-standard-8 \
        --min-cpu-platform='Intel Cascade Lake' --enable-nested-virtualization \
        --image=ubuntu-2604-resolute-amd64-v20260918 --image-project=ubuntu-os-cloud \
        --boot-disk-size=200GB --boot-disk-type=pd-balanced --boot-disk-auto-delete \
        --no-service-account --no-scopes \
        --metadata=block-project-ssh-keys=TRUE \
        --labels=purpose=fxdax-test,lifecycle=temporary \
        --maintenance-policy=TERMINATE --no-restart-on-failure \
        --max-run-duration="$lifetime" --instance-termination-action=DELETE
}

# locate points zone at the zone the instance is in, which create may have
# chosen: every other action finds the host by its name.
locate() {
    local found
    found=$("${cloud[@]}" compute instances list --filter="name=$instance" --format='value(zone.basename())')
    if [[ -n "$found" ]]; then zone=$found; fi
}

check_owner() {
    local labels
    labels=$("${cloud[@]}" compute instances describe "$instance" --zone="$zone" \
        --format='value(labels.purpose,labels.lifecycle)')
    [[ "$labels" == $'fxdax-test\ttemporary' ]] || {
        echo "Refusing to operate on an instance without the fxdax test labels." >&2
        return 1
    }
}

delete() {
    local found
    found=$("${cloud[@]}" compute instances list --filter="name=$instance" --format='value(name)')
    if [[ -n "$found" ]]; then
        check_owner
        "${cloud[@]}" compute instances delete "$instance" --zone="$zone" --delete-disks=all
    fi
    found=$("${cloud[@]}" compute instances list --filter="name=$instance" --format='value(name)')
    [[ -z "$found" ]] || { echo "Instance still exists: $instance" >&2; return 1; }
    found=$("${cloud[@]}" compute disks list --filter="name=$instance" --format='value(name)')
    [[ -z "$found" ]] || { echo "Boot disk still exists: $instance" >&2; return 1; }
    echo "Verified deletion of $instance and its boot disk." >&2
}

run() {
    check_owner
    mkdir -p "$results"
    results=$(cd "$results" && pwd)
    echo "Results: $results" >&2
    local ready=false
    for _ in {1..30}; do
        if "${cloud[@]}" compute ssh "$instance" --zone="$zone" \
            --ssh-flag='-o ConnectTimeout=10' --command=true \
            >> "$results/startup.log" 2>&1; then
            ready=true
            break
        fi
        sleep 10
    done
    "$ready" || { echo 'The host did not accept SSH in time.' >&2; return 1; }
    local staging
    staging=$(mktemp -d "${TMPDIR:-/tmp}/fxdax-gce-source.XXXXXX")
    # Only versioned and non-ignored source, including libkrun's, without Git
    # internals or build caches.
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
    (cd "$staging" && shasum -a 256 source.tar.gz) > "$results/source-archive.sha256"
    "${cloud[@]}" compute instances describe "$instance" --zone="$zone" \
        --format='json(name,zone,machineType,cpuPlatform,scheduling,advancedMachineFeatures)' \
        > "$results/instance.json"
    "${cloud[@]}" compute scp --zone="$zone" "$staging/source.tar.gz" "$instance:source.tar.gz"
    rm -f -- "$staging/source.tar.gz"
    rmdir -- "$staging"
    local status=0 run
    # Each run unpacks into a directory of its own, so that no earlier run's
    # files or recorded times carry over.
    run="run-$(date -u +%Y%m%dT%H%M%SZ)"
    "${cloud[@]}" compute ssh "$instance" --zone="$zone" --command='set -eu
        mkdir -p '"$run"'/source '"$run"'/results
        tar -xzf source.tar.gz -C '"$run"'/source
        sudo timeout --signal=TERM --kill-after=30s '"$limit"' bash '"$run"'/source/scripts/lib/test-linux-host.sh '"$run"'/source '"$run"'/results '"${FXDAX_E2E_TEST:-}"'
        ' > "$results/remote.log" 2>&1 || status=$?
    "${cloud[@]}" compute ssh "$instance" --zone="$zone" \
        --command="sudo chown -R \"\$(id -u):\$(id -g)\" $run/results" >> "$results/remote.log" 2>&1 || status=$?
    "${cloud[@]}" compute scp --recurse --zone="$zone" "$instance:$run/results/." "$results/" || status=$?
    tail -n 4 "$results/remote.log" >&2
    echo "Results: $results" >&2
    return "$status"
}

case "$action" in
    create) create ;;
    run) locate; run ;;
    delete) locate; delete ;;
    all)
        existing=$("${cloud[@]}" compute instances list --filter="name=$instance" --format='value(name)')
        [[ -z "$existing" ]] || { echo "Instance already exists: $instance" >&2; exit 1; }
        # The host's own lifetime also covers this shell being killed. Set the
        # trap before creation to cover ambiguous create responses.
        trap 'status=$?; trap - EXIT; if ! delete; then exit 1; fi; exit "$status"' EXIT
        create
        run
        ;;
    *) echo "Usage: $0 [all|create|run|delete] [instance] [results-directory]" >&2; exit 2 ;;
esac
