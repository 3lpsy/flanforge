#!/usr/bin/env bash
# Post-build gate: clone the built template, connect as `runner` over SSH with
# the daemon's own options, and run the job-time path — simulator boot and one
# `xcodebuild test`. Provisioning cannot answer this: it reaches `runner` with
# `sudo -u runner` from the bootstrap account's session, so it never leaves that
# account's launchd domain. A template that fails this is not worth retaining.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd "${template_dir}/../.." && pwd)"
# shellcheck source=../shared/env.sh
source "${template_dir}/../shared/env.sh"

die() {
  echo "error: $*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    "Usage:" \
    "  ${0##*/} [--keep] [template-name]" \
    "  ${0##*/} -h | --help | help" \
    "" \
    "Clone the built template, boot the prewarmed simulator as the runner" \
    "account over SSH, run one xcodebuild test against it, then delete the" \
    "clone. templates/tart/build.sh runs this before retaining a template." \
    "" \
    "--keep leaves the clone running for manual inspection." \
    "" \
    "Environment:" \
    "  TART_VM_NAME                          Template to clone (default: flanforge-base)" \
    "  TART_HOME                             Tart storage (default: ~/.tart)" \
    "  FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE   Guest public key; its private half is used" \
    "  FLANFORGE_PREWARM_TARGET              Simulator to boot (default: iPhone 17 Pro)" \
    "  FLANFORGE_SMOKE_BOOT_TIMEOUT_SECONDS  Bound on simulator boot (default: 300)" \
    "  FLANFORGE_SMOKE_TEST_TIMEOUT_SECONDS  Bound on the test run (default: 1200)" >&2
}

keep_clone=false
positional_argument_count=0
template_argument=""
for argument in "$@"; do
  case "${argument}" in
    -h|--help|help)
      usage
      exit 0
      ;;
    --keep)
      keep_clone=true
      ;;
    -*)
      usage
      die "unknown option '${argument}'"
      ;;
    *)
      template_argument="${argument}"
      positional_argument_count=$((positional_argument_count + 1))
      ;;
  esac
done

load_tart_template_environment "${template_dir}" "${project_root}"
# Nothing here needs the build secrets, and this script spawns children.
unset FLANFORGE_TAILSCALE_PREAUTH_KEY FLANFORGE_ADMIN_PASSWORD

[ "${positional_argument_count}" -le 1 ] || {
  usage
  die "at most one template name may be provided"
}

command -v tart >/dev/null 2>&1 \
  || die "tart is required (brew install cirruslabs/cli/tart)"
command -v ssh >/dev/null 2>&1 || die "ssh is required"

