#!/usr/bin/env bash
# Exercise the SELinux disable rewrite against fixture configs.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-selinux-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-selinux-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

provisioner="${template_dir}/scripts/provision-selinux.sh"
fake_bin="${test_root}/bin"
command_log="${test_root}/commands"
config="${test_root}/selinux-config"
mkdir -p "${fake_bin}"

die() { echo "selinux test: $1" >&2; exit 1; }

# Ownership flags are dropped so the recorded install still works unprivileged.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'printf "sudo %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'case "$1" in' \
  '  install)' \
  '    shift; args=()' \
  '    while [ "$#" -gt 0 ]; do' \
  '      case "$1" in -o|-g) shift 2 ;; *) args+=("$1"); shift ;; esac' \
  '    done' \
  '    exec install "${args[@]}" ;;' \
  '  setenforce) exit 0 ;;' \
  '  *) echo "unexpected sudo command: $1" >&2; exit 2 ;;' \
  'esac' > "${fake_bin}/sudo"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "%s\n" "${FAKE_GETENFORCE}"' > "${fake_bin}/getenforce"
chmod 0755 "${fake_bin}/sudo" "${fake_bin}/getenforce"

run_provisioner() {
  set +e
  env PATH="${fake_bin}:${PATH}" \
    FAKE_COMMAND_LOG="${command_log}" \
    FAKE_GETENFORCE="${FAKE_GETENFORCE:-Enforcing}" \
    FLANFORGE_SELINUX_CONFIG="${config}" \
    bash "${provisioner}" >"${test_root}/stdout" 2>&1
  status=$?
  set -e
}

# 1. An enforcing config is rewritten to disabled, keeping every other line,
#    and the running build is dropped out of enforcement.
: > "${command_log}"
printf '%s\n' '# comment' 'SELINUX=enforcing' 'SELINUXTYPE=targeted' > "${config}"
run_provisioner
[ "${status}" -eq 0 ] || die "an enforcing config was refused ($(cat "${test_root}/stdout"))"
grep -Fxq 'SELINUX=disabled' "${config}" || die "config was not rewritten"
grep -Fxq 'SELINUXTYPE=targeted' "${config}" || die "unrelated lines were not kept"
grep -Fxq '# comment' "${config}" || die "comments were not kept"
grep -q 'sudo setenforce 0' "${command_log}" || die "the running build was left enforcing"

# 2. A permissive build is not setenforce'd again.
: > "${command_log}"
printf '%s\n' 'SELINUX=permissive' > "${config}"
FAKE_GETENFORCE=Permissive run_provisioner
[ "${status}" -eq 0 ] || die "a permissive config was refused"
grep -Fxq 'SELINUX=disabled' "${config}" || die "a permissive config was not rewritten"
! grep -q 'sudo setenforce' "${command_log}" || die "setenforce ran without enforcement"

# 3. A guest without SELinux is a note, never a failure.
rm -f "${config}"
run_provisioner
[ "${status}" -eq 0 ] || die "a missing config failed the build"
grep -q 'nothing to disable' "${test_root}/stdout" || die "the skip was not named"

# 4. Ambiguous or symlinked configs are refused rather than edited.
printf '%s\n' 'SELINUX=enforcing' 'SELINUX=permissive' > "${config}"
run_provisioner
[ "${status}" -ne 0 ] || die "a double assignment was accepted"
printf '%s\n' 'SELINUX=enforcing' > "${test_root}/real-config"
ln -sf "${test_root}/real-config" "${config}"
run_provisioner
[ "${status}" -ne 0 ] || die "a symlinked config was accepted"

echo "selinux provisioning fake tests passed"
