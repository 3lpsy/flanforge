#!/bin/bash
# Optionally move one Simulator's first-start work into the template.
set -euo pipefail

case "${PREWARM_ENABLED:-true}" in
  false)
    echo "==> simulator prewarm disabled"
    exit 0
    ;;
  true) ;;
  *)
    echo "PREWARM_ENABLED must be true or false" >&2
    exit 1
    ;;
esac

prewarm_target="${PREWARM_TARGET:-iPhone 17 Pro}"
[[ "${prewarm_target}" =~ ^[A-Za-z0-9][A-Za-z0-9._()\ -]{0,79}$ ]] \
  || { echo "PREWARM_TARGET contains unsupported characters" >&2; exit 1; }

sudo -H -u runner env \
  HOME=/Users/runner \
  PREWARM_TARGET="${prewarm_target}" \
  /bin/bash <<'RUNNER_SIMULATOR'
set -euo pipefail

prewarm_target="${PREWARM_TARGET}"
echo "==> resolving simulator '${prewarm_target}'"
device_ids="$(PREWARM_TARGET="${prewarm_target}" \
  xcrun simctl list --json devices available \
  | PREWARM_TARGET="${prewarm_target}" python3 -c '
import json
import os
import sys

target = os.environ["PREWARM_TARGET"]
inventory = json.load(sys.stdin)
identifiers = [
    device["udid"]
    for devices in inventory.get("devices", {}).values()
    for device in devices
    if device.get("isAvailable", False) and device.get("name") == target
]
sys.stdout.write("\n".join(identifiers))
')"
device_count="$(printf '%s\n' "${device_ids}" \
  | awk 'NF { count += 1 } END { print count + 0 }')"
[ "${device_count}" -eq 1 ] \
  || { echo "expected exactly one available simulator named '${prewarm_target}'" >&2; exit 1; }
[[ "${device_ids}" =~ ^[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}$ ]] \
  || { echo "simulator returned an invalid device identifier" >&2; exit 1; }

simulator_may_be_running=true
shutdown_simulator() {
  if [ "${simulator_may_be_running}" = true ]; then
    xcrun simctl shutdown "${device_ids}" >/dev/null 2>&1 || true
  fi
}
trap shutdown_simulator EXIT INT TERM

echo "==> prewarming ${prewarm_target} (${device_ids})"
xcrun simctl bootstatus "${device_ids}" -b
xcrun simctl shutdown "${device_ids}"
simulator_may_be_running=false
trap - EXIT INT TERM
echo "==> ${prewarm_target} prewarm complete"
RUNNER_SIMULATOR
