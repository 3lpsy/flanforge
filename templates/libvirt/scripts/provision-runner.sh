#!/usr/bin/env bash
# Create the unprivileged job account and install the verified upstream runner.
set -Eeuo pipefail

trap 'echo "runner provisioning failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

runner_binary=/tmp/flanforge-forgejo-runner
runner_home=/home/runner
runner_uid=2000
subordinate_id_helper=/tmp/flanforge-subordinate-ids.sh
subordinate_id_range_file=/usr/local/share/flanforge/runner-subordinate-id-range

remove_subordinate_id_helper() {
  rm -f "${subordinate_id_helper}"
}
trap remove_subordinate_id_helper EXIT

[[ "${FORGEJO_RUNNER_VERSION:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || { echo "invalid Forgejo Runner version" >&2; exit 1; }
[[ "${FORGEJO_RUNNER_SHA256:-}" =~ ^[a-f0-9]{64}$ ]] \
  || { echo "invalid Forgejo Runner digest" >&2; exit 1; }
[ -s "${runner_binary}" ] || { echo "missing Forgejo Runner binary" >&2; exit 1; }
[ -f "${subordinate_id_helper}" ] && [ ! -L "${subordinate_id_helper}" ] \
  || { echo "missing subordinate-ID helper" >&2; exit 1; }
printf '%s  %s\n' "${FORGEJO_RUNNER_SHA256}" "${runner_binary}" | sha256sum -c -
LC_ALL=C file "${runner_binary}" | grep -Eq 'ELF 64-bit.*x86-64' \
  || { echo "Forgejo Runner must be a Linux x86-64 ELF" >&2; exit 1; }

if id runner >/dev/null 2>&1; then
  [ "$(id -u runner)" -eq "${runner_uid}" ] \
    || { echo "existing runner account has the wrong UID" >&2; exit 1; }
else
  sudo useradd --create-home --home-dir "${runner_home}" \
    --shell /bin/bash --uid "${runner_uid}" --user-group runner
fi

sudo passwd --lock runner >/dev/null
sudo install -d -m 0700 -o runner -g runner "${runner_home}"
sudo install -D -m 0755 -o root -g root \
  "${runner_binary}" /usr/local/bin/forgejo-runner
sudo install -d -m 0755 -o root -g root /usr/local/share/flanforge
printf '%s\n' "${FORGEJO_RUNNER_VERSION}" \
  | sudo tee /usr/local/share/flanforge/forgejo-runner.version >/dev/null
printf '%s\n' "${FORGEJO_RUNNER_SHA256}" \
  | sudo tee /usr/local/share/flanforge/forgejo-runner.sha256 >/dev/null
sudo chmod 0644 /usr/local/share/flanforge/forgejo-runner.*
sudo rm -f "${runner_binary}"

subordinate_range="$(sudo bash -Eeuo pipefail -c \
  'source /tmp/flanforge-subordinate-ids.sh; ensure_subordinate_id_range runner /etc/subuid /etc/subgid' \
  )"
[[ "${subordinate_range}" =~ ^existing:([0-9]+):65536$ ]] \
  || { echo "invalid subordinate-ID allocation result" >&2; exit 1; }
printf '%s:%s\n' "${BASH_REMATCH[1]}" 65536 \
  | sudo tee "${subordinate_id_range_file}" >/dev/null
sudo chmod 0644 "${subordinate_id_range_file}"
remove_subordinate_id_helper
trap - EXIT

if id -nG runner | tr ' ' '\n' | grep -Eq '^(wheel|root|libvirt|qemu)$'; then
  echo "runner belongs to a privileged group" >&2
  exit 1
fi

echo "==> Forgejo Runner ${FORGEJO_RUNNER_VERSION} installed"
