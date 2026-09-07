#!/usr/bin/env bash
# Unblock the two guest-agent RPCs the SSH-less guest channel needs, and bake
# the readiness helper. Fails closed on every assumption it makes.
set -Eeuo pipefail

trap 'echo "guest-agent provisioning failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

sysconfig="${FLANFORGE_QEMU_GA_SYSCONFIG:-/etc/sysconfig/qemu-ga}"
share_dir="${FLANFORGE_SHARE_DIR:-/usr/local/share/flanforge}"
libexec_dir="${FLANFORGE_LIBEXEC_DIR:-/usr/local/libexec}"
staged_helper="${FLANFORGE_STAGED_HELPER:-/tmp/flanforge-guest-ready}"
staged_recycle="${FLANFORGE_STAGED_RECYCLE:-/tmp/flanforge-guest-recycle}"
unblocked_rpcs=(guest-exec guest-exec-status)
job_account_name=runner
job_account_uid=2000
runner_home=/home/runner

die() { echo "$1" >&2; exit 1; }

[ -f "${sysconfig}" ] && [ ! -L "${sysconfig}" ] \
  || die "qemu-ga sysconfig is missing or not a regular file"
[ "$(stat -c '%U:%G' "${sysconfig}")" = root:root ] \
  || die "qemu-ga sysconfig is not owned by root"
[ "$(stat -c '%s' "${sysconfig}")" -le 16384 ] \
  || die "qemu-ga sysconfig exceeds 16 KiB"

# QEMU >= 9.1 packaging consumes $QEMU_GA_ARGS with an explicit --block-rpcs
# flag and ships no active assignment; older packagings consume a bare
# $BLOCK_RPCS or $BLACKLIST_RPC list. The unit decides which one is live.
unit_text="$(systemctl cat qemu-guest-agent.service 2>/dev/null || true)"
variable=""
for candidate in QEMU_GA_ARGS BLOCK_RPCS BLACKLIST_RPC; do
  if printf '%s\n' "${unit_text}" \
    | grep -Eq "[\$]\{${candidate}\}|[\$]${candidate}([^A-Za-z0-9_]|\$)"; then
    [ -z "${variable}" ] \
      || die "qemu-guest-agent.service consumes both ${variable} and ${candidate}"
    variable="${candidate}"
  fi
done
[ -n "${variable}" ] \
  || die "qemu-guest-agent.service consumes no known block-list variable"

for candidate in QEMU_GA_ARGS BLOCK_RPCS BLACKLIST_RPC; do
  [ "${candidate}" = "${variable}" ] && continue
  [ "$(grep -Ec "^[[:space:]]*${candidate}=" "${sysconfig}" || true)" -eq 0 ] \
    || die "qemu-ga sysconfig assigns ${candidate}, which the unit ignores"
done
assignments="$(grep -Ec "^[[:space:]]*${variable}=" "${sysconfig}" || true)"
[ "${assignments}" -le 1 ] \
  || die "qemu-ga sysconfig assigns ${variable} more than once"
if [ "${variable}" != QEMU_GA_ARGS ] && [ "${assignments}" -eq 0 ]; then
  die "qemu-ga block-list variable is absent"
fi

# A non-empty allow-list would override the block-list and exclude guest-exec.
allow_line="$(grep -E '^[[:space:]]*ALLOW_RPCS=' "${sysconfig}" || true)"
allow_value="${allow_line#*=}"
allow_value="${allow_value%\"}"
allow_value="${allow_value#\"}"
[ -z "${allow_value}" ] \
  || die "qemu-ga sysconfig sets a non-empty ALLOW_RPCS allow-list"

current=""
if [ "${assignments}" -eq 1 ]; then
  current_line="$(grep -E "^[[:space:]]*${variable}=" "${sysconfig}")"
  current="${current_line#*=}"
  current="${current%\"}"
  current="${current#\"}"
  current="${current%\'}"
  current="${current#\'}"
  if [ "${variable}" = QEMU_GA_ARGS ]; then
    [[ "${current}" =~ ^--block-rpcs=[a-z0-9,-]*$ ]] \
      || die "qemu-ga ${variable} carries arguments beyond --block-rpcs"
    current="${current#--block-rpcs=}"
  fi
  [[ "${current}" =~ ^[a-z0-9,-]*$ ]] \
    || die "qemu-ga ${variable} holds an unexpected value"
fi

