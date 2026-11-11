#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Installs Ubuntu's packages, the arguments, on a GitHub runner, past an outage of ubuntu.com's
# names: .github/actions/apt-install, whose action.yml says why (TASK-360). Its files are the
# runner's unless the tests name others (FXDAX_APT_MIRRORS, FXDAX_APT_SOURCES).
set -euo pipefail

mirrors=${FXDAX_APT_MIRRORS:-/etc/apt/apt-mirrors.txt}
sources=${FXDAX_APT_SOURCES:-/etc/apt/sources.list.d/ubuntu.sources}

# The image changed how apt finds its archive: mirrors listed where apt never looks protect nothing.
if ! grep -q "mirror+file:$mirrors" "$sources"; then
    echo "::error::the runner's apt doesn't fetch through $mirrors; .github/actions/apt-install's fallback mirrors don't apply"
    exit 1
fi

# The image's mirrors and the fallbacks, those whose names resolve, each tried in its priority's
# order: apt's mirror method tries each mirror for every file, lookups of a name that doesn't
# resolve timing out after 20 s each.
fallbacks=$(printf '%s\tpriority:%s\n' \
    https://mirrors.edge.kernel.org/ubuntu/ 10 \
    https://mirrors.mit.edu/ubuntu/ 11)
resolved=
while IFS= read -r line; do
    [ -n "$line" ] || continue
    host=${line#*://}
    host=${host%%/*}
    if getent ahosts "$host" > /dev/null; then
        resolved+="$line"$'\n'
    else
        echo "::warning::$host doesn't resolve: apt won't try it"
    fi
done < <(sort -u "$mirrors" <(echo "$fallbacks"))
if [ -z "$resolved" ]; then
    echo "::error::none of the Ubuntu mirrors' names resolve"
    exit 1
fi
printf '%s' "$resolved" | sudo tee "$mirrors"

# An index no mirror serves fails the update, not an install from indices stale or absent after it.
sudo apt-get update -o APT::Update::Error-Mode=any
sudo apt-get install -y "$@"
