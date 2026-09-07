#!/usr/bin/env bash
# Exercise every readiness gate against stubbed guest tools.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-guest-ready-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-guest-ready-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

helper="${template_dir}/guest/flanforge-guest-ready"
# The helper resolves every path under one prefix, so the whole guest layout is
# reproduced here rather than smuggled in through the environment.
root="${test_root}/guest"
command_log="${test_root}/commands"
job_account_file="${root}/usr/local/share/flanforge/job-account"
runner_bin="${root}/usr/local/bin/forgejo-runner"
mkdir -p "${root}/usr/bin" "${root}/usr/sbin" "${root}/usr/local/bin" \
  "${root}/usr/local/share/flanforge" "${root}/run/user/2000/podman"

die() { echo "guest-ready test: $1" >&2; exit 1; }

ln -s /usr/bin/jq "${root}/usr/bin/jq"
ln -s /usr/bin/timeout "${root}/usr/bin/timeout"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "cloud-init %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'if [ "${FAKE_CLOUD_INIT_HANGS:-false}" = true ]; then sleep 600; fi' \
  'printf "%s\n" "${FAKE_CLOUD_INIT_JSON}"' \
  'exit "${FAKE_CLOUD_INIT_EXIT:-0}"' > "${root}/usr/bin/cloud-init"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "systemctl %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'case "$1" in' \
  '  is-system-running) printf "%s\n" "${FAKE_SYSTEM_STATE}"; [ "${FAKE_SYSTEM_STATE}" = running ] ;;' \
  '  is-active)' \
  '    unit="${!#}"' \
  '    case " ${FAKE_INACTIVE_UNITS} " in *" ${unit} "*) exit 3 ;; esac' \
  '    exit 0 ;;' \
  '  *) exit 0 ;;' \
  'esac' > "${root}/usr/bin/systemctl"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "runuser %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'exit "${FAKE_RUNNER_EXIT:-0}"' > "${root}/usr/sbin/runuser"

# The helper resolves id like every other guest tool, so the invoking account
# is simulated rather than inherited from whoever runs the suite.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "%s\n" "${FAKE_UID}"' > "${root}/usr/bin/id"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "runner %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'exit "${FAKE_RUNNER_EXIT:-0}"' > "${runner_bin}"
chmod 0755 "${root}/usr/bin/cloud-init" "${root}/usr/bin/systemctl" \
  "${root}/usr/sbin/runuser" "${root}/usr/bin/id" "${runner_bin}"

printf 'runner:2000\n' > "${job_account_file}"
# The session gate tests for a socket exactly, so a fifo would not do.
python3 - "${root}/run/user/2000/podman/podman.sock" <<'PY' || die "cannot create the fixture socket"
import socket, sys
socket.socket(socket.AF_UNIX, socket.SOCK_STREAM).bind(sys.argv[1])
PY

run_helper() {
  local output
  set +e
  output="$(env \
    FAKE_COMMAND_LOG="${command_log}" \
    FAKE_CLOUD_INIT_JSON="${FAKE_CLOUD_INIT_JSON:-}" \
    FAKE_CLOUD_INIT_HANGS="${FAKE_CLOUD_INIT_HANGS:-false}" \
    FAKE_SYSTEM_STATE="${FAKE_SYSTEM_STATE:-running}" \
    FAKE_INACTIVE_UNITS="${FAKE_INACTIVE_UNITS:-}" \
    FAKE_RUNNER_EXIT="${FAKE_RUNNER_EXIT:-0}" \
    FAKE_UID="${FAKE_UID:-0}" \
    bash "${helper}" --fixture-root "${root}" "$@" 2>"${test_root}/stderr")"
  helper_status=$?
  set -e
  helper_output="${output}"
}

expect() {
  local reason="$1" want_status="$2" filter="$3"
  [ "${helper_status}" -eq "${want_status}" ] \
    || die "${reason}: exit ${helper_status}, wanted ${want_status} (${helper_output})"
  [ -z "${filter}" ] && return 0
  printf '%s' "${helper_output}" | jq -e "${filter}" >/dev/null \
    || die "${reason}: unexpected reply ${helper_output}"
}

