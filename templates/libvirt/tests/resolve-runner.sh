#!/usr/bin/env bash
# Verify bounded downloads and machine-readable signer pinning with fakes.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../host/common.sh
source "${template_dir}/host/common.sh"
# shellcheck source=../host/resolve-runner.sh
source "${template_dir}/host/resolve-runner.sh"

test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-resolver-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-resolver-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

fake_bin="${test_root}/bin"
curl_log="${test_root}/curl.log"
mkdir -p "${fake_bin}"
export FAKE_CURL_LOG="${curl_log}"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'if [ "${1:-}" = --version ]; then' \
  '  printf "curl %s (fixture)\n" "${FAKE_CURL_VERSION:-8.18.0}"' \
  '  exit 0' \
  'fi' \
  'printf "%s\n" "$*" >> "${FAKE_CURL_LOG}"' \
  'destination=""' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in' \
  '    --output) destination="$2"; shift 2 ;;' \
  '    *) shift ;;' \
  '  esac' \
  'done' \
  '[ -n "${destination}" ]' \
  'printf fixture > "${destination}"' > "${fake_bin}/curl"
chmod 0700 "${fake_bin}/curl"
PATH="${fake_bin}:${PATH}"
export PATH

FAKE_CURL_VERSION=8.4.0
export FAKE_CURL_VERSION
is_curl_transfer_bound_supported || die "curl 8.4 was rejected"
FAKE_CURL_VERSION=8.3.0
if is_curl_transfer_bound_supported; then
  die "curl 8.3 was accepted for unknown-length transfer bounds"
fi
FAKE_CURL_VERSION=8.18.0

download="${test_root}/download"
fetch_https https://example.invalid/artifact "${download}" 17 1234
[ "$(< "${download}")" = fixture ] || die "fake bounded download did not run"
grep -Fq -- '--max-time 17 --max-filesize 1234' "${curl_log}" \
  || die "download did not pass both time and transfer-size limits to curl"

expected_fingerprint=0123456789ABCDEF0123456789ABCDEF01234567
other_fingerprint=89ABCDEF0123456789ABCDEF0123456789ABCDEF
status_file="${test_root}/signature.status"
printf '%s\n' \
  "[GNUPG:] VALIDSIG AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA 0 0 0 0 0 0 0 0 0 ${expected_fingerprint}" \
  > "${status_file}"
ensure_valid_signature_status "${status_file}" "${expected_fingerprint}" \
  || die "pinned primary signer was rejected"

printf '%s\n' \
  "[GNUPG:] VALIDSIG AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA 0 0 0 0 0 0 0 0 0 ${other_fingerprint}" \
  > "${status_file}"
if ensure_valid_signature_status "${status_file}" "${expected_fingerprint}"; then
  die "wrong primary signer was accepted"
fi

printf '%s\n' \
  "[GNUPG:] VALIDSIG AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA 0 0 0 0 0 0 0 0 0 ${expected_fingerprint}" \
  "[GNUPG:] VALIDSIG BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB 0 0 0 0 0 0 0 0 0 ${expected_fingerprint}" \
  > "${status_file}"
if ensure_valid_signature_status "${status_file}" "${expected_fingerprint}"; then
  die "multiple valid signatures were accepted"
fi

for rejected_status in EXPSIG EXPKEYSIG REVKEYSIG; do
  printf '%s\n' \
    "[GNUPG:] VALIDSIG AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA 0 0 0 0 0 0 0 0 0 ${expected_fingerprint}" \
    "[GNUPG:] ${rejected_status} fixture" > "${status_file}"
  if ensure_valid_signature_status "${status_file}" "${expected_fingerprint}"; then
    die "${rejected_status} signature status was accepted"
  fi
done

# An expired sibling subkey emits KEYEXPIRED beside a signature made by a live
# key; that is key hygiene on the signer's side, not a verification failure.
printf '%s\n' \
  "[GNUPG:] KEYEXPIRED 1700000000" \
  "[GNUPG:] VALIDSIG AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA 0 0 0 0 0 0 0 0 0 ${expected_fingerprint}" \
  "[GNUPG:] KEYEXPIRED 1700000000" \
  > "${status_file}"
ensure_valid_signature_status "${status_file}" "${expected_fingerprint}" \
  || die "an expired sibling subkey blocked a valid pinned signature"

echo "runner resolver fake tests passed"
