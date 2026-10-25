# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""gcloud, as scripts/lib/gce.sh and the GCE drivers call it, for their tests (test_gce.py): a
project's hosts and their boot disks, kept in $FAKE_GCLOUD/state.json, and each call's words, with
the labels a create gave, appended to $FAKE_GCLOUD/calls.

state.json holds the hosts ("instances": each name's zone and labels), their boot disks ("disks"),
the zones ("zones", default us-east4-a alone, which a listing filtered by region gives those of),
those with no room for a host ("exhausted"), and the regions out of quota ("quota"), how the next create goes ("create": "made", the host made as asked; "taken", made at
once by another run, whose maker label is "winner", and which this create then finds there; or
"ambiguous", made as asked though the create reports an error), which listings and copies fail
("failing": "instances", "disks", "zones", "scp"), and the status SSH ends with ("ssh"). A host is
found by its zone, as gcloud's describe and delete find one, and by its name alone in a listing;
described as JSON, it is its name and zone. scp writes, where it copies to here, an archive of one
file, fetched, which names what it copied. A call this does not know fails, so that its test does
too.

Usage: python3 fake_gcloud.py ARG...
"""

import io
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


def find(instances, call, name, zone):
    """The host `name`, which `call` finds in `zone` alone, as gcloud does: else the call fails."""
    if instances.get(name, {}).get("zone") != zone:
        sys.exit(f"ERROR: (gcloud.compute.instances.{call}) {name} was not found in {zone}")
    return instances[name]


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
    elif words[:2] == ["compute", "scp"]:
        if "scp" in state.get("failing", []):
            sys.exit("ERROR: (gcloud.compute.scp) copying failed")
        if ":" not in words[3]:
            fetched = words[2].encode()
            entry = tarfile.TarInfo("fetched")
            entry.size = len(fetched)
            with tarfile.open(words[3], "w:gz") as archive:
                archive.addfile(entry, io.BytesIO(fetched))
    elif words[:2] == ["storage", "rm"]:
        pass
    elif command[2:] == ("list",) and command[1] in state.get("failing", []):
        sys.exit(f"ERROR: (gcloud.compute.{command[1]}.list) listing the {command[1]} failed")
    elif command == ("compute", "zones", "list"):
        region = flags.get("filter", "").removeprefix("region:")
        zones = state.get("zones", ["us-east4-a"])
        print("\n".join(zone for zone in zones if zone.rsplit("-", 1)[0] == region))
    elif command in [("compute", "instances", "list"), ("compute", "disks", "list")]:
        kind = command[1]
        name = flags["filter"].removeprefix("name=")
        if name in (instances if kind == "instances" else disks):
            values = {"name": name, "zone.basename()": instances.get(name, {}).get("zone")}
            print("\t".join(values[field] for field in fields(flags["format"])))
    elif command == ("compute", "instances", "describe"):
        found = find(instances, "describe", args[0], flags["zone"])
        if flags["format"].startswith("json("):
            print(json.dumps({"name": args[0], "zone": found["zone"]}))
        else:
            print("\t".join(found["labels"].get(field.removeprefix("labels."), "")
                            for field in fields(flags["format"])))
    elif command == ("compute", "instances", "create"):
        name = args[0]
        if name in instances:
            sys.exit(f"ERROR: (gcloud.compute.instances.create) {name} already exists")
        region = flags["zone"].rsplit("-", 1)[0]
        if region in state.get("quota", []):
            sys.exit(
                "ERROR: (gcloud.compute.instances.create) Could not fetch resource:\n"
                " - Quota 'SSD_TOTAL_GB' exceeded.  Limit: 500.0 in region "
                f"{region}."
            )
        if flags["zone"] in state.get("exhausted", []):
            sys.exit(
                f"ERROR: (gcloud.compute.instances.create) The zone '{flags['zone']}' does not "
                "have enough resources available to fulfill the request."
            )
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
        find(instances, "delete", name, flags["zone"])
        del instances[name]
        disks.remove(name)
        (STATE / "state.json").write_text(json.dumps(state))
    else:
        sys.exit(f"fake gcloud: no such call: {' '.join(argv)}")


if __name__ == "__main__":
    main(sys.argv[1:])
