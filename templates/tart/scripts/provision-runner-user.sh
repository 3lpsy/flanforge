#!/bin/bash
# Create the unprivileged account used for SSH staging and Forgejo jobs.
set -euo pipefail

public_key=/tmp/flanforge-guest.pub
runner_binary=/tmp/flanforge-forgejo-runner
[ -f "${public_key}" ] || { echo "missing uploaded guest SSH public key" >&2; exit 1; }
[ "$(awk 'NF { count += 1 } END { print count + 0 }' "${public_key}")" -eq 1 ] \
  || { echo "guest SSH public key must contain exactly one non-empty line" >&2; exit 1; }
ssh-keygen -l -f "${public_key}" >/dev/null 2>&1 \
  || { echo "guest SSH public key is invalid" >&2; exit 1; }
[ -s "${runner_binary}" ] \
  || { echo "missing uploaded Forgejo Runner binary" >&2; exit 1; }
LC_ALL=C file "${runner_binary}" | grep -Eq 'Mach-O.*arm64' \
  || { echo "Forgejo Runner must be a Darwin ARM64 Mach-O executable" >&2; exit 1; }

if ! id -u runner >/dev/null 2>&1; then
  # Start with a new home instead of inheriting image-owned files or following
  # a compatibility symlink into another account.
  if [ -e /Users/runner ] || [ -L /Users/runner ]; then
    sudo rm -rf /Users/runner
  fi

  # No known password is retained. SSH public-key authentication is the only
  # intended login path, and the account is deliberately not an administrator.
  generated_password="$(uuidgen)$(uuidgen)"
  sudo sysadminctl -addUser runner \
    -fullName "FlanForge Runner" \
    -home /Users/runner \
    -shell /bin/zsh \
    -password "${generated_password}"
  unset generated_password
  sudo /usr/sbin/createhomedir -c -u runner >/dev/null
fi

runner_home="$(dscl . -read /Users/runner NFSHomeDirectory | awk '{ print $2 }')"
[ "${runner_home}" = /Users/runner ] \
  || { echo "runner home must be /Users/runner" >&2; exit 1; }

sudo install -d -m 0700 -o runner -g staff "${runner_home}"
sudo install -d -m 0700 -o runner -g staff "${runner_home}/.ssh" "${runner_home}/bin"
sudo chown runner:staff "${runner_home}"
sudo chmod 0700 "${runner_home}"
sudo -H -u admin test -x /Users/admin \
  || { echo "admin home became inaccessible while creating runner" >&2; exit 1; }
sudo -H -u runner test -w "${runner_home}" \
  || { echo "runner home must be writable by runner" >&2; exit 1; }
sudo install -m 0600 -o runner -g staff "${public_key}" "${runner_home}/.ssh/authorized_keys"
sudo install -m 0700 -o runner -g staff "${runner_binary}" "${runner_home}/bin/forgejo-runner"
sudo rm -f "${public_key}" "${runner_binary}"

# Some macOS images restrict Remote Login through this group; add runner only
# when the restriction already exists rather than changing the base policy.
if dscl . -read /Groups/com.apple.access_ssh >/dev/null 2>&1; then
  sudo dseditgroup -o edit -a runner -t user com.apple.access_ssh
fi

if id -Gn runner | tr ' ' '\n' | grep -Fxq admin; then
  echo "runner must not be an administrator" >&2
  exit 1
fi

# The recycle gate: the daemon runs it by name with a bounded timeout and reads
# one JSON verdict, exactly as the libvirt template's sibling is run.
staged_recycle="${FLANFORGE_STAGED_RECYCLE:-/tmp/flanforge-guest-recycle}"
[ -f "${staged_recycle}" ] && [ ! -L "${staged_recycle}" ] \
  || { echo "the guest recycle helper was not staged" >&2; exit 1; }
bash -n "${staged_recycle}"
sudo install -D -m 0755 -o root -g wheel "${staged_recycle}" \
  /usr/local/libexec/flanforge-guest-recycle
sudo rm -f "${staged_recycle}"
