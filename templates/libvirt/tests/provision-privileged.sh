#!/usr/bin/env bash
# Exercise the privileged-account provisioner's decisions against fakes.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-privileged-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-privileged-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

provisioner="${template_dir}/scripts/provision-privileged.sh"
fake_bin="${test_root}/bin"
command_log="${test_root}/commands"
mkdir -p "${fake_bin}"

die() { echo "privileged provisioner test: $1" >&2; exit 1; }

# sudo records everything; file verbs run unprivileged against the fake root,
# account and password verbs are logged no-ops, and the nested sudo proof
# answers from FAKE_SUDO_PROOF so both grant modes are exercised.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'printf "sudo %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'if [ "${1:-}" = -H ] && [ "${2:-}" = -u ]; then' \
  '  exit "${FAKE_SUDO_PROOF:-0}"' \
  'fi' \
  'case "$1" in' \
  '  useradd|passwd|visudo|chown) exit 0 ;;' \
  '  chpasswd) exec cat >/dev/null ;;' \
  '  install)' \
  '    arguments=()' \
  '    shift' \
  '    while [ "$#" -gt 0 ]; do' \
  '      case "$1" in -o|-g) shift 2 ;; *) arguments+=("$1"); shift ;; esac' \
  '    done' \
  '    exec /usr/bin/install "${arguments[@]}" ;;' \
  '  tee|chmod|rm|cat|grep) exec "$@" ;;' \
  '  *) echo "unexpected sudo command: $1" >&2; exit 2 ;;' \
  'esac' > "${fake_bin}/sudo"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "getent %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  '[ "${FAKE_ACCOUNT_EXISTS:-false}" = true ]' > "${fake_bin}/getent"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'if [ "${1:-}" = -u ]; then printf "2001\n"; fi' \
  '[ "${FAKE_ACCOUNT_EXISTS:-false}" = true ]' > "${fake_bin}/id"
chmod 0755 "${fake_bin}"/*

run_case() {
  case_root="${test_root}/${1}"
  mkdir -p "${case_root}/share" "${case_root}/home"
  : > "${command_log}"
  env PATH="${fake_bin}:${PATH}" \
    FAKE_COMMAND_LOG="${command_log}" \
    FAKE_ACCOUNT_EXISTS="${FAKE_ACCOUNT_EXISTS:-false}" \
    FAKE_SUDO_PROOF="${FAKE_SUDO_PROOF:-0}" \
    PRIVILEGED_ACCOUNT_ENABLED="${2}" \
    FLANFORGE_SHARE_DIR="${case_root}/share" \
    FLANFORGE_PRIVILEGED_HOME="${case_root}/home" \
    FLANFORGE_PRIVILEGED_SUDOERS_FILE="${case_root}/sudoers" \
    FLANFORGE_PRIVILEGED_SUDO_PASSWORD_FILE="${password_file:-${case_root}/no-password}" \
    bash "${provisioner}"
}

# 1. Disabled with no account is a clean no-op that records nothing.
password_file=""
run_case disabled false >/dev/null || die "the disabled case was refused"
[ ! -e "${test_root}/disabled/share/privileged-account" ] \
  || die "a disabled build wrote the account record"

# 2. Disabled while the account exists refuses: the image would lie.
FAKE_ACCOUNT_EXISTS=true
if run_case disabled-stale false >/dev/null 2>&1; then
  die "a stale privileged account was accepted while disabled"
fi
FAKE_ACCOUNT_EXISTS=false

# 3. The default: passwordless sudo, locked password, record written.
run_case default true >/dev/null || die "the default case was refused"
grep -Fqx 'prunner ALL=(ALL) NOPASSWD: ALL' "${test_root}/default/sudoers" \
  || die "the default sudoers rule is not passwordless"
grep -Fqx 'prunner:2001' "${test_root}/default/share/privileged-account" \
  || die "the account record is wrong"
grep -q 'sudo passwd --lock prunner' "${command_log}" \
  || die "the default did not lock the password"
grep -q 'sudo visudo -cf' "${command_log}" \
  || die "the sudoers rule was installed unvalidated"

# 4. A password swaps the rule to password-gated sudo and is consumed.
password_file="${test_root}/sudo-pass"
printf 'correct-horse-battery\n' > "${password_file}"
FAKE_SUDO_PROOF=1
run_case password true >/dev/null || die "the password case was refused"
FAKE_SUDO_PROOF=0
grep -Fqx 'prunner ALL=(ALL) ALL' "${test_root}/password/sudoers" \
  || die "the password-mode rule still grants NOPASSWD"
grep -q 'sudo chpasswd' "${command_log}" || die "the password was not applied"
[ ! -e "${password_file}" ] || die "the sudo password file was retained"

# 5. Structurally invalid passwords are refused before any account change.
for invalid in 'short' $'two\nlines\nhere'; do
  password_file="${test_root}/invalid-sudo-pass"
  printf '%s\n' "${invalid}" > "${password_file}"
  if run_case invalid true >/dev/null 2>&1; then
    die "an invalid sudo password was accepted"
  fi
done

# 6. A symlinked password path is refused before anything reads it.
password_file="${test_root}/symlinked-sudo-pass"
printf 'correct-horse-battery\n' > "${password_file}.real"
ln -s "${password_file}.real" "${password_file}"
if run_case symlink true >/dev/null 2>&1; then
  die "a symlinked password path was accepted"
fi

echo "privileged-account provisioner fake tests passed"
