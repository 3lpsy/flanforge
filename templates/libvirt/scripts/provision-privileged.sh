#!/usr/bin/env bash
# Create the optional privileged automation account the daemon drives for
# root-needing clone-identity generalization. Disabled builds bake none.
set -Eeuo pipefail

trap 'echo "privileged-account provisioning failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

enabled="${PRIVILEGED_ACCOUNT_ENABLED:-true}"
password_file="${FLANFORGE_PRIVILEGED_SUDO_PASSWORD_FILE:-/tmp/flanforge-privileged-sudo-password}"
account=prunner
account_uid=2001
account_home="${FLANFORGE_PRIVILEGED_HOME:-/home/prunner}"
record="${FLANFORGE_SHARE_DIR:-/usr/local/share/flanforge}/privileged-account"
sudoers="${FLANFORGE_PRIVILEGED_SUDOERS_FILE:-/etc/sudoers.d/50-flanforge-prunner}"

cleanup() { sudo rm -f "${password_file}"; }
trap cleanup EXIT

[[ "${enabled}" =~ ^(true|false)$ ]] \
  || { echo "PRIVILEGED_ACCOUNT_ENABLED must be true or false" >&2; exit 1; }
# Narrow the upload window before anything else runs.
if [ -e "${password_file}" ]; then
  [ ! -L "${password_file}" ] \
    || { echo "the sudo password path must not be a symlink" >&2; exit 1; }
  sudo chmod 0600 "${password_file}"
fi

if [ "${enabled}" = false ]; then
  ! getent passwd "${account}" >/dev/null \
    || { echo "privileged account exists but is disabled" >&2; exit 1; }
  [ ! -e "${sudoers}" ] || { echo "stale privileged sudoers rule" >&2; exit 1; }
  echo "==> privileged account omitted"
  exit 0
fi

if id "${account}" >/dev/null 2>&1; then
  [ "$(id -u "${account}")" -eq "${account_uid}" ] \
    || { echo "existing ${account} account has the wrong UID" >&2; exit 1; }
else
  sudo useradd --create-home --home-dir "${account_home}" \
    --shell /bin/bash --uid "${account_uid}" --user-group "${account}"
fi
sudo install -d -m 0700 -o "${account}" -g "${account}" "${account_home}"

# Structural only, and never printed: the password stays an opaque credential.
password_mode=false
if [ -s "${password_file}" ]; then
  [ "$(wc -c < "${password_file}")" -le 513 ] \
    && [ "$(wc -l < "${password_file}")" -le 1 ] \
    && sudo grep -Eq '^[[:graph:]]{8,512}$' "${password_file}" \
    || { echo "the sudo password is structurally invalid" >&2; exit 1; }
  password_mode=true
fi

if [ "${password_mode}" = true ]; then
  { printf '%s:' "${account}"; sudo cat "${password_file}"; } | sudo chpasswd
  rule="${account} ALL=(ALL) ALL"
else
  sudo passwd --lock "${account}" >/dev/null
  rule="${account} ALL=(ALL) NOPASSWD: ALL"
fi
sudo rm -f "${password_file}"

sudoers_staging="$(mktemp "${TMPDIR:-/tmp}/flanforge-prunner-sudoers.XXXXXX")"
printf '%s\n' "${rule}" > "${sudoers_staging}"
sudo visudo -cf "${sudoers_staging}" >/dev/null
sudo install -m 0440 -o root -g root "${sudoers_staging}" "${sudoers}"
rm -f "${sudoers_staging}"

sudo install -d -m 0755 -o root -g root "$(dirname "${record}")"
printf '%s:%s\n' "${account}" "${account_uid}" | sudo tee "${record}" >/dev/null
sudo chown root:root "${record}"
sudo chmod 0644 "${record}"

# Prove the grant the daemon will rely on; password mode is for operators, so
# the only non-interactive proof there is that passwordless sudo is refused.
if [ "${password_mode}" = true ]; then
  ! sudo -H -u "${account}" sudo -n true >/dev/null 2>&1 \
    || { echo "password-mode sudo unexpectedly works without a password" >&2; exit 1; }
  echo "==> privileged account ${account} baked with password-gated sudo"
else
  sudo -H -u "${account}" sudo -n true \
    || { echo "passwordless sudo does not work for ${account}" >&2; exit 1; }
  echo "==> privileged account ${account} baked with passwordless sudo"
fi
