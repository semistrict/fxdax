#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run the branches' tests against Azurite, Azure Blob Storage's emulator
# (github.com/Azure/Azurite, MIT), on this machine (TASK-46): start it in a
# container (podman, or docker), create a container of blobs with a request
# signed with the emulator's well-known key, run the Azure tests (ignored in the
# workspace's run), and stop it. Arguments are passed to cargo nextest.
#
# FXDAX_AZURITE_IMAGE names its image (by default Microsoft's,
# mcr.microsoft.com/azure-storage/azurite).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
image="${FXDAX_AZURITE_IMAGE:-mcr.microsoft.com/azure-storage/azurite}"
engine="$(command -v podman || command -v docker)" || {
    echo "neither podman nor docker is here to run Azurite" >&2
    exit 1
}

# On the emulator's own port, which clients configured for the emulator reach (object_store's
# AZURE_STORAGE_USE_EMULATOR names no other).
port=10000
# With FXDAX_AZURITE_DEBUG naming a directory, Azurite's debug log, of every request, goes there.
debug=()
if [[ -n ${FXDAX_AZURITE_DEBUG:-} ]]; then
    debug=(-v "$FXDAX_AZURITE_DEBUG:/debug")
fi
server="$("$engine" run -d --rm -p "127.0.0.1:$port:10000" ${debug[@]+"${debug[@]}"} "$image" \
    azurite-blob --blobHost 0.0.0.0 --blobPort 10000 --loose \
    ${FXDAX_AZURITE_DEBUG:+--debug /debug/azurite.log})"
trap '"$engine" stop "$server" >/dev/null' EXIT

# The emulator's account, whose key is public, and its endpoint.
account=devstoreaccount1
key='Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=='
endpoint="http://127.0.0.1:$port/$account"
export AZURE_STORAGE_USE_EMULATOR=true AZURE_STORAGE_ACCOUNT_NAME="$account"
export AZURE_STORAGE_ACCOUNT_KEY="$key"

# Up once it answers anything.
curl -s --retry 60 --retry-connrefused --retry-delay 1 -o /dev/null "$endpoint?comp=list"

# The container, made with a request signed as Shared Key authorization signs it.
container=fxdax
date="$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S GMT')"
version=2021-08-06
string="PUT\n\n\n\n\n\n\n\n\n\n\n\nx-ms-date:$date\nx-ms-version:$version\n/$account/$account/$container\nrestype:container"
hex_key="$(printf '%s' "$key" | base64 -d | od -An -vtx1 | tr -d ' \n')"
signature="$(printf '%b' "$string" | openssl dgst -sha256 -mac HMAC -macopt "hexkey:$hex_key" -binary | base64)"
curl -sf -X PUT -H "x-ms-date: $date" -H "x-ms-version: $version" -H 'Content-Length: 0' \
    -H "Authorization: SharedKey $account:$signature" "$endpoint/$container?restype=container"

FXDAX_AZURE_URL="az://$container/test" cargo nextest run --manifest-path "$root/Cargo.toml" \
    -p extent_remote --run-ignored only -E 'test(/azurite/)' "$@"
