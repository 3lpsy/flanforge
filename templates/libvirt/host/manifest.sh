#!/usr/bin/env bash
# Build the bounded host manifest from authenticated guest and qcow2 facts.

write_image_manifest() {
  local image_path="$1"
  local image_info="$2"
  local output_directory="$3"
  local image_name="$4"
  local source_image_url="$5"
  local source_image_sha256="$6"
  local guest_manifest_file="$7"
  local project_root="$8"
  local build_provenance_file="$9"
  local qemu_plugin_version="${10}"
  local created_at
  local image_sha256
  local image_bytes
  local manifest_path
  local manifest_tmp
  local packer_version
  local source_dirty=false
  local source_revision
  local virtual_bytes

  [[ "${qemu_plugin_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || die "QEMU plugin provenance must be a semantic version"

  image_sha256="$(sha256_file "${image_path}")"
  image_bytes="$(file_size "${image_path}")"
  virtual_bytes="$(printf '%s' "${image_info}" | jq -er '."virtual-size"')"
  created_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
  source_revision="$(git -C "${project_root}" rev-parse --verify HEAD 2>/dev/null || printf unknown)"
  [ -z "$(git -C "${project_root}" status --porcelain 2>/dev/null)" ] || source_dirty=true
  packer_version="$(packer version | awk 'NR == 1 { print $2 }')"
  manifest_path="${output_directory}/${image_name}.manifest.json"
  manifest_tmp="$(mktemp "${output_directory}/.manifest.XXXXXX")"

  jq -n \
    --arg created_at "${created_at}" \
    --arg image_file "${image_name}.qcow2" \
    --arg image_sha256 "${image_sha256}" \
    --argjson image_bytes "${image_bytes}" \
    --argjson virtual_bytes "${virtual_bytes}" \
    --arg source_url "${source_image_url}" \
    --arg source_sha256 "${source_image_sha256}" \
    --arg source_revision "${source_revision}" \
    --argjson source_dirty "${source_dirty}" \
    --arg packer_version "${packer_version}" \
    --arg qemu_plugin_version "${qemu_plugin_version}" \
    --slurpfile guest "${guest_manifest_file}" \
    --slurpfile provenance "${build_provenance_file}" \
    '{
      schema_version: 1,
      created_at: $created_at,
      source: {
        os_image_url: $source_url,
        os_image_sha256: $source_sha256,
        repository_revision: $source_revision,
        repository_dirty: $source_dirty
      },
      builder: {
        packer_version: $packer_version,
        qemu_plugin_version: $qemu_plugin_version
      },
      image: {
        file: $image_file,
        format: "qcow2",
        sha256: $image_sha256,
        bytes: $image_bytes,
        virtual_bytes: $virtual_bytes
      },
      provenance: $provenance[0],
      guest: $guest[0]
    }' > "${manifest_tmp}"
  chmod 0644 "${manifest_tmp}"
  mv "${manifest_tmp}" "${manifest_path}"
  printf '%s\t%s\n' "${manifest_path}" "${image_sha256}"
}
