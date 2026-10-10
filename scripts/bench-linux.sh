#!/usr/bin/env bash
# Run scripts/bench.sh on Linux inside a Lima VM (see scripts/lib/lima.sh).
# The results stay in the VM, under the directory the script prints.
exec "$(dirname "$0")/lib/lima.sh" bench.sh "$@"
