#!/usr/bin/env bash
# Exercise every recycle gate against stubbed guest tools.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-guest-recycle-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-guest-recycle-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

helper="${template_dir}/guest/flanforge-guest-recycle"
# The helper resolves every path under one prefix, so the whole guest layout is
# reproduced here rather than smuggled in through the environment.
root="${test_root}/guest"
command_log="${test_root}/commands"
procfs="${test_root}/procfs.sh"
job_account_file="${root}/usr/local/share/flanforge/job-account"
runner_bin="${root}/usr/local/bin/forgejo-runner"
account_home="${root}/home/runner"
account_uid=2000
mkdir -p "${root}/usr/bin" "${root}/usr/sbin" "${root}/usr/local/bin" \
  "${root}/usr/local/share/flanforge" "${root}/tmp" \
  "${account_home}/_work" "${account_home}/.docker" \
  "${account_home}/.config/systemd/user"

die() { echo "guest-recycle test: $1" >&2; exit 1; }

# Only the tools the helper resolves under the fixture root; df is stubbed
# below so free space is a fixture rather than the host's.
for tool in jq timeout date; do
  target="${root}/usr/bin/${tool}"
  source="$(command -v "${tool}")" || die "the suite needs ${tool}"
  ln -sf "${source}" "${target}"
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

# Every stubbed tool logs its argv, so a gate that did not run is visible.
stub() {
  local path="$1" body="${2:-exit 0}"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    "printf '$(basename "${path}") %s\\n' \"\$*\" >> \"\${FAKE_COMMAND_LOG}\"" \
    "${body}" > "${path}"
  chmod 0755 "${path}"
}

stub "${root}/usr/bin/id" 'printf "%s\n" "${FAKE_UID}"'
stub "${root}/usr/bin/systemctl"
stub "${root}/usr/bin/loginctl"
stub "${root}/usr/sbin/runuser" 'exit "${FAKE_RUNNER_EXIT:-0}"'
stub "${root}/usr/bin/crontab"
stub "${root}/usr/bin/findmnt" 'printf "%s\n" "${FAKE_MOUNTS:-}"'
stub "${root}/usr/bin/umount"
stub "${root}/usr/bin/tailscale" \
  'if [ "$1" = status ]; then printf "{\"BackendState\":\"%s\"}\n" "${FAKE_TAILSCALE_STATE:-NeedsLogin}"; fi'
stub "${root}/usr/bin/chronyc" \
  'if [ "$1" = tracking ]; then printf "System time     : %s seconds slow of NTP time\n" "${FAKE_SKEW:-0.5}"; fi'
stub "${runner_bin}" 'exit "${FAKE_RUNNER_EXIT:-0}"'

# ps answers the two questions the gate asks of it, from the real process tree.
cat > "${root}/usr/bin/ps" <<'PS'
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
chmod 0755 "${root}/usr/bin/ps"

# `pgrep -u` reports the command substitution the gate wrapped around this call:
# a live child of the gate's own shell, owned by the same account. Reporting it
# is what a stub returning only fixture pids could never do.
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
cat > "${root}/usr/bin/kill" <<'KILL'
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
chmod 0755 "${root}/usr/bin/kill"

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

printf 'runner:%s\n' "${account_uid}" > "${job_account_file}"

# df is stubbed rather than symlinked so free space is a fixture, not the
# host's, and the disk gate can actually be driven.
stub "${root}/usr/bin/df" \
  'printf "Filesystem 1M-blocks Used Available Capacity Mounted\n/dev/vda1 40000 100 %s 1%% /\n" "${FAKE_FREE_MB:-20000}"'

