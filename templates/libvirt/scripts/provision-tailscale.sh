#!/usr/bin/env bash
# Install the upstream Fedora package, then either retain a build-time tailnet
# login or prove the unjoined runtime contract.
set -Eeuo pipefail

trap 'echo "Tailscale provisioning failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

repo_url=https://pkgs.tailscale.com/stable/fedora/tailscale.repo
repo_staging_file="$(mktemp /tmp/flanforge-tailscale-repo.XXXXXX)"
operator_unit_source="${FLANFORGE_TAILSCALE_OPERATOR_UNIT_FILE:-/tmp/flanforge-tailscale-operator.service}"
preauth_key_file="${FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE:-/tmp/flanforge-tailscale-preauth-key}"
argument_helper="${FLANFORGE_TAILSCALE_ARGUMENT_HELPER:-/tmp/flanforge-tailscale-arguments.sh}"
retained_login_marker=/usr/local/share/flanforge/tailscale-retained-login
enabled="${TAILSCALE_ENABLED:-false}"
login_server="${TAILSCALE_LOGIN_SERVER:-}"
node_hostname="${TAILSCALE_HOSTNAME:-}"
extra_args_string="${TAILSCALE_EXTRA_ARGS:-}"
cleanup() {
  rm -f "${repo_staging_file}" "${argument_helper}"
  sudo rm -f "${preauth_key_file}"
}
trap cleanup EXIT

[[ "${enabled}" =~ ^(true|false)$ ]] \
  || { echo "TAILSCALE_ENABLED must be true or false" >&2; exit 1; }
# Narrow the upload window before anything else runs.
if [ -e "${preauth_key_file}" ]; then
  [ ! -L "${preauth_key_file}" ] \
    || { echo "the tailnet key path must not be a symlink" >&2; exit 1; }
  sudo chmod 0600 "${preauth_key_file}"
fi

[ -f "${operator_unit_source}" ] && [ ! -L "${operator_unit_source}" ]
[ "$(wc -c < "${operator_unit_source}")" -le 8192 ]
[ "$(grep -c '^ExecStart=/usr/bin/tailscale set --operator=runner$' \
  "${operator_unit_source}")" -eq 1 ]
grep -Fqx 'Requires=tailscaled.service' "${operator_unit_source}"
grep -Fqx 'After=tailscaled.service' "${operator_unit_source}"
grep -Fqx 'Before=sshd.service' "${operator_unit_source}"
grep -Fqx 'WantedBy=multi-user.target' "${operator_unit_source}"

curl --fail --location --proto '=https' --tlsv1.2 \
  --connect-timeout 30 --max-time 120 --max-filesize 16384 \
  --output "${repo_staging_file}" "${repo_url}"
[ -s "${repo_staging_file}" ] && [ ! -L "${repo_staging_file}" ]
[ "$(wc -l < "${repo_staging_file}")" -eq 8 ]
[ "$(grep -c '^\[tailscale-stable\]$' "${repo_staging_file}")" -eq 1 ]
[ "$(grep -c '^\[' "${repo_staging_file}")" -eq 1 ]
grep -Fqx 'name=Tailscale stable' "${repo_staging_file}"
grep -Fqx 'baseurl=https://pkgs.tailscale.com/stable/fedora/$basearch' \
  "${repo_staging_file}"
grep -Fqx 'gpgkey=https://pkgs.tailscale.com/stable/fedora/repo.gpg' \
  "${repo_staging_file}"
grep -Fqx 'enabled=1' "${repo_staging_file}"
grep -Fqx 'type=rpm' "${repo_staging_file}"
grep -Fqx 'repo_gpgcheck=1' "${repo_staging_file}"
grep -Fqx 'gpgcheck=1' "${repo_staging_file}"
[ "$(grep -Ec '^(baseurl|mirrorlist|metalink|gpgkey)=' \
  "${repo_staging_file}")" -eq 2 ]

sudo install -m 0644 -o root -g root "${repo_staging_file}" \
  /etc/yum.repos.d/tailscale.repo
