#!/usr/bin/env bash
# Run scripts/test-e2e.sh on Linux inside a Lima VM (see scripts/lib/lima.sh).
# The logs stay in the VM, under the directory the script prints.
exec "$(dirname "$0")/lib/lima.sh" test-e2e.sh "$@"