: > "${command_log}"

# 1. Usage errors are refused before anything runs in the guest.
for arguments in "--wait-seconds abc" "--wait-seconds 0" "--wait-seconds 121" \
  "--wait-seconds" "" "--unknown"; do
  # shellcheck disable=SC2086
  run_helper ${arguments}
  [ "${helper_status}" -eq 64 ] \
    || die "usage error was accepted: '${arguments}' exited ${helper_status}"
done
if grep -q cloud-init "${command_log}"; then
  die "a usage error still probed the guest"
fi

# 2. --self-test reports the contract without running any gate.
: > "${command_log}"
run_helper --self-test
expect "self-test" 0 '.schema == 1 and .contract == 2 and .ready == false and
  .gate == "self-test" and .failed_units == [] and
  .job_account == {name: "runner", uid: 2000}'
if grep -q cloud-init "${command_log}"; then
  die "the self-test probed cloud-init"
fi

# The normalized modern value; the legacy class-name spelling is case 5b.
readonly DONE_JSON='{"status":"done","datasource":"nocloud"}'

# 3. The whole ladder passes.
FAKE_CLOUD_INIT_JSON="${DONE_JSON}"
run_helper --wait-seconds 5
expect "a fully provisioned guest" 0 '.ready == true and .gate == "ready"'

# 4. cloud-init states map to retry or a named terminal failure.
FAKE_CLOUD_INIT_JSON='{"status":"running","datasource":null}'
run_helper --wait-seconds 5
expect "a running cloud-init" 10 '.ready == false and .gate == "cloud-init"'
FAKE_CLOUD_INIT_JSON='{"status":"not run","datasource":null}'
run_helper --wait-seconds 5
expect "an unstarted cloud-init" 10 '.gate == "cloud-init"'
FAKE_CLOUD_INIT_JSON='{"status":"error","errors":["Failed loading yaml blob. Invalid format at line 7 column 3"]}'
run_helper --wait-seconds 5
expect "a failed cloud-init" 11 '.gate == "cloud-init" and .state == "error" and
  (.detail | test("Invalid format at line 7"))'
FAKE_CLOUD_INIT_JSON='{"status":"disabled"}'
run_helper --wait-seconds 5
expect "a disabled cloud-init" 11 '.gate == "cloud-init"'

# 5. A detached seed reports done with the wrong datasource.
FAKE_CLOUD_INIT_JSON='{"status":"done","datasource":"none"}'
run_helper --wait-seconds 5
expect "a detached seed" 11 '.gate == "cloud-init" and .state == "datasource" and
  (.detail | test("none"))'

# 5b. Both datasource spellings pass: pre-23 cloud-init emits the class name.
FAKE_CLOUD_INIT_JSON='{"status":"done","datasource":"DataSourceNoCloud [seed=/dev/vdb]"}'
run_helper --wait-seconds 5
expect "a legacy datasource spelling" 0 '.ready == true'
FAKE_CLOUD_INIT_JSON='{"status":"done","datasource":"DataSourceNone"}'
run_helper --wait-seconds 5
expect "a legacy detached seed" 11 '.gate == "cloud-init" and .state == "datasource"'

# 6. Degraded systemd is not fatal; a required unit that is down is.
FAKE_CLOUD_INIT_JSON="${DONE_JSON}"
FAKE_SYSTEM_STATE=degraded
run_helper --wait-seconds 5
expect "a degraded but usable guest" 0 '.ready == true'
FAKE_SYSTEM_STATE=starting
run_helper --wait-seconds 5
expect "a still-starting systemd" 10 '.gate == "units"'
FAKE_SYSTEM_STATE=running
FAKE_INACTIVE_UNITS="tailscaled.service qemu-guest-agent.service"
run_helper --wait-seconds 5
expect "inactive required units" 11 '.gate == "units" and
  (.failed_units | length == 2) and
  (.failed_units | index("tailscaled.service") != null)'

