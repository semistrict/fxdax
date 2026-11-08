# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""gcloud, as scripts/lib/gce.sh and the GCE drivers call it, for their tests (test_gce.py): a
project's hosts and their boot disks, kept in $FAKE_GCLOUD/state.json, and each call's words, with
the labels a create gave, appended to $FAKE_GCLOUD/calls.

state.json holds the hosts ("instances": each name's zone, labels and status, RUNNING unless it
says TERMINATED, as a stopped host is; its machine type, CPU platform and service account, if
given, "machine", "cpu" and "account"; and its schedule's end, "end", which set-scheduling clears,
and only on a stopped host), their boot disks ("disks"),
the zones ("zones", default us-east4-a alone, which a listing filtered by region gives those of),
those with no room for a host ("exhausted"), and the regions out of quota ("quota"), how the next create goes ("create": "made", the host made as asked; "taken", made at
once by another run, whose maker label is "winner", and which this create then finds there; or
"ambiguous", made as asked though the create reports an error; or a list of these, one a create,
the last for every create after), which listings and copies fail
("failing": "instances", "disks", "zones", "scp"), the status SSH ends with ("ssh"), and what it
prints ("ssh_out", nothing by default), or, call by call, until they run out, each SSH's status and
what it prints ("ssh_calls", pairs). With "ssh_run", each SSH runs its command too, in bash, on
the host it names, whose home is $FAKE_GCLOUD/hosts/NAME/home and whose /run, the tmpfs a stop
empties, is $FAKE_GCLOUD/hosts/NAME/run, sudo and setsid running what they're given, and the guest agent's
google_metadata_script_runner noting each script it runs in metadata-scripts-ran, and ending
with "metadata_scripts", 0 by default: its status
and output are the command's, unless "ssh_calls" says otherwise, as a connection dropped once it
ran; an SSH to a host not running fails, as a refused connection. A host's startup script has run
(/run/fxdax-host/ready) once it is made or started, or when it is first reached, if the state made
it. A host is found by its zone, as gcloud's describe, start, stop and delete find one, and by its
name alone in a listing, filtered by its name (name=NAME) or by its labels (labels.KEY=VALUE, AND
between them); described as JSON, it is its name and zone. A host is started only if its zone has
room ("exhausted"). scp writes, where it copies to here, an archive of one
file, fetched, which names what it copied; and keeps what it copies to a host in uploaded/, by its
path there, its slashes underscores. Cloud Storage's copies (`storage cp`, of a file, or with
--recursive, of a directory into one) go to and from its buckets, kept in $FAKE_GCLOUD/gs, and fail
if "storage" is failing. A call this does not know fails, so that its test does too.

Usage: python3 fake_gcloud.py ARG...
"""

import io
import json
import os
import re
import shutil
import subprocess
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


def bucket(path):
    """Where a path of a bucket's, gs://BUCKET/NAME, is kept: $FAKE_GCLOUD/gs/BUCKET/NAME; any
    other path is this machine's."""
    if path.startswith("gs://"):
        return STATE / "gs" / path.removeprefix("gs://")
    return Path(path)


def host_directory(name):
    """Where the host `name` keeps its files: home, its home, and run, its /run."""
    return STATE / "hosts" / name


def booted(name):
    """The host `name`'s startup script ran: its /run says it is ready."""
    (host_directory(name) / "run" / "fxdax-host").mkdir(parents=True, exist_ok=True)
    (host_directory(name) / "run" / "fxdax-host" / "ready").touch()


def run_on_host(name, command, agent=0):
    """Runs `command` as the host `name`'s shell would: in bash, in its home, where it starts, its
    /run its own,
    sudo and setsid running what they are given. Its status and output."""
    if not host_directory(name).exists():
        booted(name)
    home, tools = host_directory(name) / "home", STATE / "host-bin"
    home.mkdir(exist_ok=True)
    tools.mkdir(exist_ok=True)
    # /run, not a path within another, as $HOME/run-x is.
    command = re.sub(r"(?<![\w./$}-])/run/", f"{host_directory(name)}/run/", command)
    for tool in ("sudo", "setsid"):
        (tools / tool).write_text('#!/bin/sh\nexec "$@"\n')
        (tools / tool).chmod(0o755)
    # The guest agent's runner of the metadata's scripts: it says, in the host's home, it ran.
    runner = tools / "google_metadata_script_runner"
    runner.write_text(f'#!/bin/sh\necho "$@" >> "$HOME/metadata-scripts-ran"\nexit {agent}\n')
    runner.chmod(0o755)
    environment = {"HOME": str(home), "PATH": f"{tools}:/usr/bin:/bin"}
    done = subprocess.run(
        ["/bin/bash", "-c", command], env=environment, capture_output=True, text=True,
        stdin=subprocess.DEVNULL, cwd=home,
    )
    return done.returncode, done.stdout


def listed(instance, name, filter_):
    """Whether the host `name`, as `instance` has it, passes the listing's filter."""
    for term in filter_.split(" AND "):
        key, _, value = term.partition("=")
        if key == "name" and name != value:
            return False
        if key.startswith("labels.") and instance["labels"].get(key[len("labels."):]) != value:
            return False
    return True


