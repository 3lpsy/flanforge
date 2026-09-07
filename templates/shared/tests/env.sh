#!/usr/bin/env bash
# Exercise template-local settings and the one-release Tart fallback.
set -Eeuo pipefail

shared_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project_root="$(cd "${shared_dir}/../.." && pwd)"
# shellcheck source=../env.sh
source "${shared_dir}/env.sh"

die() {
  echo "error: $*" >&2
  exit 1
}

test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-env-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-env-tests."* ]]; then
    chmod -R u+w "${test_root}" 2>/dev/null || true
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

tart_dir="${test_root}/templates/tart"
libvirt_dir="${test_root}/templates/libvirt"
mkdir -p "${tart_dir}" "${libvirt_dir}"
printf '%s\n' 'TEMPLATE_SCOPE=legacy' 'LEGACY_ONLY=loaded' > "${test_root}/.env"
printf '%s\n' 'TEMPLATE_SCOPE=tart' > "${tart_dir}/.env"
printf '%s\n' 'TEMPLATE_SCOPE=libvirt' > "${libvirt_dir}/.env"
chmod 0400 "${test_root}/.env" "${tart_dir}/.env" "${libvirt_dir}/.env"

legacy_before="$(cksum "${test_root}/.env")"
tart_before="$(cksum "${tart_dir}/.env")"
warning_file="${test_root}/warning"
tart_result="$(
  {
    unset TEMPLATE_SCOPE LEGACY_ONLY
    load_tart_template_environment "${tart_dir}" "${test_root}"
    printf '%s:%s:%s\n' "${TEMPLATE_SCOPE}" "${LEGACY_ONLY:-unset}" \
      "$(env | awk -F= '$1 == "TEMPLATE_SCOPE" { print $2 }')"
  } 2> "${warning_file}"
)"
[ "$(printf '%s\n' "${tart_result}" | tail -n 1)" = tart:unset:tart ] \
  || die "Tart-local settings did not take precedence"
[ ! -s "${warning_file}" ] || die "Tart-local settings emitted a fallback warning"
[ "$(cksum "${tart_dir}/.env")" = "${tart_before}" ] \
  || die "Tart loading modified its template environment file"

libvirt_result="$(
  unset TEMPLATE_SCOPE LEGACY_ONLY
  load_template_environment "${libvirt_dir}"
  printf '%s:%s\n' "${TEMPLATE_SCOPE}" "${LEGACY_ONLY:-unset}"
)"
[ "$(printf '%s\n' "${libvirt_result}" | tail -n 1)" = libvirt:unset ] \
  || die "libvirt settings were not isolated"

find "${tart_dir}" -name .env -delete
fallback_result="$(
  {
    unset TEMPLATE_SCOPE LEGACY_ONLY
    load_tart_template_environment "${tart_dir}" "${test_root}"
    printf '%s:%s\n' "${TEMPLATE_SCOPE}" "${LEGACY_ONLY:-unset}"
  } 2> "${warning_file}"
)"
[ "$(printf '%s\n' "${fallback_result}" | tail -n 1)" = legacy:loaded ] \
  || die "Tart root fallback did not load legacy settings"
grep -Fq 'root .env fallback is deprecated' "${warning_file}" \
  || die "Tart root fallback did not emit its deprecation warning"

empty_libvirt_dir="${test_root}/templates/empty-libvirt"
mkdir -p "${empty_libvirt_dir}"
isolated_result="$(
  unset TEMPLATE_SCOPE LEGACY_ONLY
  load_template_environment "${empty_libvirt_dir}"
  printf '%s:%s\n' "${TEMPLATE_SCOPE:-unset}" "${LEGACY_ONLY:-unset}"
)"
[ "${isolated_result}" = unset:unset ] \
  || die "libvirt loaded settings from outside its template directory"

(
  set +a
  load_template_environment "${libvirt_dir}" >/dev/null
  case "$-" in
    *a*) die "environment loading enabled allexport in its caller" ;;
  esac
)
(
  set -a
  load_template_environment "${libvirt_dir}" >/dev/null
  case "$-" in
    *a*) ;;
    *) die "environment loading disabled caller allexport" ;;
  esac
)

invalid_dir="${test_root}/templates/invalid"
mkdir -p "${invalid_dir}/.env"
if (load_template_environment "${invalid_dir}") >/dev/null 2>&1; then
  die "a non-file template environment was accepted"
fi

[ "$(cksum "${test_root}/.env")" = "${legacy_before}" ] \
  || die "Tart fallback modified the root environment file"
if grep -Eq '^[[:space:]]*set[[:space:]]+dotenv-load' "${project_root}/justfile"; then
  die "Just still loads a repository-wide environment file"
fi

echo "template environment loading tests passed"
