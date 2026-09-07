#!/usr/bin/env bash
# Exercise Tailscale repository and identity checks without installing packages.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-tailscale-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-tailscale-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

fake_bin="${test_root}/bin"
fake_guest_root="${test_root}/guest"
command_log="${test_root}/commands"
operator_unit_staging="${test_root}/flanforge-tailscale-operator.service"
preauth_key_staging="${test_root}/preauth-key"
tailscale_state_file="${test_root}/tailscale-joined"
argument_helper_source="${template_dir}/../shared/tailscale/arguments.sh"
argument_helper_staging="${test_root}/flanforge-tailscale-arguments.sh"
login_server="https://headscale.example.com"
# An obvious fixture: pre-auth keys are opaque, so no vendor prefix is implied.
fixture_key="preauth-key-fixture-0123456789"
retained_login_marker="usr/local/share/flanforge/tailscale-retained-login"
mkdir -p "${fake_bin}" "${fake_guest_root}"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'output_file=""' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in --output) output_file="$2"; shift 2 ;; *) shift ;; esac' \
  'done' \
  '[ -n "${output_file}" ]' \
  'printf "%s\n" "[tailscale-stable]" "name=Tailscale stable" "baseurl=https://pkgs.tailscale.com/stable/fedora/\$basearch" "enabled=1" "type=rpm" "repo_gpgcheck=1" "gpgcheck=${FAKE_GPGCHECK:-1}" "gpgkey=https://pkgs.tailscale.com/stable/fedora/repo.gpg" > "${output_file}"' \
  > "${fake_bin}/curl"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'printf "%s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'while [ "${1:-}" = -H ] || [ "${1:-}" = -u ]; do' \
  '  if [ "$1" = -u ]; then shift 2; else shift; fi' \
  'done' \
  'case "$1" in' \
  '  install)' \
  '    source_file="${@: -2:1}"; destination="${@: -1}"' \
  '    destination="${FAKE_GUEST_ROOT}${destination}"' \
  '    mkdir -p "$(dirname "${destination}")"' \
  '    cp "${source_file}" "${destination}" ;;' \
  '  dnf|systemctl|systemd-analyze) exit 0 ;;' \
  '  chmod|rm) exec "$@" ;;' \
  '  tailscale|/usr/bin/tailscale) shift; exec "${FAKE_BIN}/tailscale" "$@" ;;' \
  '  *) echo "unexpected sudo command: $1" >&2; exit 2 ;;' \
  'esac' > "${fake_bin}/sudo"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'printf "tailscale %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'case "$1" in' \
  '  logout|set) exit 0 ;;' \
  '  up) : > "${FAKE_TAILSCALE_STATE}"; exit 0 ;;' \
  '  debug) printf "%s\n" '\''{"OperatorUser":"runner"}'\'' ;;' \
  '  status)' \
  '    if [ "${FAKE_TAILSCALE_JOINED:-false}" = true ] \' \
  '      || [ -f "${FAKE_TAILSCALE_STATE}" ]; then' \
  '      printf "%s\n" '\''{"BackendState":"Running","HaveNodeKey":true,"TailscaleIPs":["100.64.0.1"],"CurrentTailnet":{}}'\'' ' \
  '    else' \
  '      printf "%s\n" '\''{"BackendState":"NeedsLogin","TailscaleIPs":[]}'\'' ' \
  '    fi ;;' \
  '  *) echo "unexpected tailscale command: $*" >&2; exit 2 ;;' \
  'esac' > "${fake_bin}/tailscale"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "${fake_bin}/tailscaled"
