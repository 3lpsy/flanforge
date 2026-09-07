#!/bin/bash
# Install Tailscale and optionally retain a Headscale/Tailscale login in the
# template. Retaining login state intentionally gives every clone the same
# node identity; use FlanForge's runtime bootstrap for per-allocation nodes.
set -euo pipefail

eval "$(/opt/homebrew/bin/brew shellenv)"
export HOMEBREW_NO_AUTO_UPDATE=1

enabled="${TAILSCALE_ENABLED:-false}"
login_server="${TAILSCALE_LOGIN_SERVER:-}"
hostname="${TAILSCALE_HOSTNAME:-}"
extra_args_string="${TAILSCALE_EXTRA_ARGS:-}"
argument_helper="${FLANFORGE_TAILSCALE_ARGUMENT_HELPER:-/tmp/flanforge-tailscale-arguments.sh}"

[ -f "${argument_helper}" ] && [ ! -L "${argument_helper}" ] \
  || { echo "Tailscale argument helper is unavailable" >&2; exit 1; }
# shellcheck source=../../shared/tailscale/arguments.sh
source "${argument_helper}"

[[ "${enabled}" =~ ^(true|false)$ ]] \
  || { echo "TAILSCALE_ENABLED must be true or false" >&2; exit 1; }

# The join key arrives as an uploaded file rather than an environment value, so
# it is never on a command line and no other provisioning script can read it.
preauth_key_file=/tmp/flanforge-tailscale-preauth-key
cleanup_preauth_key() {
  sudo rm -f "${preauth_key_file}"
}
trap cleanup_preauth_key EXIT
[ ! -e "${preauth_key_file}" ] || sudo chmod 0600 "${preauth_key_file}"

brew install tailscale
sudo /opt/homebrew/bin/tailscaled install-system-daemon

# A custom or reused base must not silently retain a previous node identity.
sudo /opt/homebrew/bin/tailscale logout >/dev/null 2>&1 || true

# Runtime bootstrap is performed as the non-admin runner account before its
# Forgejo process starts. Operator access controls Tailscale only; it does not
# grant macOS administrator privileges. The operator is a per-profile
# preference, so assert it after any login rather than only once.
ensure_operator() {
  sudo /opt/homebrew/bin/tailscale set --operator=runner
}

if [ "${enabled}" = false ]; then
  ensure_operator
  echo "==> Tailscale installed and logged out"
  exit 0
fi

[ -s "${preauth_key_file}" ] \
  || { echo "a tailnet join key is required when Tailscale is enabled" >&2; exit 1; }
[[ "${login_server}" =~ ^https?://[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?(:[0-9]{1,5})?/?$ ]] \
  || { echo "TAILSCALE_LOGIN_SERVER must be an HTTP(S) origin" >&2; exit 1; }
if [ -n "${hostname}" ]; then
  [[ "${hostname}" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$ ]] \
    || { echo "TAILSCALE_HOSTNAME must be a valid host label" >&2; exit 1; }
fi

parse_tailscale_extra_arguments "${extra_args_string}" \
  || { echo "TAILSCALE_EXTRA_ARGS contains invalid or reserved options" >&2; exit 1; }

# Built as one always-populated array: macOS ships bash 3.2, where expanding an
# empty array under `set -u` aborts the script.
up_args=(
  "--login-server=${login_server}"
)
if [ -n "${hostname}" ]; then
  up_args+=("--hostname=${hostname}")
fi
if [ "${TAILSCALE_EXTRA_ARGUMENT_COUNT}" -gt 0 ]; then
  up_args+=("${TAILSCALE_EXTRA_ARGUMENTS[@]}")
fi

sudo /opt/homebrew/bin/tailscale up \
  "--auth-key=file:${preauth_key_file}" \
  "${up_args[@]}"
cleanup_preauth_key
trap - EXIT
ensure_operator

for _ in {1..30}; do
  if sudo /opt/homebrew/bin/tailscale status --json 2>/dev/null \
    | grep -Eq '"BackendState"[[:space:]]*:[[:space:]]*"Running"'; then
    echo "==> Tailscale authenticated; system daemon will reconnect on boot"
    exit 0
  fi
  sleep 2
done

echo "Tailscale did not reach Running state" >&2
exit 1
