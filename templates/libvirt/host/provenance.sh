#!/usr/bin/env bash
# Stage dependency trust and record generic build inputs.
# shellcheck disable=SC2154 # Sourced dependency derivation owns route outputs.

read_qemu_plugin_version() {
  local packer_template="$1"
  local version_line
  local version_line_count
  local qemu_plugin_version

  [ -f "${packer_template}" ] && [ ! -L "${packer_template}" ] \
    || die "Packer template must be a regular file, not a symlink"
  version_line="$(awk '
    /^[[:space:]]*qemu[[:space:]]*=[[:space:]]*\{[[:space:]]*$/ {
      in_qemu = 1
      next
    }
    in_qemu && /^[[:space:]]*\}[[:space:]]*$/ {
      in_qemu = 0
      next
    }
    in_qemu && /^[[:space:]]*version[[:space:]]*=/ { print }
  ' "${packer_template}")"
  version_line_count="$(printf '%s\n' "${version_line}" \
    | awk 'NF { count += 1 } END { print count + 0 }')"
  [ "${version_line_count}" -eq 1 ] \
    || die "QEMU plugin must have exactly one version constraint"
  qemu_plugin_version="$(printf '%s\n' "${version_line}" | sed -E -n \
    's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"= ([0-9]+\.[0-9]+\.[0-9]+)"[[:space:]]*$/\1/p')"
  [[ "${qemu_plugin_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || die "QEMU plugin version must be one exact semantic version"
  printf '%s\n' "${qemu_plugin_version}"
}

prepare_dependency_ca() {
  local input_file="$1"
  local staging_file="$2"

  if [ -z "${input_file}" ]; then
    : > "${staging_file}"
    chmod 0600 "${staging_file}"
    return
  fi

  ensure_absolute_path FLANFORGE_DEPENDENCY_CA_FILE "${input_file}"
  [ -f "${input_file}" ] && [ ! -L "${input_file}" ] \
    || die "dependency CA file must be a regular file, not a symlink"
  [ "$(file_size "${input_file}")" -le 1048576 ] \
    || die "dependency CA file exceeds 1 MiB"
  require_command openssl
  openssl x509 -in "${input_file}" -noout >/dev/null \
    || die "dependency CA file is not an X.509 certificate"
  cp "${input_file}" "${staging_file}"
  chmod 0600 "${staging_file}"
  sha256_file "${staging_file}"
}

write_build_provenance() {
  local output_file="$1"
  local runner_api_url="$2"
  local runner_download_base_url="$3"
  local runner_signing_fingerprint="$4"
  local dependency_ca_sha256="$5"
  local dependency_proxy_configured=false

  if [ -n "${dependency_proxy_url}" ] \
    || [ -n "${dependency_container_registry_url}" ]; then
    dependency_proxy_configured=true
  fi

  jq -n \
    --arg runner_api_url "${runner_api_url}" \
    --arg runner_download_base_url "${runner_download_base_url}" \
    --arg runner_signing_primary_fingerprint "${runner_signing_fingerprint}" \
    --argjson dependency_proxy_configured "${dependency_proxy_configured}" \
    --arg dependency_ca_sha256 "${dependency_ca_sha256}" \
    '{
      runner_release: {
        api_url: $runner_api_url,
        download_base_url: $runner_download_base_url,
        signing_primary_fingerprint: $runner_signing_primary_fingerprint
      },
      dependency_proxy: {
        configured: $dependency_proxy_configured,
        ca_sha256: ($dependency_ca_sha256 |
          if length > 0 then . else null end)
      }
    }' > "${output_file}"
  chmod 0600 "${output_file}"
}
