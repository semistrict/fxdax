# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Sourced by the scripts that run servers their tests need in containers (scripts/test-rustfs.sh,
# scripts/test-azurite.sh): which container engine to run them with.

# container_engine prints the path of the container engine that answers here: FXDAX_CONTAINER_ENGINE
# if set, else docker's or podman's, whichever runs, docker's first (TASK-149.20: one installed but
# not running, as a podman whose machine is stopped, was taken, and failed).
container_engine() {
    if [[ -n ${FXDAX_CONTAINER_ENGINE:-} ]]; then
        command -v "$FXDAX_CONTAINER_ENGINE"
        return
    fi
    local engine
    for engine in docker podman; do
        if command -v "$engine" > /dev/null && "$engine" info > /dev/null 2>&1; then
            command -v "$engine"
            return
        fi
    done
    echo "neither docker nor podman runs here: start one, or set FXDAX_CONTAINER_ENGINE" >&2
    return 1
}

# HEALTH_EVERY is how often a container's health check runs, as healthy_container starts it: what
# its readiness is known within, the engine running the check inside the container.
HEALTH_EVERY=1s

# healthy_container ENGINE HEALTH RUN_ARG... starts a container detached, as `ENGINE run -d --rm
# RUN_ARG...` does, with HEALTH, a shell command run inside it, as its health check: the server's
# own word that it is ready, asked in its own network. It prints the container's ID once the engine
# says it is healthy, waiting for that event, at most HEALTHY_WITHIN seconds (120): not probing the
# server from here, through the engine's port forwarding, which takes connections before the
# server does and cuts them short, nor retrying what failed (TASK-239: curl retrying a transfer it
# had written part of into /dev/null, whose truncation fails on Linux, exited 23). A container
# that doesn't get healthy in time is stopped, its log said, and it fails.
healthy_container() {
    local engine=$1 health=$2 since id status=""
    shift 2
    since=$(date +%s)
    id=$("$engine" run -d --rm --health-cmd "$health" --health-interval "$HEALTH_EVERY" \
        --health-retries 120 "$@") || return
    if [[ $(basename "$engine") == podman ]]; then
        timeout "${HEALTHY_WITHIN:-120}" "$engine" wait --condition=healthy "$id" > /dev/null &&
            status=healthy
    else
        # The container's health events since before it started, as they come, none missed,
        # through a FIFO: no coproc in macOS's bash 3.2.
        local events fifo action
        fifo=$(mktemp -u)
        mkfifo "$fifo"
        "$engine" events --since "$since" --filter "container=$id" \
            --filter event=health_status --format '{{.Action}}' > "$fifo" &
        events=$!
        exec 3< "$fifo"
        rm "$fifo"
        while read -r -t "${HEALTHY_WITHIN:-120}" action <&3; do
            if [[ $action == "health_status: healthy" ]]; then
                status=healthy
                break
            fi
        done
        exec 3<&-
        kill "$events" 2> /dev/null
        wait "$events" 2> /dev/null
    fi
    if [[ $status != healthy ]]; then
        echo "container $id didn't say it was healthy within ${HEALTHY_WITHIN:-120} s:" >&2
        "$engine" logs "$id" >&2
        "$engine" stop "$id" > /dev/null
        return 1
    fi
    echo "$id"
}
