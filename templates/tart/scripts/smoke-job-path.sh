#!/bin/bash
# Job-time smoke check. This is NOT a Packer provisioner: it runs in a clone as
# `runner`, over SSH, delivered by templates/tart/smoke.sh. Provisioning reaches
# `runner` with `sudo -u runner` from the bootstrap account's session, which
# leaves the work in that account's launchd domain with a console session
# present; a real job has neither. Only this path proves the guest works.
set -euo pipefail

[ -n "${SSH_CONNECTION:-}" ] \
  || { echo "this check is meaningless unless it runs over SSH" >&2; exit 1; }
[ "$(id -un)" = runner ] \
  || { echo "this check must run as the runner account" >&2; exit 1; }

prewarm_target="${SMOKE_PREWARM_TARGET:-iPhone 17 Pro}"
[[ "${prewarm_target}" =~ ^[A-Za-z0-9][A-Za-z0-9._()\ -]{0,79}$ ]] \
  || { echo "SMOKE_PREWARM_TARGET contains unsupported characters" >&2; exit 1; }
boot_bound="${SMOKE_BOOT_TIMEOUT_SECONDS:-300}"
test_bound="${SMOKE_TEST_TIMEOUT_SECONDS:-1200}"
for bound in "${boot_bound}" "${test_bound}"; do
  [[ "${bound}" =~ ^[1-9][0-9]{0,4}$ ]] \
    || { echo "smoke timeouts must be positive integers of seconds" >&2; exit 1; }
done

# Nothing here may hang the template build it gates.
run_bounded() {
  local bound="$1"
  shift
  "$@" &
  local job_pid=$!
  ( sleep "${bound}"; kill -TERM "${job_pid}" 2>/dev/null || true ) >/dev/null 2>&1 &
  local watchdog_pid=$!
  local status=0
  wait "${job_pid}" || status=$?
  kill -TERM "${watchdog_pid}" 2>/dev/null || true
  wait "${watchdog_pid}" 2>/dev/null || true
  return "${status}"
}

echo "==> resolving simulator '${prewarm_target}'"
device_id="$(SMOKE_PREWARM_TARGET="${prewarm_target}" \
  xcrun simctl list --json devices available \
  | SMOKE_PREWARM_TARGET="${prewarm_target}" python3 -c '
import json
import os
import sys

target = os.environ["SMOKE_PREWARM_TARGET"]
inventory = json.load(sys.stdin)
identifiers = [
    device["udid"]
    for devices in inventory.get("devices", {}).values()
    for device in devices
    if device.get("isAvailable", False) and device.get("name") == target
]
sys.stdout.write("\n".join(identifiers))
')"
[ "$(printf '%s\n' "${device_id}" | awk 'NF { count += 1 } END { print count + 0 }')" -eq 1 ] \
  || { echo "expected exactly one available simulator named '${prewarm_target}'" >&2; exit 1; }
[[ "${device_id}" =~ ^[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}$ ]] \
  || { echo "simulator returned an invalid device identifier" >&2; exit 1; }

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-smoke.XXXXXX")"
cleanup() {
  local status=$?
  xcrun simctl shutdown "${device_id}" >/dev/null 2>&1 || true
  rm -rf "${work_dir}"
  return "${status}"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

echo "==> booting ${prewarm_target} (${device_id})"
run_bounded "${boot_bound}" xcrun simctl bootstatus "${device_id}" -b

mkdir -p "${work_dir}/Sources/FlanForgeSmoke" "${work_dir}/Tests/FlanForgeSmokeTests"
cat > "${work_dir}/Package.swift" <<'SMOKE_PACKAGE'
// swift-tools-version:5.9
import PackageDescription

// No dependencies: the check must not need forge or registry access.
let package = Package(
  name: "FlanForgeSmoke",
  platforms: [.iOS(.v17)],
  targets: [
    .target(name: "FlanForgeSmoke"),
    .testTarget(name: "FlanForgeSmokeTests", dependencies: ["FlanForgeSmoke"]),
  ]
)
SMOKE_PACKAGE
cat > "${work_dir}/Sources/FlanForgeSmoke/FlanForgeSmoke.swift" <<'SMOKE_SOURCE'
public func flanForgeSmokeValue() -> Int { 1 }
SMOKE_SOURCE
cat > "${work_dir}/Tests/FlanForgeSmokeTests/FlanForgeSmokeTests.swift" <<'SMOKE_TEST'
import XCTest
@testable import FlanForgeSmoke

final class FlanForgeSmokeTests: XCTestCase {
  func testRunsOnTheSimulator() {
    XCTAssertEqual(flanForgeSmokeValue(), 1)
  }
}
SMOKE_TEST

cd "${work_dir}"
# Printed first so a scheme mismatch is readable in the failure output.
xcodebuild -list
echo "==> xcodebuild test on ${device_id}"
run_bounded "${test_bound}" xcodebuild test \
  -scheme FlanForgeSmoke \
  -destination "platform=iOS Simulator,id=${device_id}" \
  -derivedDataPath "${work_dir}/DerivedData" \
  CODE_SIGNING_ALLOWED=NO

# Informational only. A run on 2026-08-16 recorded no file over SSH: screen
# capture wants a console session, which no job has. Never fail on it.
echo "==> recording probe (informational)"
video_file="${work_dir}/smoke.mp4"
xcrun simctl io "${device_id}" recordVideo --force "${video_file}" >/dev/null 2>&1 &
recorder_pid=$!
sleep 5
kill -INT "${recorder_pid}" 2>/dev/null || true
wait "${recorder_pid}" 2>/dev/null || true
if [ -s "${video_file}" ]; then
  echo "    note: simctl io recordVideo produced a file"
else
  echo "    note: simctl io recordVideo produced no file, as expected without a console session"
fi

echo "job-time smoke check passed as $(id -un) over SSH"