# Keep whatever else the distribution blocks, drop only the exec pair, and
# impose the file-RPC blocks: current packaging ships with nothing blocked.
kept=()
entries=()
IFS=',' read -r -a entries <<< "${current}"
for entry in ${entries[@]+"${entries[@]}"}; do
  [ -n "${entry}" ] || continue
  keep=true
  for unblocked in "${unblocked_rpcs[@]}"; do
    [ "${entry}" != "${unblocked}" ] || keep=false
  done
  [ "${keep}" = false ] || kept+=("${entry}")
done
for required in guest-file-open guest-file-close guest-file-read \
  guest-file-write guest-file-seek guest-file-flush; do
  present=false
  for entry in ${kept[@]+"${kept[@]}"}; do
    [ "${entry}" != "${required}" ] || present=true
  done
  [ "${present}" = true ] || kept+=("${required}")
done
effective="$(IFS=','; printf '%s' "${kept[*]}")"
assignment_value="${effective}"
[ "${variable}" != QEMU_GA_ARGS ] || assignment_value="--block-rpcs=${effective}"

mode="$(stat -c '%a' "${sysconfig}")"
rewritten="$(mktemp "${TMPDIR:-/tmp}/flanforge-qemu-ga.XXXXXX")"
if [ "${assignments}" -eq 1 ]; then
  awk -v name="${variable}" -v value="${assignment_value}" '
    $0 ~ "^[[:space:]]*" name "=" { printf "%s=%s\n", name, value; next }
    { print }
  ' "${sysconfig}" > "${rewritten}"
else
  cat "${sysconfig}" > "${rewritten}"
  printf '%s=%s\n' "${variable}" "${assignment_value}" >> "${rewritten}"
fi
sudo install -m "${mode}" -o root -g root "${rewritten}" "${sysconfig}"
rm -f "${rewritten}"

rewritten_line="$(grep -E "^[[:space:]]*${variable}=" "${sysconfig}")"
for unblocked in "${unblocked_rpcs[@]}"; do
  if printf '%s' "${rewritten_line}" | grep -Eq "(^|[=,])${unblocked}(,|$)"; then
    die "qemu-ga still blocks ${unblocked}"
  fi
done

# The next boot is the only one that matters: Packer's builder attaches no
# guest-agent port, so the service is deliberately not restarted here.
sudo install -d -m 0755 -o root -g root "${share_dir}"
printf '%s\n' "${effective}" \
  | sudo tee "${share_dir}/qemu-ga-block-rpcs" >/dev/null
sudo chown root:root "${share_dir}/qemu-ga-block-rpcs"
sudo chmod 0644 "${share_dir}/qemu-ga-block-rpcs"
printf '%s:%s\n' "${job_account_name}" "${job_account_uid}" \
  | sudo tee "${share_dir}/job-account" >/dev/null
sudo chown root:root "${share_dir}/job-account"
sudo chmod 0644 "${share_dir}/job-account"

[ -f "${staged_helper}" ] && [ ! -L "${staged_helper}" ] \
  || die "the guest readiness helper was not staged"
bash -n "${staged_helper}"
sudo install -D -m 0755 -o root -g root "${staged_helper}" \
  "${libexec_dir}/flanforge-guest-ready"
rm -f "${staged_helper}"

# The recycle gate is the readiness helper's sibling: same shape, same one JSON
# line, same typed exit codes, and the daemon runs it the same way.
[ -f "${staged_recycle}" ] && [ ! -L "${staged_recycle}" ] \
  || die "the guest recycle helper was not staged"
bash -n "${staged_recycle}"
sudo install -D -m 0755 -o root -g root "${staged_recycle}" \
  "${libexec_dir}/flanforge-guest-recycle"
rm -f "${staged_recycle}"

# `runuser -l` runs a login shell, which reads .bash_profile rather than
# .bashrc, so the agent channel would otherwise lose the dependency routing.
# The 0700 home needs sudo even for the existence probe.
if [ -d "${runner_home}" ] && ! sudo test -e "${runner_home}/.bash_profile"; then
  printf '%s\n' \
    '[ -f "$HOME/.bashrc" ] && . "$HOME/.bashrc"' \
    | sudo tee "${runner_home}/.bash_profile" >/dev/null
  sudo chown "${job_account_name}:${job_account_name}" \
    "${runner_home}/.bash_profile"
  sudo chmod 0600 "${runner_home}/.bash_profile"
fi

echo "==> guest-agent exec channel enabled"