run_helper() {
  local output
  set +e
  output="$(env \
    FAKE_COMMAND_LOG="${command_log}" \
    FAKE_PROCFS="${procfs}" \
    FAKE_UID="${FAKE_UID:-0}" \
    FAKE_ACCOUNT_UID="${account_uid}" \
    FAKE_RUNNER_EXIT="${FAKE_RUNNER_EXIT:-0}" \
    FAKE_FREE_MB="${FAKE_FREE_MB:-20000}" \
    FAKE_SKEW="${FAKE_SKEW:-0.5}" \
    FAKE_SURVIVORS="${FAKE_SURVIVORS:-}" \
    FAKE_REPORT_SUBSHELL="${FAKE_REPORT_SUBSHELL:-false}" \
    FAKE_TAILSCALE_STATE="${FAKE_TAILSCALE_STATE:-NeedsLogin}" \
    FAKE_MOUNTS="${FAKE_MOUNTS:-}" \
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

# 2. --self-test reports the contract without resetting anything.
: > "${command_log}"
run_helper --self-test
expect "self-test" 0 '.schema == 1 and .contract == 1 and .clean == false and
  .gate == "self-test" and .job_account == {name: "runner", uid: 2000}'
if grep -Eq "^(kill|pkill) " "${command_log}"; then
  die "the self-test reset the guest"
fi

# 3. A clean machine passes, and the reset actually ran.
: > "${command_log}"
printf 'job output\n' > "${account_home}/_work/artifact"
printf 'machine token\n' > "${account_home}/.netrc"
printf 'creds\n' > "${account_home}/.docker/config.json"
printf 'forged\n' > "${account_home}/.flanforge-regeneration-complete"
printf 'token\n' > "${root}/tmp/flanforged-one-job-token.abc123"
run_helper --simulator-reset apps
expect "a clean machine" 0 '.clean == true and .gate == "clean" and
  .state == "apps" and .free_mb == 20000'
for leftover in "${account_home}/_work" "${account_home}/.netrc" \
  "${account_home}/.docker/config.json" \
  "${account_home}/.flanforge-regeneration-complete" \
  "${root}/tmp/flanforged-one-job-token.abc123" \
  "${account_home}/.config/systemd/user"; do
  [ ! -e "${leftover}" ] || die "the reset left ${leftover} behind"
done
grep -q "^loginctl terminate-user runner" "${command_log}" \
  || die "the user session was not torn down"
grep -q "^tailscale logout" "${command_log}" || die "the tailnet session was not dropped"
grep -q "^crontab -r -u runner" "${command_log}" || die "user cron was not removed"

# 4. What the gate must not touch: removing these removes the entire point.
mkdir -p "${account_home}/.cache" "${account_home}/.ssh"
printf 'cached\n' > "${account_home}/.cache/toolchain"
printf 'daemon key\n' > "${account_home}/.ssh/authorized_keys"
run_helper --simulator-reset apps
expect "a second pass" 0 '.clean == true'
[ -f "${account_home}/.cache/toolchain" ] || die "the toolchain cache was removed"
[ -f "${account_home}/.ssh/authorized_keys" ] || die "the daemon's SSH key was removed"
[ -x "${runner_bin}" ] || die "the runner binary was removed"

# 5. A process the job account still owns fails the machine, whatever it is
# called. A name filter here would be defeated by a job that names its daemon
# after the shell the reset itself runs in.
for survivor in "4242 leftover-daemon" "4242 bash" "4242 zsh" "4242 sshd" \
  "4242 login" "4242 pgrep"; do
  FAKE_SURVIVORS="${survivor}"
  run_helper --simulator-reset apps
  expect "a surviving ${survivor#* }" 11 '.clean == false and .gate == "processes"'
done

# 6. A survivor is signalled by pid. `pkill -u` would match the reset's own
# shell and session, so the gate must never name the account to a killer.
: > "${command_log}"
FAKE_SURVIVORS="4242 leftover-daemon"
run_helper --simulator-reset apps
expect "a survivor is killed" 11 '.gate == "processes"'
grep -q "^kill -KILL 4242" "${command_log}" || die "the survivor was not killed by pid"
if grep -q "^pkill " "${command_log}"; then
  die "the gate signalled every process the job account owns, its own included"
fi
FAKE_SURVIVORS=""

# 7. The reset runs as the job account over SSH, which is the shape the daemon
# actually uses on Tart and uses on libvirt whenever guest.channel = "ssh". A
# gate that signals the account it is running as never reaches its own emit, so
# the proof is simply that a verdict came back at all.
: > "${command_log}"
FAKE_UID="${account_uid}"
run_helper --simulator-reset apps
expect "the gate survives running as the job account" 0 '.clean == true'
# Tearing the login session down is root's, and it would take this reset's own
# session with it, so it is not attempted from the account.
if grep -q "^loginctl terminate-user" "${command_log}"; then
  die "the gate tore down the session that carried it"
fi
grep -q "^crontab -r$" "${command_log}" \
  || die "the account's own cron was not removed without -u"
FAKE_UID=0

# 8. The gate's own command substitutions are children of its shell and owned by
# the same account, so `pgrep -u` reports them. Counting them is counting
# itself, and the machine would never pass.
FAKE_REPORT_SUBSHELL=true
run_helper --simulator-reset apps
expect "the gate's own subshell" 0 '.clean == true'
FAKE_UID="${account_uid}"
run_helper --simulator-reset apps
expect "the gate's own subshell as the job account" 0 '.clean == true'
FAKE_UID=0
FAKE_REPORT_SUBSHELL=false

# 9. A disk below the floor fails the machine, not the next job.
FAKE_FREE_MB=100
run_helper --simulator-reset apps
expect "a full disk" 11 '.gate == "disk" and .free_mb == 100'
FAKE_FREE_MB=20000

# 10. Clock skew is resynced, then re-checked; only a skew that survives fails.
FAKE_SKEW=600.0
run_helper --simulator-reset apps
expect "a skewed clock" 11 '.gate == "clock" and .skew_seconds == 600'
grep -q "^chronyc makestep" "${command_log}" || die "the clock was not resynced"
FAKE_SKEW=0.5

# 11. A runner that cannot answer one-job fails the machine.
FAKE_RUNNER_EXIT=1
run_helper --simulator-reset apps
expect "an incompatible runner" 11 '.gate == "runner"'
FAKE_RUNNER_EXIT=0

# 12. A tailnet session that survived the logout fails the machine: it is the
# one credential-shaped carryover the reset cannot simply unlink.
FAKE_TAILSCALE_STATE=Running
run_helper --simulator-reset apps
expect "a surviving tailnet session" 11 '.gate == "tailscale"'
FAKE_TAILSCALE_STATE=NeedsLogin

# 13. A malformed job-account record is terminal and never invented.
printf 'root:0\n' > "${job_account_file}"
run_helper --simulator-reset apps
expect "a privileged job account" 11 '.gate == "account" and
  .job_account == {name: "", uid: 0}'
printf 'runner:%s\n' "${account_uid}" > "${job_account_file}"

# 14. A hostile detail still yields one bounded, printable JSON line.
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

# 15. A missing tool still answers in JSON, with the tool named.
mv "${root}/usr/bin/pgrep" "${root}/usr/bin/pgrep.away"
run_helper --simulator-reset apps
expect "a missing guest tool" 11 '.clean == false and .gate == "self-test" and
  .state == "absent" and (.detail | test("pgrep"))'
grep -q "missing guest tool" "${test_root}/stderr" \
  || die "a missing tool was not named on stderr"
mv "${root}/usr/bin/pgrep.away" "${root}/usr/bin/pgrep"

# 16. A traversal-shaped fixture root is a usage error, never a path.
set +e
bash "${helper}" --fixture-root "${root}/../guest" --simulator-reset apps >/dev/null 2>&1
status=$?
set -e
[ "${status}" -eq 64 ] || die "a traversal fixture root was accepted (${status})"

echo "guest recycle helper fake tests passed"
