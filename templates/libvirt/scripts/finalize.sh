#!/usr/bin/env bash
# Remove build identity and per-instance state, then power off the image.
set -Eeuo pipefail

[ "$(id -u)" -eq 0 ] || { echo "finalizer must run as root" >&2; exit 1; }
[ ! -e /tmp/flanforge-subordinate-ids.sh ] \
  || { echo "subordinate-ID build helper was retained" >&2; exit 1; }

rm -f /tmp/flanforge-guest-manifest.json
rm -f /etc/sudoers.d/90-cloud-init-users
find /home/packer/.ssh -mindepth 1 -maxdepth 1 -delete 2>/dev/null || true
passwd --lock packer >/dev/null 2>&1 || true
usermod --shell /usr/sbin/nologin packer

find /home/runner -type f \( -name '.bash_history' -o -name '.python_history' \) -delete
find /home/runner/.cache -mindepth 1 -delete 2>/dev/null || true
find /home/runner/.local/share/containers -mindepth 1 -delete 2>/dev/null || true
rm -f /root/.bash_history /root/.python_history

retained_login_marker=/usr/local/share/flanforge/tailscale-retained-login
retain_tailscale_login=false
if [ -f "${retained_login_marker}" ] && [ ! -L "${retained_login_marker}" ]; then
  retain_tailscale_login=true
fi

# Stopping the daemon first flushes any retained login to disk.
systemctl stop flanforge-tailscale-operator.service
systemctl stop tailscaled.service
for tailscale_dir in /var/lib/tailscale /var/cache/tailscale; do
  [ ! -L "${tailscale_dir}" ] \
    || { echo "Tailscale state directory must not be a symlink" >&2; exit 1; }
  # A build-time login lives in the identity directory and must survive; every
  # other build leaves the image without a tailnet identity.
  if [ "${retain_tailscale_login}" = true ] \
    && [ "${tailscale_dir}" = /var/lib/tailscale ]; then
    [ -n "$(find "${tailscale_dir}" -mindepth 1 -print -quit)" ] \
      || { echo "retained Tailscale login state is missing" >&2; exit 1; }
    continue
  fi
  if [ -d "${tailscale_dir}" ]; then
    find "${tailscale_dir}" -mindepth 1 -delete
    [ -z "$(find "${tailscale_dir}" -mindepth 1 -print -quit)" ]
  fi
done

cloud-init clean --logs --seed
find /etc/ssh -maxdepth 1 -type f -name 'ssh_host_*' -delete
rm -f /var/lib/dbus/machine-id
: > /etc/machine-id

dnf clean all
journalctl --rotate >/dev/null 2>&1 || true
journalctl --vacuum-time=1s >/dev/null 2>&1 || true
sync
systemctl poweroff