sudo dnf -y install tailscale
[ "$(command -v tailscale)" = /usr/bin/tailscale ]
# Fedora 42 merged /usr/sbin into /usr/bin; either resolution is the packaged one.
[[ "$(command -v tailscaled)" =~ ^/usr/s?bin/tailscaled$ ]]

sudo install -m 0644 -o root -g root "${operator_unit_source}" \
  /etc/systemd/system/flanforge-tailscale-operator.service
rm -f "${operator_unit_source}"
sudo systemd-analyze verify \
  /etc/systemd/system/flanforge-tailscale-operator.service
sudo systemctl daemon-reload
sudo systemctl enable --now tailscaled.service
sudo tailscale logout >/dev/null 2>&1 || true
sudo systemctl enable --now flanforge-tailscale-operator.service
sudo -H -u runner /usr/bin/tailscale set --operator=runner
sudo -H -u runner /usr/bin/tailscale debug prefs \
  | grep -Eq '"OperatorUser"[[:space:]]*:[[:space:]]*"runner"'

if [ "${enabled}" = false ]; then
  tailscale_status="$(sudo -H -u runner /usr/bin/tailscale status --json)"
  printf '%s' "${tailscale_status}" | jq -e '
    .BackendState != "Running" and
    ((.TailscaleIPs // []) | length == 0) and
    ((.HaveNodeKey // false) == false) and
    ((.CurrentTailnet // null) == null)
  ' >/dev/null
  echo "==> Tailscale installed, enabled, and left unjoined"
  exit 0
fi

# Structural only, and never printed: the key stays an opaque credential.
[ -f "${preauth_key_file}" ] && [ -s "${preauth_key_file}" ] \
  || { echo "a tailnet pre-auth key is required when Tailscale is enabled" >&2; exit 1; }
[ "$(wc -c < "${preauth_key_file}")" -le 513 ] \
  && [ "$(wc -l < "${preauth_key_file}")" -le 1 ] \
  && LC_ALL=C grep -Eq '^[[:graph:]]{8,512}$' "${preauth_key_file}" \
  || { echo "the tailnet pre-auth key is structurally invalid" >&2; exit 1; }

[[ "${login_server}" =~ ^https://[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?(:[0-9]{1,5})?/?$ ]] \
  || { echo "TAILSCALE_LOGIN_SERVER must be an HTTPS origin" >&2; exit 1; }
if [ -n "${node_hostname}" ]; then
  [[ "${node_hostname}" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$ ]] \
    || { echo "TAILSCALE_HOSTNAME must be a valid host label" >&2; exit 1; }
fi

[ -f "${argument_helper}" ] && [ ! -L "${argument_helper}" ] \
  || { echo "Tailscale argument helper is unavailable" >&2; exit 1; }
# shellcheck source=../../shared/tailscale/arguments.sh
source "${argument_helper}"
parse_tailscale_extra_arguments "${extra_args_string}" \
  || { echo "TAILSCALE_EXTRA_ARGS contains invalid or reserved options" >&2; exit 1; }

up_args=("--login-server=${login_server}")
[ -z "${node_hostname}" ] || up_args+=("--hostname=${node_hostname}")
if [ "${TAILSCALE_EXTRA_ARGUMENT_COUNT}" -gt 0 ]; then
  up_args+=("${TAILSCALE_EXTRA_ARGUMENTS[@]}")
fi

sudo /usr/bin/tailscale up "--auth-key=file:${preauth_key_file}" "${up_args[@]}"
sudo rm -f "${preauth_key_file}"
# The login starts a new profile, so the operator grant must be re-asserted.
sudo /usr/bin/tailscale set --operator=runner

for _ in {1..30}; do
  tailscale_status="$(sudo -H -u runner /usr/bin/tailscale status --json)"
  if printf '%s' "${tailscale_status}" | jq -e '
    .BackendState == "Running" and
    ((.TailscaleIPs // []) | length > 0) and
    ((.HaveNodeKey // false) == true)
  ' >/dev/null; then
    sudo install -D -m 0644 -o root -g root /dev/null "${retained_login_marker}"
    echo "==> Tailscale installed and authenticated; the login is retained"
    exit 0
  fi
  sleep 2
done

echo "Tailscale did not reach Running state" >&2
exit 1
