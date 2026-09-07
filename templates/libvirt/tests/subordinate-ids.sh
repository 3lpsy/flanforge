#!/usr/bin/env bash
# Exercise subordinate-ID selection and validation with temporary files.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../host/common.sh
source "${template_dir}/host/common.sh"
# shellcheck source=../scripts/subordinate-ids.sh
source "${template_dir}/scripts/subordinate-ids.sh"

test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-subid-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-subid-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

subuid="${test_root}/subuid"
subgid="${test_root}/subgid"
export FLANFORGE_SUBORDINATE_ID_LOCK_FILE="${test_root}/allocation.lock"

printf '%s\n' 'packer:100000:65536' > "${subuid}"
printf '%s\n' 'builder:165536:65536' > "${subgid}"
selection="$(select_subordinate_id_range runner "${subuid}" "${subgid}")"
[ "${selection}" = new:231072:65536 ] \
  || die "allocation did not avoid ranges from both subordinate-ID files"
ensured="$(ensure_subordinate_id_range runner "${subuid}" "${subgid}")"
[ "${ensured}" = existing:231072:65536 ] \
  || die "new subordinate-ID range was not verified"
[ "$(grep -c '^runner:231072:65536$' "${subuid}")" -eq 1 ] \
  || die "subuid does not contain exactly one allocated runner range"
[ "$(grep -c '^runner:231072:65536$' "${subgid}")" -eq 1 ] \
  || die "subgid does not contain exactly one allocated runner range"

printf '%s\n' 'runner:300000:65536' 'worker:400000:65536' > "${subuid}"
printf '%s\n' 'runner:300000:65536' 'worker:500000:65536' > "${subgid}"
[ "$(select_subordinate_id_range runner "${subuid}" "${subgid}")" \
  = existing:300000:65536 ] || die "valid existing range was rejected"

printf '%s\n' 'runner:200000:65536' 'worker:250000:1000' > "${subuid}"
printf '%s\n' 'runner:200000:65536' > "${subgid}"
if select_subordinate_id_range runner "${subuid}" "${subgid}" >/dev/null 2>&1; then
  die "overlapping runner range was accepted"
fi

printf '%s\n' 'runner:200000:65536' > "${subuid}"
printf '%s\n' 'runner:300000:65536' > "${subgid}"
if select_subordinate_id_range runner "${subuid}" "${subgid}" >/dev/null 2>&1; then
  die "mismatched UID and GID ranges were accepted"
fi

printf '%s\n' 'runner:200000:65536' 'runner:300000:65536' > "${subuid}"
printf '%s\n' 'runner:200000:65536' > "${subgid}"
if select_subordinate_id_range runner "${subuid}" "${subgid}" >/dev/null 2>&1; then
  die "duplicate runner ranges were accepted"
fi

printf '%s\n' 'runner:0:65536' > "${subuid}"
printf '%s\n' 'runner:0:65536' > "${subgid}"
if select_subordinate_id_range runner "${subuid}" "${subgid}" >/dev/null 2>&1; then
  die "runner range mapping host root IDs was accepted"
fi

printf '%s\n' 'not:a:valid:entry' > "${subuid}"
: > "${subgid}"
if select_subordinate_id_range runner "${subuid}" "${subgid}" >/dev/null 2>&1; then
  die "malformed subordinate-ID input was accepted"
fi

: > "${subuid}"
: > "${subgid}"
append_subordinate_id_line() {
  local file="$1"
  local entry="$2"
  [ "${file}" != "${subgid}" ] || return 1
  printf '%s\n' "${entry}" >> "${file}"
}
if ensure_subordinate_id_range runner "${subuid}" "${subgid}" >/dev/null 2>&1; then
  die "simulated subgid write failure unexpectedly succeeded"
fi
[ ! -s "${subuid}" ] && [ ! -s "${subgid}" ] \
  || die "failed paired update left a one-sided subordinate-ID allocation"

grep -Fq 'remove_subordinate_id_helper' \
  "${template_dir}/scripts/provision-runner.sh" \
  || die "runner provisioning does not remove its uploaded allocation helper"
grep -Fq '[ ! -e /tmp/flanforge-subordinate-ids.sh ]' \
  "${template_dir}/scripts/finalize.sh" \
  || die "image finalization does not reject a retained allocation helper"

echo "subordinate-ID fake tests passed"
