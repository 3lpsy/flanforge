#!/usr/bin/env bash
# Exercise every recycle gate against stubbed guest tools. Runs anywhere: the
# helper resolves launchctl, security and simctl under a fixture root, so no
# Mac is needed to prove its argument handling, its reset, or its verdicts.
#
# The daemon reaches this gate over SSH as the job account and the image grants
# that account no sudo, so the job account — not root — is what these cases run
# as by default.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-tart-recycle-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-tart-recycle-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

helper="${template_dir}/guest/flanforge-guest-recycle"
root="${test_root}/guest"
command_log="${test_root}/commands"
procfs="${test_root}/procfs.sh"
account_home="${root}/Users/runner"
runner_bin="${account_home}/bin/forgejo-runner"
account_uid=501
mkdir -p "${root}/usr/bin" "${root}/usr/sbin" "${root}/usr/local/bin" \
  "${root}/bin" "${root}/sbin" "${root}/opt/homebrew/bin" "${root}/tmp" \
  "${root}/Volumes" "${account_home}/bin" "${account_home}/_work" \
  "${account_home}/Library/LaunchAgents" "${account_home}/Library/Keychains" \
  "${account_home}/.docker"

die() { echo "tart guest-recycle test: $1" >&2; exit 1; }

for tool in jq timeout; do
  source="$(command -v "${tool}")" || die "the suite needs ${tool}"
  ln -sf "${source}" "${root}/usr/local/bin/${tool}"
done

# The gate reasons about process ancestry, so the fixture answers those
# questions for real — from /proc where it exists, from ps otherwise — rather
# than stubbing the process tree flat, which is what let a pid-walk bug hide.
cat > "${procfs}" <<'PROCFS'
parent_of() {
  local pid="$1" line rest
  if [ -r "/proc/${pid}/stat" ]; then
    line="$(cat "/proc/${pid}/stat")"
    rest="${line##*) }"
    # shellcheck disable=SC2086
    set -- ${rest}
    printf '%s\n' "$2"
    return 0
  fi
  ps -o ppid= -p "${pid}" 2>/dev/null | tr -cd '0-9'
}

name_of() {
  local pid="$1"
  if [ -r "/proc/${pid}/comm" ]; then
    cat "/proc/${pid}/comm"
    return 0
  fi
  ps -o comm= -p "${pid}" 2>/dev/null | tr -d ' '
}

# Every pid from this one up to init: what a caller running as the job account
# owns legitimately, and what a reset must never signal.
own_ancestry() {
  local walk="${1:-${PPID}}" own=" "
  while [ "${walk}" -gt 1 ]; do
    own="${own}${walk} "
    walk="$(parent_of "${walk}")"
    [ -n "${walk}" ] || break
  done
  printf '%s' "${own}"
}
PROCFS

stub() {
  local path="$1" body="${2:-exit 0}"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    "printf '$(basename "${path}") %s\\n' \"\$*\" >> \"\${FAKE_COMMAND_LOG}\"" \
    "${body}" > "${path}"
  chmod 0755 "${path}"
}

# `id -u` with no argument reports the invoking account; `id -u runner` reports
# the job account, and the helper reads its uid rather than assuming one.
stub "${root}/usr/bin/id" \
  'if [ "$#" -ge 2 ]; then printf "%s\n" "${FAKE_ACCOUNT_UID:-501}"; else printf "%s\n" "${FAKE_UID:-501}"; fi'
stub "${root}/bin/launchctl"
stub "${root}/usr/bin/security"
stub "${root}/usr/bin/crontab"
stub "${root}/usr/sbin/diskutil"
stub "${root}/sbin/mount" 'printf "%s\n" "${FAKE_MOUNTS:-}"'
# The image asserts `runner must not be an administrator` and bakes no sudoers
# rule, so `sudo -n` is a refusal. A reset that routed the account's own files
# through it would be a silent no-op reporting itself clean.
stub "${root}/usr/bin/sudo" 'exit 1'
stub "${root}/opt/homebrew/bin/tailscale" \
  'if [ "$1" = status ]; then printf "{\"BackendState\":\"%s\"}\n" "${FAKE_TAILSCALE_STATE:-NeedsLogin}"; fi'