def next_create(state):
    """How the next create goes (state.json's "create")."""
    how = state.get("create", "made")
    if isinstance(how, list):
        how = how.pop(0) if len(how) > 1 else how[0]
    return how


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
        status, out = state.get("ssh", 0), state.get("ssh_out", "")
        if state.get("ssh_run"):
            if instances.get(words[2], {}).get("status", "RUNNING") != "RUNNING":
                print(f"ssh: connect to host {words[2]} port 22: Connection refused", file=sys.stderr)
                sys.exit(255)
            status, out = run_on_host(words[2], flags["command"], state.get("metadata_scripts", 0))
        if state.get("ssh_calls"):
            status, out = state["ssh_calls"].pop(0)
            (STATE / "state.json").write_text(json.dumps(state))
        print(out, end="")
        sys.exit(status)
    elif words[:2] == ["compute", "scp"]:
        if "scp" in state.get("failing", []):
            sys.exit("ERROR: (gcloud.compute.scp) copying failed")
        if ":" not in words[3]:
            fetched = words[2].encode()
            entry = tarfile.TarInfo("fetched")
            entry.size = len(fetched)
            with tarfile.open(words[3], "w:gz") as archive:
                archive.addfile(entry, io.BytesIO(fetched))
        else:
            (STATE / "uploaded").mkdir(exist_ok=True)
            name = words[3].split(":", 1)[1].replace("/", "_")
            shutil.copyfile(words[2], STATE / "uploaded" / name)
    elif words[:2] == ["storage", "rm"]:
        pass
    elif words[:2] == ["storage", "cp"]:
        if "storage" in state.get("failing", []):
            sys.exit("ERROR: (gcloud.storage.cp) copying failed")
        source, target = (bucket(word) for word in words[2:4])
        if "recursive" in flags:
            shutil.copytree(source, target / source.name)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
    elif command[2:] == ("list",) and command[1] in state.get("failing", []):
        sys.exit(f"ERROR: (gcloud.compute.{command[1]}.list) listing the {command[1]} failed")
    elif command == ("compute", "zones", "list"):
        region = flags.get("filter", "").removeprefix("region:")
        zones = state.get("zones", ["us-east4-a"])
        print("\n".join(zone for zone in zones if zone.rsplit("-", 1)[0] == region))
    elif command == ("compute", "disks", "list"):
        name = flags["filter"].removeprefix("name=")
        if name in disks:
            print(name)
    elif command == ("compute", "instances", "list"):
        for name, instance in instances.items():
            if listed(instance, name, flags["filter"]):
                values = {
                    "name": name,
                    "zone.basename()": instance["zone"],
                    "status": instance.get("status", "RUNNING"),
                }
                print("\t".join(values[field] for field in fields(flags["format"])))
    elif command == ("compute", "instances", "describe"):
        found = find(instances, "describe", args[0], flags["zone"])
        if flags["format"].startswith("json("):
            print(json.dumps({"name": args[0], "zone": found["zone"]}))
        elif flags["format"].startswith("csv[no-heading]("):
            print(",".join([found.get("status", "RUNNING"), found.get("machine", ""),
                            found.get("cpu", ""), found.get("account", "")]))
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
        how = next_create(state)
        if how == "taken":
            labels["maker"] = "winner"
        instances[name] = {"zone": flags["zone"], "labels": labels, "status": "RUNNING"}
        disks.append(name)
        (STATE / "state.json").write_text(json.dumps(state))
        booted(name)
        if how == "taken":
            sys.exit(f"ERROR: (gcloud.compute.instances.create) {name} already exists")
        if how == "ambiguous":
            sys.exit("ERROR: (gcloud.compute.instances.create) the request timed out")
    elif command == ("compute", "instances", "start"):
        name = args[0]
        found = find(instances, "start", name, flags["zone"])
        if flags["zone"] in state.get("exhausted", []):
            sys.exit(
                f"ERROR: (gcloud.compute.instances.start) The zone '{flags['zone']}' does not "
                "have enough resources available to fulfill the request."
            )
        if found.get("status", "RUNNING") != "RUNNING":
            found["status"] = "RUNNING"
            (STATE / "state.json").write_text(json.dumps(state))
            booted(name)
    elif command == ("compute", "instances", "stop"):
        name = args[0]
        find(instances, "stop", name, flags["zone"])["status"] = "TERMINATED"
        (STATE / "state.json").write_text(json.dumps(state))
        # Its /run is a tmpfs, gone with the host's power.
        if (host_directory(name) / "run").exists():
            shutil.rmtree(host_directory(name) / "run")
    elif command == ("compute", "instances", "set-scheduling"):
        found = find(instances, "set-scheduling", args[0], flags["zone"])
        if found.get("status", "RUNNING") != "TERMINATED":
            sys.exit("ERROR: (gcloud.compute.instances.set-scheduling) Termination time cannot be "
                     "changed while the instance is running")
        if "clear-max-run-duration" in flags and "clear-instance-termination-action" in flags:
            found.pop("end", None)
        (STATE / "state.json").write_text(json.dumps(state))
    elif command == ("compute", "instances", "add-labels"):
        found = find(instances, "add-labels", args[0], flags["zone"])
        found["labels"].update(label.split("=") for label in flags["labels"].split(","))
        (STATE / "state.json").write_text(json.dumps(state))
    elif command == ("compute", "instances", "add-metadata"):
        find(instances, "add-metadata", args[0], flags["zone"])
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
