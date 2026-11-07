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