template_name="${template_argument:-${TART_VM_NAME:-flanforge-base}}"
[[ "${template_name}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] \
  || die "template name contains unsupported characters"
tart get "${template_name}" >/dev/null 2>&1 \
  || die "Tart VM '${template_name}' not found; build it with templates/tart/build.sh"

prewarm_target="${FLANFORGE_PREWARM_TARGET:-iPhone 17 Pro}"
# Embedded single-quoted in the remote command below, so quoting characters and
# whitespace beyond the simulator naming shape are rejected here.
[[ "${prewarm_target}" =~ ^[A-Za-z0-9][A-Za-z0-9._()\ -]{0,79}$ ]] \
  || die "FLANFORGE_PREWARM_TARGET contains unsupported characters"
boot_timeout="${FLANFORGE_SMOKE_BOOT_TIMEOUT_SECONDS:-300}"
test_timeout="${FLANFORGE_SMOKE_TEST_TIMEOUT_SECONDS:-1200}"
for bound in "${boot_timeout}" "${test_timeout}"; do
  [[ "${bound}" =~ ^[1-9][0-9]{0,4}$ ]] \
    || die "smoke timeouts must be positive integers of seconds"
done

guest_script="${template_dir}/scripts/smoke-job-path.sh"
[ -f "${guest_script}" ] || die "guest smoke script not found at '${guest_script}'"

default_guest_identity="${HOME}/Library/Application Support/flanforge/guest-ssh-key"
guest_public_key="${FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE:-${default_guest_identity}.pub}"
guest_identity="${guest_public_key%.pub}"
[ -f "${guest_identity}" ] \
  || die "guest SSH private key not found at '${guest_identity}'"

clone_name="${template_name}-smoke-$$"
known_hosts_file=""
clone_created=false

cleanup() {
  local status=$?
  if [ -n "${known_hosts_file}" ]; then
    rm -f "${known_hosts_file}"
  fi
  if [ "${clone_created}" = true ] && [ "${keep_clone}" != true ]; then
    echo "==> deleting ${clone_name}"
    tart stop "${clone_name}" >/dev/null 2>&1 || true
    tart delete "${clone_name}" >/dev/null 2>&1 \
      || echo "warning: could not delete '${clone_name}'; remove it manually" >&2
  fi
  return "${status}"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

known_hosts_file="$(mktemp "${TMPDIR:-/tmp}/flanforge-smoke-known-hosts.XXXXXX")" \
  || die "cannot create a temporary known_hosts file"
chmod 0600 "${known_hosts_file}"

echo "==> cloning ${template_name} -> ${clone_name}"
tart clone "${template_name}" "${clone_name}"
clone_created=true

echo "==> starting ${clone_name}"
tart run --no-graphics "${clone_name}" >/dev/null 2>&1 &
tart_pid=$!

guest_ip=""
for _ in $(seq 1 60); do
  if ! kill -0 "${tart_pid}" 2>/dev/null; then
    die "the Tart process exited before the guest was reachable"
  fi
  guest_ip="$(tart ip "${clone_name}" 2>/dev/null || true)"
  if [ -n "${guest_ip}" ]; then
    break
  fi
  sleep 2
done
[ -n "${guest_ip}" ] || die "guest did not report an address within 120s"
echo "==> guest address: ${guest_ip}"

# The daemon's own hardening, minus its pinned anchor and its quiet logging: a
# fresh clone's key is recorded on first use, where the daemon pins a recorded
# one, and ssh diagnostics are wanted here.
ssh_options=(
  -F /dev/null
  -o GlobalKnownHostsFile=/dev/null
  -o BatchMode=yes
  -o IdentitiesOnly=yes
  -o ForwardAgent=no
  -o ForwardX11=no
  -o ControlMaster=no
  -o ControlPath=none
  -o PermitLocalCommand=no
  -o ProxyCommand=none
  -o StrictHostKeyChecking=accept-new
  -o UserKnownHostsFile="${known_hosts_file}"
  -o ConnectTimeout=10
  -i "${guest_identity}"
)

for _ in $(seq 1 30); do
  if ssh "${ssh_options[@]}" "runner@${guest_ip}" true >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
ssh "${ssh_options[@]}" "runner@${guest_ip}" true >/dev/null 2>&1 \
  || die "cannot reach the guest as 'runner'; the identity or account is wrong"

echo "==> running the job-time path as runner over SSH"
remote_command="SMOKE_PREWARM_TARGET='${prewarm_target}'"
remote_command="${remote_command} SMOKE_BOOT_TIMEOUT_SECONDS='${boot_timeout}'"
remote_command="${remote_command} SMOKE_TEST_TIMEOUT_SECONDS='${test_timeout}'"
remote_command="${remote_command} bash -s"
smoke_status=0
ssh "${ssh_options[@]}" "runner@${guest_ip}" "${remote_command}" \
  < "${guest_script}" || smoke_status=$?

echo
if [ "${keep_clone}" = true ]; then
  echo "Clone '${clone_name}' left running at ${guest_ip}."
  echo "  ssh -i '${guest_identity}' runner@${guest_ip}"
  echo "  tart stop '${clone_name}' && tart delete '${clone_name}'"
fi

[ "${smoke_status}" -eq 0 ] \
  || die "template '${template_name}' failed the job-time smoke check"
echo "Template '${template_name}' passed the job-time smoke check."
