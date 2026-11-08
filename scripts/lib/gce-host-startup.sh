#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A kept GCE host's startup script (scripts/lib/gce.sh), run each time it
# starts: installs an inactivity lease, which every command from
# scripts/lib/gce.sh renews, and powers the host off, which stops it, once
# nothing has renewed it for two hours and no job of a run's runs, should the run
# that held it not stop it, as one killed doesn't. A job runs on with its waiter
# gone, for a resume to fetch (gce_remote_detached).
set -euo pipefail

install -d -m 0755 /var/lib/fxdax-host
# Renewed as the host starts: the lease of its last run is hours or days old.
touch /var/lib/fxdax-host/lease
cat > /usr/local/sbin/fxdax-host-expire <<'CHECK'
#!/usr/bin/env bash
set -euo pipefail
lease=/var/lib/fxdax-host/lease
modified=0
if [[ -f "$lease" ]]; then modified=$(stat -c %Y "$lease"); fi
if (( $(date +%s) - modified < 7200 )); then exit 0; fi
# A job still running, its status not yet written (gce_remote_detached).
for job in /home/*/*.job; do
    if [[ -f $job/pid && ! -e $job/status ]] && kill -0 "$(< "$job/pid")" 2> /dev/null; then
        exit 0
    fi
done
logger -t fxdax-host 'two hours without commands or jobs; powering off'
exec systemctl poweroff
CHECK
chmod 0755 /usr/local/sbin/fxdax-host-expire
cat > /etc/systemd/system/fxdax-host-expire.service <<'UNIT'
[Unit]
Description=Power off an abandoned fxdax host
[Service]
Type=oneshot
ExecStart=/usr/local/sbin/fxdax-host-expire
UNIT
cat > /etc/systemd/system/fxdax-host-expire.timer <<'UNIT'
[Unit]
Description=Check the fxdax host's inactivity lease
[Timer]
OnBootSec=1min
OnUnitActiveSec=1min
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload
systemctl enable --now fxdax-host-expire.timer
# Nothing restarts the host's services under a run: needrestart only lists what
# would need it, and no daily upgrade runs: a host's runs install what they need.
# needrestart restarted sshd and re-executed systemd after a run's packages went
# in, and the run's job died with no status 11 s later (GCE r990, 2026-11-08).
install -d -m 0755 /etc/needrestart/conf.d
echo "\$nrconf{restart} = 'l';" > /etc/needrestart/conf.d/fxdax.conf
systemctl disable --now apt-daily.timer apt-daily-upgrade.timer unattended-upgrades.service \
    2> /dev/null || true
# Ready since this start: /run is a tmpfs, which a stop empties (gce_wait_ready).
install -d -m 0755 /run/fxdax-host
touch /run/fxdax-host/ready
