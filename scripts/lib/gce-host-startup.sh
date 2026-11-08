#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A disposable GCE host's startup script (scripts/lib/gce.sh): installs an
# inactivity lease, which every command from scripts/lib/gce.sh renews, and
# powers the host off once nothing has renewed it for a day, should its own
# deletion fail.
set -euo pipefail

install -d -m 0755 /var/lib/fxdax-host
if [[ ! -e /var/lib/fxdax-host/lease ]]; then
    touch /var/lib/fxdax-host/lease
fi
cat > /usr/local/sbin/fxdax-host-expire <<'CHECK'
#!/usr/bin/env bash
set -euo pipefail
lease=/var/lib/fxdax-host/lease
modified=0
if [[ -f "$lease" ]]; then modified=$(stat -c %Y "$lease"); fi
if (( $(date +%s) - modified < 86400 )); then exit 0; fi
logger -t fxdax-host 'a day without commands; powering off'
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
# would need it, and no daily upgrade runs on a host that lives a few hours.
# needrestart restarted sshd and re-executed systemd after a run's packages went
# in, and the run's job died with no status 11 s later (GCE r990, 2026-11-08).
install -d -m 0755 /etc/needrestart/conf.d
echo "\$nrconf{restart} = 'l';" > /etc/needrestart/conf.d/fxdax.conf
systemctl disable --now apt-daily.timer apt-daily-upgrade.timer unattended-upgrades.service \
    2> /dev/null || true
touch /var/lib/fxdax-host/ready
