# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""gcloud, as scripts/lib/gce.sh and the GCE drivers call it, for their tests (test_gce.py): a
project's hosts and their boot disks, kept in $FAKE_GCLOUD/state.json, and each call's words, with
the labels a create gave, appended to $FAKE_GCLOUD/calls.

state.json holds the hosts ("instances": each name's zone and labels), their boot disks ("disks"),
how the next create goes ("create": "made", the host made as asked; "taken", made at once by
another run, whose maker label is "winner", and which this create then finds there; or
"ambiguous", made as asked though the create reports an error), which listings fail ("failing":
"instances", "disks"), and the status SSH ends with ("ssh"). scp writes an empty archive where it
copies to here. A call this does not know fails, so that its test does too.

Usage: python3 fake_gcloud.py ARG...
"""

import json
import os
import re
import sys
import tarfile
from pathlib import Path

STATE = Path(os.environ["FAKE_GCLOUD"])


def fields(format_):
    """The fields of a `value(...)` format, in its order."""
    return re.fullmatch(r"value\((.*)\)", format_).group(1).split(",")


def main(argv):
    words = [arg for arg in argv if not arg.startswith("--")]
    flags = dict(arg[2:].partition("=")[::2] for arg in argv if arg.startswith("--"))
    with open(STATE / "calls", "a") as calls:
        labels = [f"--labels={flags['labels']}"] if "labels" in flags else []
        print(" ".join(words + labels), file=calls)
    state = json.loads((STATE / "state.json").read_text())
    instances, disks = state.setdefault("instances", {}), state.setdefault("disks", [])
    command, args = tuple(words[:3]), words[3:]
    if words[:2] == ["compute", "ssh"]:
        sys.exit(state.get("ssh", 0))
    elif words[:2] == ["compute", "scp"] and ":" not in words[3]:
        with tarfile.open(words[3], "w:gz"):
            pass
    elif words[:2] == ["storage", "rm"]:
        pass
    elif command == ("compute", "zones", "list"):
        print("us-east4-a")
    elif command in [("compute", "instances", "list"), ("compute", "disks", "list")]:
        kind = command[1]
        if kind in state.get("failing", []):
            sys.exit(f"ERROR: (gcloud.compute.{kind}.list) listing the {kind} failed")
        name = flags["filter"].removeprefix("name=")
        if name in (instances if kind == "instances" else disks):
            values = {"name": name, "zone.basename()": instances.get(name, {}).get("zone")}
            print("\t".join(values[field] for field in fields(flags["format"])))
    elif command == ("compute", "instances", "describe"):
        name = args[0]
        if name not in instances:
            sys.exit(f"ERROR: (gcloud.compute.instances.describe) {name} was not found")
        labels = instances[name]["labels"]
        print("\t".join(labels.get(field.removeprefix("labels."), "")
                        for field in fields(flags["format"])))
    elif command == ("compute", "instances", "create"):
        name = args[0]
        if name in instances:
            sys.exit(f"ERROR: (gcloud.compute.instances.create) {name} already exists")
        labels = dict(label.split("=") for label in flags["labels"].split(","))
        how = state.get("create", "made")
        if how == "taken":
            labels["maker"] = "winner"
        instances[name] = {"zone": flags["zone"], "labels": labels}
        disks.append(name)
        (STATE / "state.json").write_text(json.dumps(state))
        if how == "taken":
            sys.exit(f"ERROR: (gcloud.compute.instances.create) {name} already exists")
        if how == "ambiguous":
            sys.exit("ERROR: (gcloud.compute.instances.create) the request timed out")
    elif command == ("compute", "instances", "delete"):
        name = args[0]
        del instances[name]
        disks.remove(name)
        (STATE / "state.json").write_text(json.dumps(state))
    else:
        sys.exit(f"fake gcloud: no such call: {' '.join(argv)}")


if __name__ == "__main__":
    main(sys.argv[1:])
