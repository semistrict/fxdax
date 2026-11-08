# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A container engine, docker's commands as scripts/lib/containers.sh calls them, for its tests
(test_containers.py): each call's words appended to $FAKE_ENGINE/calls, a line each, and `run -d`
printing the container's ID, `container-1`. `events` prints the lines of $FAKE_ENGINE/events, the
container's health events as docker formats their Action, then stays, as docker's stream does,
until it is killed. `logs` prints "the server's log"; `stop` and `info` print nothing. A command
this doesn't know fails, so that its test does too.

Usage: python3 fake_container_engine.py ARG...
"""

import os
import signal
import sys
from pathlib import Path

STATE = Path(os.environ["FAKE_ENGINE"])


def main(args):
    with open(STATE / "calls", "a") as calls:
        print(" ".join(args), file=calls)
    command = args[0]
    if command == "run":
        print("container-1")
    elif command == "events":
        events = STATE / "events"
        if events.exists():
            sys.stdout.write(events.read_text())
            sys.stdout.flush()
        signal.pause()
    elif command == "logs":
        print("the server's log")
    elif command in ("stop", "info"):
        pass
    else:
        sys.exit(f"fake engine: unknown command {args}")


if __name__ == "__main__":
    main(sys.argv[1:])