chmod 0700 "${fake_bin}"/*

command() {
  if [ "${1:-}" = -v ] && [ "${2:-}" = tailscale ]; then
    printf '%s\n' /usr/bin/tailscale
  elif [ "${1:-}" = -v ] && [ "${2:-}" = tailscaled ]; then
    printf '%s\n' /usr/sbin/tailscaled
  else
    builtin command "$@"
  fi
}
export -f command

run_fixture() {
  cp "${template_dir}/systemd/flanforge-tailscale-operator.service" \
    "${operator_unit_staging}"
  cp "${argument_helper_source}" "${argument_helper_staging}"
  rm -f "${tailscale_state_file}"
  env PATH="${fake_bin}:${PATH}" FAKE_BIN="${fake_bin}" \
    FAKE_COMMAND_LOG="${command_log}" FAKE_GUEST_ROOT="${fake_guest_root}" \
    FAKE_TAILSCALE_STATE="${tailscale_state_file}" \
    FLANFORGE_TAILSCALE_OPERATOR_UNIT_FILE="${operator_unit_staging}" \
    FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE="${preauth_key_staging}" \
    FLANFORGE_TAILSCALE_ARGUMENT_HELPER="${argument_helper_staging}" \
    "$@" bash "${template_dir}/scripts/provision-tailscale.sh" >/dev/null
}

stage_preauth_key() {
  printf '%s\n' "$1" > "${preauth_key_staging}"
  chmod 0600 "${preauth_key_staging}"
}

run_fixture
grep -Fqx 'dnf -y install tailscale' "${command_log}"
grep -Fqx 'systemctl enable --now tailscaled.service' "${command_log}"
grep -Fqx 'systemctl enable --now flanforge-tailscale-operator.service' \
  "${command_log}"
grep -Fqx -- '-H -u runner /usr/bin/tailscale set --operator=runner' \
  "${command_log}"
grep -Fqx 'gpgcheck=1' \
  "${fake_guest_root}/etc/yum.repos.d/tailscale.repo"
cmp "${template_dir}/systemd/flanforge-tailscale-operator.service" \
  "${fake_guest_root}/etc/systemd/system/flanforge-tailscale-operator.service"

: > "${command_log}"
if run_fixture FAKE_GPGCHECK=0 >/dev/null 2>&1; then
  echo "unsigned Tailscale repository configuration was accepted" >&2
  exit 1
fi
if grep -Fq 'dnf ' "${command_log}"; then
  echo "package installation ran before repository validation" >&2
  exit 1
fi

if run_fixture FAKE_TAILSCALE_JOINED=true >/dev/null 2>&1; then
  echo "joined Tailscale state was accepted" >&2
  exit 1
fi

# A pre-auth key is the only switch, and it must never survive or be echoed.
: > "${command_log}"
stage_preauth_key "${fixture_key}"
run_fixture TAILSCALE_ENABLED=true \
  TAILSCALE_LOGIN_SERVER="${login_server}" \
  TAILSCALE_HOSTNAME=flanforge-linux-ci \
  TAILSCALE_EXTRA_ARGS='--accept-dns=true --shields-up'
grep -Fqx -- "tailscale up --auth-key=file:${preauth_key_staging} \
--login-server=${login_server} --hostname=flanforge-linux-ci \
--accept-dns=true --shields-up" "${command_log}"
grep -Fqx -- 'tailscale set --operator=runner' "${command_log}"
[ ! -e "${preauth_key_staging}" ] \
  || { echo "the pre-auth key survived provisioning" >&2; exit 1; }
[ ! -e "${argument_helper_staging}" ] \
  || { echo "the argument helper survived provisioning" >&2; exit 1; }
[ -f "${argument_helper_source}" ] \
  || { echo "the shared argument helper was removed" >&2; exit 1; }
if grep -Fq "${fixture_key}" "${command_log}"; then
  echo "the pre-auth key reached a command line" >&2
  exit 1
fi
[ -f "${fake_guest_root}/${retained_login_marker}" ] \
  || { echo "the retained-login marker was not installed" >&2; exit 1; }
rm -f "${fake_guest_root}/${retained_login_marker}"

# Everything below must fail closed before any join is attempted.
for invalid in \
  "TAILSCALE_LOGIN_SERVER=http://headscale.example.com" \
  "TAILSCALE_HOSTNAME=-invalid-label" \
  "TAILSCALE_EXTRA_ARGS=--login-server=https://other.example.com"; do
  : > "${command_log}"
  stage_preauth_key "${fixture_key}"
  if run_fixture TAILSCALE_ENABLED=true \
    TAILSCALE_LOGIN_SERVER="${login_server}" "${invalid}" >/dev/null 2>&1; then
    echo "invalid Tailscale input was accepted: ${invalid}" >&2
    exit 1
  fi
  if grep -Fq 'tailscale up' "${command_log}"; then
    echo "a join was attempted with invalid input: ${invalid}" >&2
    exit 1
  fi
done

for invalid_key in "" "short" "key with spaces"; do
  : > "${command_log}"
  stage_preauth_key "${invalid_key}"
  if run_fixture TAILSCALE_ENABLED=true \
    TAILSCALE_LOGIN_SERVER="${login_server}" >/dev/null 2>&1; then
    echo "a malformed pre-auth key was accepted" >&2
    exit 1
  fi
  if grep -Fq 'tailscale up' "${command_log}"; then
    echo "a join was attempted with a malformed key" >&2
    exit 1
  fi
done

: > "${command_log}"
rm -f "${preauth_key_staging}"
if run_fixture TAILSCALE_ENABLED=true \
  TAILSCALE_LOGIN_SERVER="${login_server}" >/dev/null 2>&1; then
  echo "an enabled build without a pre-auth key was accepted" >&2
  exit 1
fi

# The finalizer keeps a retained identity and scrubs it in every other build.
grep -Fq '/usr/local/share/flanforge/tailscale-retained-login' \
  "${template_dir}/scripts/finalize.sh"
grep -Fq 'retain_tailscale_login' "${template_dir}/scripts/finalize.sh"
grep -Fq 'TAILSCALE_ENABLED' "${template_dir}/scripts/provision-verify.sh"
grep -Fq '/usr/local/share/flanforge/tailscale-retained-login' \
  "${template_dir}/host/test-tailscale.sh"
grep -Fq 'var.tailscale_preauth_key_file' "${template_dir}/flanforge-base.pkr.hcl"
grep -Fq 'shared/tailscale/arguments.sh' "${template_dir}/flanforge-base.pkr.hcl"
grep -Fq 'local.tailscale_environment' "${template_dir}/flanforge-base.pkr.hcl"
if grep -Eq 'TAILSCALE_PREAUTH_KEY=' "${template_dir}/flanforge-base.pkr.hcl"; then
  echo "the pre-auth key was exposed through a provisioner environment" >&2
  exit 1
fi

# shellcheck source=../host/test-tailscale.sh
source "${template_dir}/host/test-tailscale.sh"
remote_tailscale_contract=""
build_remote_tailscale_contract
grep -Fq '/usr/bin/tailscale' <<< "${remote_tailscale_contract}"
grep -Fq '.BackendState != "Running"' <<< "${remote_tailscale_contract}"
grep -Fq 'systemctl is-active flanforge-tailscale-operator.service' \
  <<< "${remote_tailscale_contract}"
operator_stop_order="$(grep -n \
  'systemctl stop flanforge-tailscale-operator.service' \
  "${template_dir}/scripts/finalize.sh" | cut -d: -f1)"
tailscaled_stop_order="$(grep -n 'systemctl stop tailscaled.service' \
  "${template_dir}/scripts/finalize.sh" | cut -d: -f1)"
state_scrub_order="$(grep -n \
  'for tailscale_dir in /var/lib/tailscale /var/cache/tailscale' \
  "${template_dir}/scripts/finalize.sh" | cut -d: -f1)"
[ "${operator_stop_order}" -lt "${tailscaled_stop_order}" ]
[ "${tailscaled_stop_order}" -lt "${state_scrub_order}" ]
grep -Fq 'systemctl stop tailscaled.service' "${template_dir}/scripts/finalize.sh"
grep -Fq '/var/lib/tailscale /var/cache/tailscale' \
  "${template_dir}/scripts/finalize.sh"
grep -Fqx 'Requires=tailscaled.service' \
  "${template_dir}/systemd/flanforge-tailscale-operator.service"
grep -Fqx 'After=tailscaled.service' \
  "${template_dir}/systemd/flanforge-tailscale-operator.service"
grep -Fqx 'Before=sshd.service' \
  "${template_dir}/systemd/flanforge-tailscale-operator.service"
grep -Fqx 'ExecStart=/usr/bin/tailscale set --operator=runner' \
  "${template_dir}/systemd/flanforge-tailscale-operator.service"
grep -Fqx 'WantedBy=multi-user.target' \
  "${template_dir}/systemd/flanforge-tailscale-operator.service"
grep -Fq 'systemd/flanforge-tailscale-operator.service' \
  "${template_dir}/flanforge-base.pkr.hcl"
runner_order="$(grep -n 'scripts/provision-runner.sh' \
  "${template_dir}/flanforge-base.pkr.hcl" | cut -d: -f1)"
tailscale_order="$(grep -n 'scripts/provision-tailscale.sh' \
  "${template_dir}/flanforge-base.pkr.hcl" | cut -d: -f1)"
[ "${runner_order}" -lt "${tailscale_order}" ] \
  || { echo "Tailscale provisioning ran before the runner existed" >&2; exit 1; }

echo "Tailscale provisioning fake tests passed"