# 7. The session gate retries rather than failing.
FAKE_INACTIVE_UNITS="user@2000.service"
run_helper --wait-seconds 5
expect "an unstarted user session" 10 '.gate == "session"'

# 8. A runner that cannot answer one-job is terminal.
FAKE_INACTIVE_UNITS=""
FAKE_RUNNER_EXIT=1
run_helper --wait-seconds 5
expect "an incompatible runner" 11 '.gate == "runner"'
FAKE_RUNNER_EXIT=0

# 9. Running as another account drops to the job account; running as it does not.
: > "${command_log}"
run_helper --wait-seconds 5
expect "a root-run readiness probe" 0 '.ready == true'
grep -q "^runuser .*forgejo-runner" "${command_log}" \
  || die "the runner gate did not drop to the job account"
FAKE_UID=2000
: > "${command_log}"
run_helper --wait-seconds 5
expect "a job-account readiness probe" 0 '.ready == true'
if grep -q "^runuser " "${command_log}"; then
  die "the runner gate dropped privileges it did not hold"
fi
FAKE_UID=0

# 10. A hostile detail still yields one bounded, printable JSON line.
hostile="$(printf 'a%.0s' {1..10000})"
FAKE_CLOUD_INIT_JSON="$(jq -cn --arg detail "line1
line2	${hostile}   ünicode" '{status: "error", errors: [$detail]}')"
run_helper --wait-seconds 5
expect "a hostile cloud-init error" 11 '.gate == "cloud-init"'
[ "$(printf '%s' "${helper_output}" | wc -l)" -eq 0 ] \
  || die "the reply is not a single line"
[ "${#helper_output}" -le 4096 ] || die "the reply exceeds 4096 bytes"
detail="$(printf '%s' "${helper_output}" | jq -r '.detail')"
[ "${#detail}" -le 256 ] || die "the detail exceeds 256 bytes"
printf '%s' "${detail}" | LC_ALL=C grep -qE '^[ -~]*$' \
  || die "the detail is not printable ASCII"

# 11. A hanging cloud-init returns within the wait plus its fallback bound.
FAKE_CLOUD_INIT_JSON="${DONE_JSON}"
FAKE_CLOUD_INIT_HANGS=true
started="$(date +%s)"
run_helper --wait-seconds 2
elapsed=$(( $(date +%s) - started ))
[ "${helper_status}" -eq 10 ] \
  || die "a hanging cloud-init did not report a retry (exit ${helper_status})"
[ "${elapsed}" -le 20 ] || die "a hanging cloud-init was not bounded (${elapsed}s)"
FAKE_CLOUD_INIT_HANGS=false

# 12. A malformed job-account record is terminal and never invented.
printf 'root:0\n' > "${job_account_file}"
run_helper --wait-seconds 5
expect "a privileged job account" 11 '.gate == "account" and
  .job_account == {name: "", uid: 0}'
printf 'runner:2000\n' > "${job_account_file}"

# 13. A missing tool still answers in JSON, with the tool named.
mv "${root}/usr/bin/cloud-init" "${root}/usr/bin/cloud-init.away"
run_helper --wait-seconds 5
expect "a missing guest tool" 11 '.ready == false and .gate == "self-test" and
  .state == "absent" and (.detail | test("cloud-init"))'
grep -q "missing guest tool" "${test_root}/stderr" \
  || die "a missing tool was not named on stderr"
mv "${root}/usr/bin/cloud-init.away" "${root}/usr/bin/cloud-init"

# 14. A traversal-shaped fixture root is a usage error, never a path.
set +e
bash "${helper}" --fixture-root "${root}/../guest" --wait-seconds 5 >/dev/null 2>&1
status=$?
set -e
[ "${status}" -eq 64 ] || die "a traversal fixture root was accepted (${status})"

echo "guest readiness helper fake tests passed"
