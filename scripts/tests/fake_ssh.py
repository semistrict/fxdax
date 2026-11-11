# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""ssh, as scripts/lib/host.sh calls it, for its tests (test_host.py): hosts kept in
$FAKE_SSH/hosts/NAME, each a home, where each call's command runs, in bash, with the call's
standard input, and a PATH whose first directory, $FAKE_SSH/host-bin, holds the host's tools that
this machine lacks or that a test watches: sudo running what it is given; setsid running it in a
session of its own; fxjob recording its options in the home's fxjob-calls and running its command;
timeout running its command; and flock, fcntl's locks. Each call's host and command, a line, are
appended to $FAKE_SSH/calls.

$FAKE_SSH/state.json says how calls go: "down", the hosts that refuse connections, every call to
one ending 255; and "calls", call by call until they run out, what each does instead of running
alone: {"status": N} runs it and ends N, its output kept, as a connection dropped once it ran
does; {"fail": N} runs nothing and ends N; null runs it. Git on a host runs as this machine's,
copying from local paths (protocol.file.allow).

Usage: python3 fake_ssh.py [-o OPTION]... HOST COMMAND
"""

import json
import os
import subprocess
import sys
from pathlib import Path

STATE = Path(os.environ["FAKE_SSH"])
TOOLS = {
    "sudo": '#!/bin/sh\nexec "$@"\n',
    "setsid": f'#!/bin/sh\nexec {sys.executable} -c '
              '\'import os, sys; os.setsid(); os.execvp(sys.argv[1], sys.argv[1:])\' "$@"\n',
    "fxjob": '#!/bin/bash\necho "$*" >> "$HOME/fxjob-calls"\n'
             'while [[ $1 == -* ]]; do case $1 in --) shift; break ;; --bench) shift ;; '
             '*) shift 2 ;; esac; done\nexec "$@"\n',
    "timeout": '#!/bin/bash\nwhile [[ $1 == -* ]]; do shift; done\nshift\nexec "$@"\n',
    "flock": f'#!/bin/sh\nexec {sys.executable} {Path(__file__).with_name("fake_flock.py")} "$@"\n',
}


def tools():
    """The host's tools' directory, made."""
    directory = STATE / "host-bin"
    directory.mkdir(exist_ok=True)
    for name, text in TOOLS.items():
        (directory / name).write_text(text)
        (directory / name).chmod(0o755)
    return directory


def home(host):
    """The home of `host`, made."""
    directory = STATE / "hosts" / host / "home"
    directory.mkdir(parents=True, exist_ok=True)
    return directory


def main(argv):
    while argv and argv[0] == "-o":
        argv = argv[2:]
    host, command = argv[0], " ".join(argv[1:])
    with open(STATE / "calls", "a") as calls:
        print(f"{host} {command}", file=calls)
    state = json.loads((STATE / "state.json").read_text())
    if host in state.get("down", []):
        print(f"ssh: connect to host {host} port 22: Connection refused", file=sys.stderr)
        return 255
    how = None
    if state.get("calls"):
        how = state["calls"].pop(0)
        (STATE / "state.json").write_text(json.dumps(state))
    if how and "fail" in how:
        return how["fail"]
    environment = {
        "HOME": str(home(host)),
        "PATH": f"{tools()}:/usr/bin:/bin:/usr/sbin:/sbin",
        "GIT_CONFIG_COUNT": "1",
        "GIT_CONFIG_KEY_0": "protocol.file.allow",
        "GIT_CONFIG_VALUE_0": "always",
    }
    done = subprocess.run(["/bin/bash", "-c", command], env=environment, cwd=home(host))
    return how["status"] if how else done.returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
