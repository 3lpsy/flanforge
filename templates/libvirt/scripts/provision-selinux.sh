#!/usr/bin/env bash
# Disable SELinux in the image. The agent channel runs every guest command
# under qemu-ga's confined domain, which denies the readiness helper's tool
# probes (cloud-init and friends carry their own exec types) and would confine
# runuser, systemd-run, and rootless podman the same way. These guests are
# single-tenant, ephemeral CI machines; an image built without SELinux is
# skipped with a note rather than failed.
set -Eeuo pipefail

trap 'echo "SELinux provisioning failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

config="${FLANFORGE_SELINUX_CONFIG:-/etc/selinux/config}"

die() { echo "$1" >&2; exit 1; }

if [ ! -e "${config}" ]; then
  echo "==> no SELinux config at ${config}; nothing to disable"
  exit 0
fi
[ -f "${config}" ] && [ ! -L "${config}" ] \
  || die "SELinux config is not a regular file"

# One active assignment, rewritten in place; anything else is a config this
# script does not understand well enough to edit.
assignments="$(grep -Ec '^[[:space:]]*SELINUX=' "${config}" || true)"
[ "${assignments}" -eq 1 ] \
  || die "SELinux config must carry exactly one SELINUX= assignment"

rewritten="$(mktemp "${TMPDIR:-/tmp}/flanforge-selinux.XXXXXX")"
sed -E 's/^[[:space:]]*SELINUX=.*$/SELINUX=disabled/' "${config}" > "${rewritten}"
grep -Eq '^SELINUX=disabled$' "${rewritten}" \
  || die "SELinux config rewrite did not take"
sudo install -m 0644 -o root -g root "${rewritten}" "${config}"
rm -f "${rewritten}"

# The config governs the next boot; drop the running build to permissive so
# later provisioning already behaves like the image it is producing.
if command -v getenforce >/dev/null && [ "$(getenforce)" = Enforcing ]; then
  sudo setenforce 0
fi

echo "==> SELinux disabled for the image ($(getenforce 2>/dev/null || echo absent) until reboot)"