stub "${root}/bin/df" \
  'printf "Filesystem 1M-blocks Used Avail Capacity Mounted\n/dev/disk1 80000 100 %s 1%% /\n" "${FAKE_FREE_MB:-40000}"'
stub "${root}/usr/bin/sntp" \
  'printf "sntp 4.2.8 %s +/- 0.01 time.apple.com\n" "${FAKE_SKEW:-+0.004}"'
stub "${runner_bin}" 'exit "${FAKE_RUNNER_EXIT:-0}"'

# ps answers the two questions the gate asks of it, from the real process tree.
cat > "${root}/bin/ps" <<'PS'
#!/usr/bin/env bash
source "${FAKE_PROCFS}"
target=""
field=ppid
while [ "$#" -gt 0 ]; do
  case "$1" in
    -p) target="${2:-}"; shift 2 ;;
    -o) case "${2:-}" in comm=) field=comm ;; esac; shift 2 ;;
    *) shift ;;
  esac
done
case "${target}" in ''|*[!0-9]*) exit 1 ;; esac
if [ "${field}" = comm ]; then name_of "${target}"; else parent_of "${target}"; fi
PS
chmod 0755 "${root}/bin/ps"

# `pgrep -u` reports the command substitution a caller wraps around it: a live
# child of the gate's own shell, owned by the same account. Reporting it is what
# a stub returning only fixture pids could never do.
cat > "${root}/usr/bin/pgrep" <<'PGREP'
#!/usr/bin/env bash
printf 'pgrep %s\n' "$*" >> "${FAKE_COMMAND_LOG}"
source "${FAKE_PROCFS}"
found=false
if [ "${FAKE_REPORT_SUBSHELL:-false}" = true ]; then
  walk="${PPID}"
  while [ "${walk}" -gt 1 ]; do
    printf '%s %s\n' "${walk}" "$(name_of "${walk}")"
    walk="$(parent_of "${walk}")"
    [ -n "${walk}" ] || break
  done
  found=true
fi
if [ -n "${FAKE_SURVIVORS:-}" ]; then
  printf '%s\n' "${FAKE_SURVIVORS}"
  found=true
fi
[ "${found}" = true ]
PGREP
chmod 0755 "${root}/usr/bin/pgrep"

# Signals are acted out rather than asserted on: a gate that signals its own
# session dies before it can emit, which is the only way a fixture can fail the
# way the guest did.
cat > "${root}/bin/kill" <<'KILL'
#!/usr/bin/env bash
printf 'kill %s\n' "$*" >> "${FAKE_COMMAND_LOG}"
source "${FAKE_PROCFS}"
own="$(own_ancestry)"
for target in "$@"; do
  case "${target}" in -*) continue ;; esac
  case "${own}" in *" ${target} "*) command kill -KILL "${target}" ;; esac
done
exit 0
KILL
chmod 0755 "${root}/bin/kill"

# `pkill -KILL -u <uid>` matches by uid alone, so when the gate runs as the job
# account it matches the gate's own shell, its login session and the sshd that
# carried it. The gate must never reach for it; this stub is what proves so.
cat > "${root}/usr/bin/pkill" <<'PKILL'
#!/usr/bin/env bash
printf 'pkill %s\n' "$*" >> "${FAKE_COMMAND_LOG}"
source "${FAKE_PROCFS}"
if [ "${FAKE_UID}" = "${FAKE_ACCOUNT_UID}" ]; then
  command kill -KILL "${PPID}" 2>/dev/null || true
fi
exit 0
PKILL
chmod 0755 "${root}/usr/bin/pkill"

# xcrun is the simulator surface; `xcrun simctl <verb>` is what the helper runs.
cat > "${root}/usr/bin/xcrun" <<'XCRUN'
#!/usr/bin/env bash
printf 'xcrun %s\n' "$*" >> "${FAKE_COMMAND_LOG}"
case "$2" in
  list) printf '%s\n' "${FAKE_BOOTED_JSON:-}" ;;
  listapps) printf '%s\n' "${FAKE_APPS:-}" ;;
