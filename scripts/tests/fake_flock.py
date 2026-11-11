# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""flock(1), as scripts/lib/host-remote.sh calls it, for hosts without util-linux's (fake_ssh.py):
`flock [-n] [-u] FD` locks, or with -u unlocks, the file its caller has open as FD, the lock its
caller's as long as it keeps the file open; `flock [-n] FILE COMMAND...` runs COMMAND holding a lock
of FILE, made if it is not there. -n fails, with 1, rather than wait for another's lock.

Usage: python3 fake_flock.py [-n] [-u] FD | FILE COMMAND...
"""

import fcntl
import os
import subprocess
import sys


def main(argv):
    blocking, unlock = True, False
    while argv and argv[0].startswith("-"):
        blocking &= argv[0] != "-n"
        unlock |= argv[0] == "-u"
        argv = argv[1:]
    target, command = argv[0], argv[1:]
    if target.isdigit() and not command:
        fd = int(target)
    else:
        fd = os.open(target, os.O_RDONLY | os.O_CREAT, 0o644)
    operation = fcntl.LOCK_UN if unlock else fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB)
    try:
        fcntl.flock(fd, operation)
    except BlockingIOError:
        return 1
    return subprocess.run(command).returncode if command else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
