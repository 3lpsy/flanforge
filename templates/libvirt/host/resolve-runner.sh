#!/usr/bin/env bash
# Resolve, download, and authenticate the current upstream Linux runner.

is_curl_transfer_bound_supported() {
  local curl_output
  local curl_version
  local major
  local minor
  curl_output="$(curl --version 2>/dev/null)" || return 1
  curl_version="$(printf '%s\n' "${curl_output}" | awk 'NR == 1 { print $2 }')"
  [[ "${curl_version}" =~ ^([0-9]+)\.([0-9]+)\.[0-9]+ ]] || return 1
  major="${BASH_REMATCH[1]}"
  minor="${BASH_REMATCH[2]}"
  [ "${major}" -gt 8 ] || { [ "${major}" -eq 8 ] && [ "${minor}" -ge 4 ]; }
}

fetch_https() {
  local url="$1"
  local destination="$2"
  local maximum_seconds="$3"
  local maximum_bytes="$4"
  [[ "${maximum_seconds}" =~ ^[1-9][0-9]*$ ]] || return 2
  [[ "${maximum_bytes}" =~ ^[1-9][0-9]*$ ]] || return 2
  curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --fail --location --silent --show-error \
    --connect-timeout 10 --max-time "${maximum_seconds}" \
    --max-filesize "${maximum_bytes}" \
    --output "${destination}" "${url}"
}

ensure_valid_signature_status() {
  local status_file="$1"
  local expected_primary_fingerprint="$2"
  local primary_fingerprint
  local valid_signature_count

  [[ "${expected_primary_fingerprint}" =~ ^[A-F0-9]{40}$ ]] || return 1
  # KEYEXPIRED is deliberately absent: it fires for any expired subkey on the
  # key, even when the signature was made by a live one. A signature actually
  # made by an expired key is EXPKEYSIG, which stays fatal.
  if awk '
    $1 == "[GNUPG:]" &&
      ($2 == "BADSIG" || $2 == "ERRSIG" || $2 == "EXPSIG" ||
       $2 == "EXPKEYSIG" || $2 == "REVKEYSIG" ||
       $2 == "KEYREVOKED" || $2 == "SIGEXPIRED" || $2 == "NO_PUBKEY" ||
       $2 == "NODATA" || $2 == "FAILURE" || $2 == "ERROR" ||
       $2 == "UNEXPECTED") { rejected = 1 }
    END { exit rejected ? 0 : 1 }
  ' "${status_file}"; then
    return 1
  fi
  valid_signature_count="$(awk \
    '$1 == "[GNUPG:]" && $2 == "VALIDSIG" { count++ } END { print count + 0 }' \
    "${status_file}")" || return 1
  [ "${valid_signature_count}" -eq 1 ] || return 1
  primary_fingerprint="$(awk \
    '$1 == "[GNUPG:]" && $2 == "VALIDSIG" { print $NF }' \
    "${status_file}")" || return 1
  [ "${primary_fingerprint}" = "${expected_primary_fingerprint}" ]
}

resolve_forgejo_runner() {
  local staging_dir="$1"
  local api_url="$2"
  local download_base_url="$3"
  local release_key_fingerprint="$4"
  local release_file="${staging_dir}/runner-release.json"
  local release_key="${staging_dir}/forgejo-release-key.asc"
  local signature_status="${staging_dir}/runner-signature.status"
  local signature
  local tag
  local key_fingerprints
  local forgejo_runner_binary
  local forgejo_runner_sha256
  local forgejo_runner_version
  local runner_url
  local runner_size

  local release_api_max_bytes=1048576
  local runner_max_bytes=268435456
  local signature_max_bytes=1048576
  local release_key_max_bytes=1048576

  fetch_https "${api_url}" "${release_file}" 60 "${release_api_max_bytes}"
  [ "$(file_size "${release_file}")" -le 1048576 ] \
    || die "Forgejo Runner release response exceeds 1 MiB"
  tag="$(jq -er '(.tag_name // .name) | select(type == "string")' "${release_file}")" \
    || die "Forgejo Runner release response has no tag"
  [[ "${tag}" =~ ^v?([0-9]+\.[0-9]+\.[0-9]+)$ ]] \
    || die "upstream Forgejo Runner tag '${tag}' is not semantic"

  forgejo_runner_version="${BASH_REMATCH[1]}"
  forgejo_runner_binary="${staging_dir}/forgejo-runner-${forgejo_runner_version}-linux-amd64"
  signature="${forgejo_runner_binary}.asc"
  runner_url="${download_base_url}/v${forgejo_runner_version}/forgejo-runner-${forgejo_runner_version}-linux-amd64"

  fetch_https "${runner_url}" "${forgejo_runner_binary}" 300 "${runner_max_bytes}"
  fetch_https "${runner_url}.asc" "${signature}" 60 "${signature_max_bytes}"
  fetch_https \
    "https://keys.openpgp.org/vks/v1/by-fingerprint/${release_key_fingerprint}" \
    "${release_key}" 60 "${release_key_max_bytes}"

  runner_size="$(file_size "${forgejo_runner_binary}")"
  [ "${runner_size}" -ge 1048576 ] && [ "${runner_size}" -le 268435456 ] \
    || die "Forgejo Runner binary size is outside the 1-256 MiB bound"
  [ "$(file_size "${signature}")" -ge 1 ] \
    && [ "$(file_size "${signature}")" -le "${signature_max_bytes}" ] \
    || die "Forgejo Runner signature is empty or exceeds 1 MiB"
  [ "$(file_size "${release_key}")" -ge 1 ] \
    && [ "$(file_size "${release_key}")" -le "${release_key_max_bytes}" ] \
    || die "Forgejo release key is empty or exceeds 1 MiB"
  LC_ALL=C file "${forgejo_runner_binary}" | grep -Eq 'ELF 64-bit.*x86-64' \
    || die "upstream Forgejo Runner is not a Linux x86-64 ELF"

  install -d -m 0700 "${staging_dir}/gnupg"
  gpg --batch --quiet --homedir "${staging_dir}/gnupg" \
    --import "${release_key}" >/dev/null 2>&1
  key_fingerprints="$(gpg --batch --homedir "${staging_dir}/gnupg" \
    --with-colons --fingerprint "${release_key_fingerprint}" 2>/dev/null \
    | awk -F: '$1 == "fpr" { print $10 }')"
  printf '%s\n' "${key_fingerprints}" | grep -Fxq "${release_key_fingerprint}" \
    || die "downloaded Forgejo release key has the wrong fingerprint"
  gpg --batch --homedir "${staging_dir}/gnupg" --status-fd=1 \
    --verify "${signature}" "${forgejo_runner_binary}" \
    > "${signature_status}" 2>/dev/null \
    || die "Forgejo Runner release signature is invalid"
  ensure_valid_signature_status "${signature_status}" "${release_key_fingerprint}" \
    || die "Forgejo Runner signature was not made by the pinned primary key"

  forgejo_runner_sha256="$(sha256_file "${forgejo_runner_binary}")"
  printf '%s\t%s\t%s\n' "${forgejo_runner_binary}" \
    "${forgejo_runner_version}" "${forgejo_runner_sha256}"
}
