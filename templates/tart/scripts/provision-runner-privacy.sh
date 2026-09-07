#!/bin/bash
# Give the job account the privacy grants the base image gives only to its own
# bootstrap account. Optional: set PRIVACY_GRANTS_ENABLED=false, or delete this
# script and its line in flanforge-base.pkr.hcl, to ship without them.
set -euo pipefail

case "${PRIVACY_GRANTS_ENABLED:-true}" in
  false)
    echo "==> runner privacy grants disabled"
    exit 0
    ;;
  true) ;;
  *)
    echo "PRIVACY_GRANTS_ENABLED must be true or false" >&2
    exit 1
    ;;
esac

bootstrap_home="${HOME:-}"
[[ "${bootstrap_home}" = /* ]] \
  || { echo "provisioning account has no absolute home directory" >&2; exit 1; }

runner_home="$(dscl . -read /Users/runner NFSHomeDirectory | awk '{ print $2 }')"
[ "${runner_home}" = /Users/runner ] \
  || { echo "runner home must be /Users/runner" >&2; exit 1; }

command -v sqlite3 >/dev/null 2>&1 \
  || { echo "sqlite3 is required to seed privacy grants" >&2; exit 1; }

tcc_directory="Library/Application Support/com.apple.TCC"
runner_tcc_dir="${runner_home}/${tcc_directory}"
runner_tcc_db="${runner_tcc_dir}/TCC.db"
bootstrap_tcc_db="${bootstrap_home}/${tcc_directory}/TCC.db"

# A user-level privacy database is created by that account's own tccd, which
# never runs for an account with no session. Seed runner's from the bootstrap
# account's, so the schema is exactly the one this macOS build wrote, then keep
# only the rows below. `.backup` is consistent against a live write-ahead log.
if ! sudo test -f "${runner_tcc_db}"; then
  sudo test -f "${bootstrap_tcc_db}" \
    || { echo "no user privacy database to copy the current schema from" >&2; exit 1; }
  echo "==> seeding a user privacy database for runner"
  sudo install -d -m 0700 -o runner -g staff "${runner_tcc_dir}"
  sudo sqlite3 "${bootstrap_tcc_db}" ".backup '${runner_tcc_db}'"
  sudo sqlite3 "${runner_tcc_db}" 'DELETE FROM access;'
fi

# `sshd-keygen-wrapper` is the responsible process for everything a job runs
# over SSH; `osascript` and Python are how those jobs drive other applications.
# System-level grants are already global, so these cover the per-user database.
echo "==> granting runner accessibility, screen recording, event post, Apple events"
sudo sqlite3 "${runner_tcc_db}" <<'RUNNER_PRIVACY_GRANTS'
INSERT OR REPLACE
INTO access (
  service,
  client_type,
  client,
  auth_value,
  auth_reason,
  auth_version,
  indirect_object_identifier_type,
  indirect_object_identifier
) VALUES
-- Accessibility: read and drive another application's UI elements.
('kTCCServiceAccessibility', 1, '/usr/libexec/sshd-keygen-wrapper', 2, 0, 1, NULL, 'UNUSED'),
('kTCCServiceAccessibility', 1, '/usr/bin/osascript', 2, 0, 1, NULL, 'UNUSED'),
('kTCCServiceAccessibility', 0, 'org.python.python', 2, 0, 1, NULL, 'UNUSED'),
-- Screen recording: host-side `screencapture` and simulator video capture.
('kTCCServiceScreenCapture', 1, '/usr/libexec/sshd-keygen-wrapper', 2, 0, 1, NULL, 'UNUSED'),
('kTCCServiceScreenCapture', 1, '/usr/bin/osascript', 2, 0, 1, NULL, 'UNUSED'),
('kTCCServiceScreenCapture', 0, 'org.python.python', 2, 0, 1, NULL, 'UNUSED'),
-- Post event: synthesize keyboard and pointer input.
('kTCCServicePostEvent', 1, '/usr/libexec/sshd-keygen-wrapper', 2, 0, 1, NULL, 'UNUSED'),
('kTCCServicePostEvent', 1, '/usr/bin/osascript', 2, 0, 1, NULL, 'UNUSED'),
('kTCCServicePostEvent', 0, 'org.python.python', 2, 0, 1, NULL, 'UNUSED'),
-- Apple events: script System Events, which is how UI automation is driven.
('kTCCServiceAppleEvents', 1, '/usr/libexec/sshd-keygen-wrapper', 2, 0, 1, 0, 'com.apple.systemevents'),
('kTCCServiceAppleEvents', 1, '/usr/bin/osascript', 2, 0, 1, 0, 'com.apple.systemevents');
RUNNER_PRIVACY_GRANTS

# Root wrote the database, and may have left write-ahead sidecars behind.
sudo chmod 0600 "${runner_tcc_db}"
for sidecar in "${runner_tcc_db}" "${runner_tcc_db}-wal" "${runner_tcc_db}-shm"; do
  if sudo test -e "${sidecar}"; then
    sudo chown runner:staff "${sidecar}"
  fi
done

granted_services="$(sudo sqlite3 "${runner_tcc_db}" \
  'SELECT DISTINCT service FROM access WHERE auth_value = 2;')"
for expected_service in kTCCServiceAccessibility kTCCServiceAppleEvents \
  kTCCServicePostEvent kTCCServiceScreenCapture; do
  printf '%s\n' "${granted_services}" | grep -Fxq "${expected_service}" \
    || { echo "runner privacy grant ${expected_service} was not stored" >&2; exit 1; }
done
echo "==> runner privacy grants stored"