esac
exit 0
XCRUN
chmod 0755 "${root}/usr/bin/xcrun"

run_helper() {
  local output
  set +e
  output="$(env \
    FAKE_COMMAND_LOG="${command_log}" \
    FAKE_PROCFS="${procfs}" \
    FAKE_UID="${FAKE_UID:-${account_uid}}" \
    FAKE_ACCOUNT_UID="${FAKE_ACCOUNT_UID:-${account_uid}}" \
    FAKE_RUNNER_EXIT="${FAKE_RUNNER_EXIT:-0}" \
    FAKE_FREE_MB="${FAKE_FREE_MB:-40000}" \
    FAKE_SKEW="${FAKE_SKEW:-+0.004}" \
    FAKE_SURVIVORS="${FAKE_SURVIVORS:-}" \
    FAKE_REPORT_SUBSHELL="${FAKE_REPORT_SUBSHELL:-false}" \
    FAKE_TAILSCALE_STATE="${FAKE_TAILSCALE_STATE:-NeedsLogin}" \
    FAKE_MOUNTS="${FAKE_MOUNTS:-}" \
    FAKE_BOOTED_JSON="${FAKE_BOOTED_JSON:-}" \
    FAKE_APPS="${FAKE_APPS:-}" \
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

# 1. The argument allow-list is the same one the Linux sibling enforces, so a
#    daemon that speaks to both never has to know which it reached.
for arguments in "--simulator-reset" "--simulator-reset all" "" "--unknown" \
  "--simulator-reset Apps"; do
  # shellcheck disable=SC2086
  run_helper ${arguments}
  [ "${helper_status}" -eq 64 ] \
    || die "usage error was accepted: '${arguments}' exited ${helper_status}"
done
if grep -Eq "^(kill|pkill) " "${command_log}"; then
  die "a usage error still reset the guest"
fi

# 2. --self-test reports the same contract the Linux sibling reports.
: > "${command_log}"
run_helper --self-test
expect "self-test" 0 '.schema == 1 and .contract == 1 and .clean == false and
  .gate == "self-test" and .job_account == {name: "runner", uid: 501}'
if grep -Eq "^(kill|pkill) " "${command_log}"; then
  die "the self-test reset the guest"
fi

# 3. A clean machine passes, and the reset actually ran — as the job account,
#    with sudo refusing, which is the only shape the daemon ever produces here.
#    Every path below is the account's own, so none of it may depend on an
#    elevation this image does not grant.
: > "${command_log}"
printf 'job output\n' > "${account_home}/_work/artifact"
printf 'creds\n' > "${account_home}/.docker/config.json"
printf 'keychain\n' > "${account_home}/Library/Keychains/login.keychain-db"
printf 'machine token\n' > "${account_home}/.netrc"
printf 'forged\n' > "${account_home}/.flanforge-regeneration-complete"
printf 'token\n' > "${root}/tmp/flanforged-one-job-token.abc123"
printf 'persist\n' > "${account_home}/Library/LaunchAgents/com.evil.persist.plist"
run_helper --simulator-reset apps
expect "a clean machine" 0 '.clean == true and .gate == "clean" and
  .state == "apps" and .free_mb == 40000'
for leftover in "${account_home}/_work" "${account_home}/.docker/config.json" \
  "${account_home}/Library/Keychains/login.keychain-db" \
  "${account_home}/.netrc" \
  "${account_home}/.flanforge-regeneration-complete" \
  "${root}/tmp/flanforged-one-job-token.abc123" \
  "${account_home}/Library/LaunchAgents"; do
  [ ! -e "${leftover}" ] || die "the reset left ${leftover} behind"
done
if grep -q "^sudo " "${command_log}"; then
  die "the reset routed the account's own state through an elevation the image never granted"
fi
# A job can install a LaunchAgent and persist itself, so the launchd domain is
# what has to be cleared, not just the process table.
grep -q "^launchctl bootout gui/501/com.evil.persist" "${command_log}" \
  || die "a job-installed LaunchAgent was not booted out"
grep -q "^tailscale logout" "${command_log}" || die "the tailnet session was not dropped"
grep -q "^crontab -r$" "${command_log}" || die "the account's own cron was not removed"
mkdir -p "${account_home}/Library/LaunchAgents"

# 4. What the gate must not touch: removing these removes the entire point.
mkdir -p "${account_home}/Library/Developer/CoreSimulator" "${account_home}/.ssh"
printf 'runtimes\n' > "${account_home}/Library/Developer/CoreSimulator/runtimes"
printf 'daemon key\n' > "${account_home}/.ssh/authorized_keys"
run_helper --simulator-reset apps
expect "a second pass" 0 '.clean == true'
[ -f "${account_home}/Library/Developer/CoreSimulator/runtimes" ] \
  || die "the simulator runtimes were removed"
[ -f "${account_home}/.ssh/authorized_keys" ] || die "the daemon's SSH key was removed"
[ -x "${runner_bin}" ] || die "the runner binary was removed"

# 5. simulator_reset decides how far the gate goes, and each setting is
#    visible in what it ran.
: > "${command_log}"
FAKE_BOOTED_JSON='{"devices":{"iOS 18":[{"udid":"AAAA-BBBB"}]}}'
FAKE_APPS='"CFBundleIdentifier" = "com.example.app";'
run_helper --simulator-reset apps
expect "an app reset" 0 '.state == "apps"'
grep -q "^xcrun simctl uninstall AAAA-BBBB com.example.app" "${command_log}" \
  || die "the job's app was not uninstalled"
if grep -q "^xcrun simctl erase" "${command_log}"; then
  die "an app reset erased the device"
fi

: > "${command_log}"
run_helper --simulator-reset erase
expect "an erasing reset" 0 '.state == "erase"'
grep -q "^xcrun simctl erase all" "${command_log}" || die "the devices were not erased"

: > "${command_log}"
run_helper --simulator-reset none
expect "a retaining reset" 0 '.state == "none"'
if grep -q "^xcrun simctl" "${command_log}"; then
  die "simulator_reset = none still touched the simulator"
fi
FAKE_BOOTED_JSON=""
FAKE_APPS=""

# 6. A process the job account still owns fails the machine, whatever it is
#    called. This gate is what the whole reset is for, so it excludes the
#    reset's own by pid and nothing by name: a name filter is defeated by a job
#    that names its daemon after the shell the reset runs in, which is exactly
#    the shape `zsh -c '''...'''` leaves behind.
for survivor in "4242 leftover-daemon" "4242 bash" "4242 zsh" "4242 sshd" \
  "4242 login" "4242 pgrep" "4242 flanforge-guest-recycle"; do
  FAKE_SURVIVORS="${survivor}"
  run_helper --simulator-reset apps
  expect "a surviving ${survivor#* }" 11 '.clean == false and .gate == "processes"'
done

# 7. A survivor is signalled by pid. `pkill -u` matches the reset's own shell,
#    its login session and the sshd that carried it, so the gate must never name
#    the account to a killer — it would die before it could emit.
: > "${command_log}"
FAKE_SURVIVORS="4242 leftover-daemon"
run_helper --simulator-reset apps
expect "a survivor is killed" 11 '.gate == "processes"'
grep -q "^kill -KILL 4242" "${command_log}" || die "the survivor was not killed by pid"
if grep -q "^pkill " "${command_log}"; then
  die "the gate signalled every process the job account owns, its own included"
fi
FAKE_SURVIVORS=""

# 8. The gate's own command substitutions are children of its shell and owned by
#    the same account, so `pgrep -u` reports them. Counting them is counting
#    itself, and the machine would never pass.
FAKE_REPORT_SUBSHELL=true
run_helper --simulator-reset apps
expect "the gate's own subshell" 0 '.clean == true'
FAKE_REPORT_SUBSHELL=false

# 9. Over the root path the same reset holds, and cron is addressed by name.
: > "${command_log}"
FAKE_UID=0
run_helper --simulator-reset apps
expect "the reset as root" 0 '.clean == true'
grep -q "^crontab -r -u runner" "${command_log}" || die "user cron was not removed"
FAKE_UID="${account_uid}"

# 10. A disk below the floor fails the machine, not the next job.
FAKE_FREE_MB=100
run_helper --simulator-reset apps
expect "a full disk" 11 '.gate == "disk" and .free_mb == 100'
FAKE_FREE_MB=40000

# 11. Clock skew is resynced, then re-checked. A Mac that sleeps pauses its
#     guests, and both OIDC and TLS reject on that alone. Stepping the clock is
#     root's, so over SSH a skew that survives simply fails the machine.
FAKE_SKEW="+600.5"
run_helper --simulator-reset apps
expect "a skewed clock" 11 '.gate == "clock" and .skew_seconds == 600'
: > "${command_log}"
FAKE_UID=0
run_helper --simulator-reset apps
expect "a skewed clock as root" 11 '.gate == "clock"'
grep -q "^sntp -sS " "${command_log}" || die "the clock was not resynced"
FAKE_UID="${account_uid}"
FAKE_SKEW="-0.004"
run_helper --simulator-reset apps
expect "a clock behind but inside the bound" 0 '.clean == true and .skew_seconds == 0'
FAKE_SKEW="+0.004"

# 12. A runner that cannot answer one-job fails the machine.
FAKE_RUNNER_EXIT=1
run_helper --simulator-reset apps
expect "an incompatible runner" 11 '.gate == "runner"'
FAKE_RUNNER_EXIT=0

# 13. A tailnet session that survived the logout fails the machine: it is the
#     one credential-shaped carryover the reset cannot simply unlink.
FAKE_TAILSCALE_STATE=Running
run_helper --simulator-reset apps
expect "a surviving tailnet session" 11 '.gate == "tailscale"'
FAKE_TAILSCALE_STATE=NeedsLogin

# 14. A job account that is missing or privileged is terminal, never invented.
FAKE_ACCOUNT_UID=0
run_helper --simulator-reset apps
expect "a privileged job account" 11 '.gate == "account" and
  .job_account.uid == 0'
FAKE_ACCOUNT_UID="${account_uid}"

# 15. A hostile detail still yields one bounded, printable JSON line.
FAKE_SURVIVORS="$(printf '9 a%.0s' {1..10000})"
run_helper --simulator-reset apps
expect "a hostile process listing" 11 '.gate == "processes"'
[ "$(printf '%s' "${helper_output}" | wc -l)" -eq 0 ] \
  || die "the reply is not a single line"
[ "${#helper_output}" -le 4096 ] || die "the reply exceeds 4096 bytes"
detail="$(printf '%s' "${helper_output}" | jq -r '.detail')"
[ "${#detail}" -le 256 ] || die "the detail exceeds 256 bytes"
printf '%s' "${detail}" | LC_ALL=C grep -qE '^[ -~]*$' \
  || die "the detail is not printable ASCII"
FAKE_SURVIVORS=""

# 16. A missing tool still answers in JSON, with the tool named.
mv "${root}/usr/bin/pgrep" "${root}/usr/bin/pgrep.away"
run_helper --simulator-reset apps
expect "a missing guest tool" 11 '.clean == false and .gate == "self-test" and
  .state == "absent" and (.detail | test("pgrep"))'
grep -q "missing guest tool" "${test_root}/stderr" \
  || die "a missing tool was not named on stderr"
mv "${root}/usr/bin/pgrep.away" "${root}/usr/bin/pgrep"

# 17. A traversal-shaped fixture root is a usage error, never a path.
set +e
bash "${helper}" --fixture-root "${root}/../guest" --simulator-reset apps >/dev/null 2>&1
status=$?
set -e
[ "${status}" -eq 64 ] || die "a traversal fixture root was accepted (${status})"

echo "tart guest recycle helper fake tests passed"
