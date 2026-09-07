#!/usr/bin/env bash
# Build a verified Fedora qcow2 base for the FlanForge libvirt backend.
# shellcheck disable=SC2154 # Sourced dependency derivation owns route outputs.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd "${template_dir}/../.." && pwd)"
# shellcheck source=../shared/env.sh
source "${template_dir}/../shared/env.sh"
# shellcheck source=host/common.sh
source "${template_dir}/host/common.sh"
# shellcheck source=host/dependencies.sh
source "${template_dir}/host/dependencies.sh"
# shellcheck source=host/resolve-runner.sh
source "${template_dir}/host/resolve-runner.sh"
# shellcheck source=host/manifest.sh
source "${template_dir}/host/manifest.sh"
# shellcheck source=host/provenance.sh
source "${template_dir}/host/provenance.sh"
# shellcheck source=../shared/tailscale/arguments.sh
source "${template_dir}/../shared/tailscale/arguments.sh"

usage() {
  printf '%s\n' \
    "Usage:" \
    "  ${0##*/} [--dry-run | --validate-only]" \
    "  ${0##*/} -h | --help | help" \
    "" \
    "Build the Linux/x86-64 qcow2 base. --dry-run resolves and verifies the" \
    "current upstream Forgejo Runner and validates Packer without starting" \
    "QEMU. --validate-only performs syntax validation with temporary" \
    "placeholder inputs; Packer init may still fetch its pinned plugin." \
    "" \
    "Copy .env.example to this directory's .env for all supported settings." >&2
}

mode=build
for argument in "$@"; do
  case "${argument}" in
    -h|--help|help) usage; exit 0 ;;
    --dry-run) [ "${mode}" = build ] || die "select only one mode"; mode=dry-run ;;
    --validate-only) [ "${mode}" = build ] || die "select only one mode"; mode=validate-only ;;
    *) usage; die "unknown argument '${argument}'" ;;
  esac
done

load_template_environment "${template_dir}"

# The privileged automation account defaults on; an optional password swaps
# its sudoers rule from NOPASSWD to password-gated. The password is captured
# and unexported like the tailnet key, and travels only as a private file.
privileged_account_enabled="${FLANFORGE_PRIVILEGED_ACCOUNT:-true}"
privileged_sudo_password="${FLANFORGE_PRIVILEGED_SUDO_PASSWORD:-}"
unset FLANFORGE_PRIVILEGED_SUDO_PASSWORD

# A pre-auth key is the only switch: without one the image stays unjoined.
# Captured and unexported before any child process runs, so the credential
# reaches the guest only through a private file.
tailscale_preauth_key="${FLANFORGE_TAILSCALE_PREAUTH_KEY:-}"
tailscale_login_server="${FLANFORGE_TAILSCALE_LOGIN_SERVER:-}"
tailscale_hostname="${FLANFORGE_TAILSCALE_HOSTNAME:-}"
tailscale_extra_args="${FLANFORGE_TAILSCALE_EXTRA_ARGS:-}"
unset FLANFORGE_TAILSCALE_PREAUTH_KEY
tailscale_enabled=false
[ -z "${tailscale_preauth_key}" ] || tailscale_enabled=true

ensure_linux_x86_64
require_command jq
require_command packer
require_command sha256sum
require_command ssh-keygen

source_image_url="${LIBVIRT_BASE_IMAGE_URL:-https://download.fedoraproject.org/pub/fedora/linux/releases/44/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2}"
source_image_sha256="${LIBVIRT_BASE_IMAGE_SHA256:-28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f}"
image_name="${LIBVIRT_IMAGE_NAME:-flanforge-base}"
output_directory="${LIBVIRT_OUTPUT_DIR:-${template_dir}/output}"
cpu_count="${LIBVIRT_BUILD_CPU:-4}"
memory_mb="${LIBVIRT_BUILD_MEMORY_MB:-4096}"
disk_size_mb="${LIBVIRT_DISK_SIZE_MB:-40960}"
services_root_domain="${SERVICES_ROOT_DOMAIN:-}"
container_registry_mirror="${FLANFORGE_CONTAINER_REGISTRY_MIRROR:-}"
dependency_ca_input="${FLANFORGE_DEPENDENCY_CA_FILE:-}"
runner_api_url="${FORGEJO_RUNNER_API_URL:-https://data.forgejo.org/api/v1/repos/forgejo/runner/releases/latest}"
runner_download_base="${FORGEJO_RUNNER_DOWNLOAD_BASE_URL:-https://code.forgejo.org/forgejo/runner/releases/download}"
runner_key_fingerprint="${FORGEJO_RELEASE_KEY_FINGERPRINT:-EB114F5E6C0DC2BCDD183550A4B61A2DC5923710}"
qemu_plugin_version="$(read_qemu_plugin_version \
  "${template_dir}/flanforge-base.pkr.hcl")"

ensure_https_url LIBVIRT_BASE_IMAGE_URL "${source_image_url}"
ensure_https_url FORGEJO_RUNNER_API_URL "${runner_api_url}"
ensure_https_url FORGEJO_RUNNER_DOWNLOAD_BASE_URL "${runner_download_base}"
[[ "${source_image_sha256}" =~ ^[a-f0-9]{64}$ ]] \
  || die "LIBVIRT_BASE_IMAGE_SHA256 must be 64 lowercase hex characters"
ensure_safe_name LIBVIRT_IMAGE_NAME "${image_name}"
ensure_absolute_path LIBVIRT_OUTPUT_DIR "${output_directory}"
ensure_positive_integer LIBVIRT_BUILD_CPU "${cpu_count}" 64
ensure_positive_integer LIBVIRT_BUILD_MEMORY_MB "${memory_mb}" 131072
ensure_positive_integer LIBVIRT_DISK_SIZE_MB "${disk_size_mb}" 1048576
[ "${memory_mb}" -ge 2048 ] || die "LIBVIRT_BUILD_MEMORY_MB must be at least 2048"
[ "${disk_size_mb}" -ge 16384 ] || die "LIBVIRT_DISK_SIZE_MB must be at least 16384"
[[ "${runner_key_fingerprint}" =~ ^[A-F0-9]{40}$ ]] \
  || die "FORGEJO_RELEASE_KEY_FINGERPRINT must be 40 uppercase hex characters"

[[ "${privileged_account_enabled}" =~ ^(true|false)$ ]] \
  || die "FLANFORGE_PRIVILEGED_ACCOUNT must be true or false"
if [ -n "${privileged_sudo_password}" ]; then
  [ "${privileged_account_enabled}" = true ] \
    || die "FLANFORGE_PRIVILEGED_SUDO_PASSWORD needs the privileged account enabled"
  is_preauth_key_shaped "${privileged_sudo_password}" \
    || die "FLANFORGE_PRIVILEGED_SUDO_PASSWORD must be 8 to 512 printable non-whitespace characters"
fi

if [ "${tailscale_enabled}" = true ]; then
  is_preauth_key_shaped "${tailscale_preauth_key}" \
    || die "FLANFORGE_TAILSCALE_PREAUTH_KEY must be 8 to 512 printable non-whitespace characters"
  [ -n "${tailscale_login_server}" ] \
    || die "FLANFORGE_TAILSCALE_LOGIN_SERVER is required with a pre-auth key"
  ensure_https_url FLANFORGE_TAILSCALE_LOGIN_SERVER "${tailscale_login_server}"
fi
if [ -n "${tailscale_hostname}" ]; then
  is_valid_host_label "${tailscale_hostname}" \
    || die "FLANFORGE_TAILSCALE_HOSTNAME must be a valid host label"
fi
parse_tailscale_extra_arguments "${tailscale_extra_args}" \
  || die "FLANFORGE_TAILSCALE_EXTRA_ARGS contains invalid or reserved options"

derive_dependency_routes "${services_root_domain}" "${container_registry_mirror}"

if [ "${mode}" != validate-only ] && [ -e "${output_directory}" ]; then
  die "output '${output_directory}' already exists; archive or remove it explicitly"
fi

staging_dir="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-libvirt-build.XXXXXX")" \
  || die "cannot create a private staging directory"
chmod 0700 "${staging_dir}"
cleanup() {
  local status=$?
  if [[ "${staging_dir}" == "${TMPDIR:-/tmp}/flanforge-libvirt-build."* ]]; then
    find "${staging_dir}" -depth -delete 2>/dev/null || true
  fi
  return "${status}"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

build_identity="${staging_dir}/build-ssh-key"
ssh-keygen -q -t ed25519 -N '' -C flanforge-packer-build -f "${build_identity}"
# Always present so Packer has a file to upload; empty keeps the image unjoined.
tailscale_preauth_key_file="${staging_dir}/tailscale-preauth-key"
: > "${tailscale_preauth_key_file}"
chmod 0600 "${tailscale_preauth_key_file}"
if [ "${tailscale_enabled}" = true ]; then
  printf '%s\n' "${tailscale_preauth_key}" > "${tailscale_preauth_key_file}"
fi
unset tailscale_preauth_key
# Same shape for the optional sudo password; empty keeps NOPASSWD sudo.
privileged_sudo_password_file="${staging_dir}/privileged-sudo-password"
: > "${privileged_sudo_password_file}"
chmod 0600 "${privileged_sudo_password_file}"
if [ -n "${privileged_sudo_password}" ]; then
  printf '%s\n' "${privileged_sudo_password}" > "${privileged_sudo_password_file}"
fi
unset privileged_sudo_password
guest_manifest_file="${staging_dir}/guest-manifest.json"
dependency_ca_file="${staging_dir}/dependency-ca.pem"
build_provenance_file="${staging_dir}/build-provenance.json"
packer_output_directory="${output_directory}"
dependency_ca_sha256="$(prepare_dependency_ca "${dependency_ca_input}" \
  "${dependency_ca_file}")"
write_build_provenance "${build_provenance_file}" "${runner_api_url}" \
  "${runner_download_base}" "${runner_key_fingerprint}" \
  "${dependency_ca_sha256}"

if [ "${mode}" = validate-only ]; then
  packer_output_directory="${staging_dir}/output"
  forgejo_runner_version=0.0.0
  forgejo_runner_sha256="$(printf '0%.0s' {1..64})"
  forgejo_runner_binary="${staging_dir}/forgejo-runner-placeholder"
  : > "${forgejo_runner_binary}"
else
  require_command curl
  is_curl_transfer_bound_supported \
    || die "curl 8.4 or newer is required for bounded unknown-length downloads"
  require_command file
  require_command gpg
  runner_result="$(resolve_forgejo_runner "${staging_dir}" "${runner_api_url}" \
    "${runner_download_base}" "${runner_key_fingerprint}")"
  IFS=$'\t' read -r forgejo_runner_binary forgejo_runner_version \
    forgejo_runner_sha256 <<< "${runner_result}"
fi

packer_arguments=(
  -var "source_image_url=${source_image_url}"
  -var "source_image_sha256=${source_image_sha256}"
  -var "output_directory=${packer_output_directory}"
  -var "image_name=${image_name}.qcow2"
  -var "cpu_count=${cpu_count}"
  -var "memory_mb=${memory_mb}"
  -var "disk_size_mb=${disk_size_mb}"
  -var "build_ssh_private_key_file=${build_identity}"
  -var "build_ssh_public_key_file=${build_identity}.pub"
  -var "forgejo_runner_binary_file=${forgejo_runner_binary}"
  -var "forgejo_runner_version=${forgejo_runner_version}"
  -var "forgejo_runner_sha256=${forgejo_runner_sha256}"
  -var "dependency_proxy_url=${dependency_proxy_url}"
  -var "cargo_index_url=${dependency_cargo_index_url}"
  -var "npm_registry_url=${dependency_npm_registry_url}"
  -var "python_index_url=${dependency_python_index_url}"
  -var "pytorch_index_url=${dependency_pytorch_index_url}"
  -var "maven_repository_url=${dependency_maven_repository_url}"
  -var "google_maven_repository_url=${dependency_google_maven_repository_url}"
  -var "container_registry_mirror=${container_registry_mirror}"
  -var "dependency_ca_file=${dependency_ca_file}"
  -var "gradle_init_script_file=${template_dir}/../shared/dependencies/chilled-proxy.init.gradle"
  -var "guest_manifest_file=${guest_manifest_file}"
  -var "privileged_account_enabled=${privileged_account_enabled}"
  -var "privileged_sudo_password_file=${privileged_sudo_password_file}"
  -var "tailscale_enabled=${tailscale_enabled}"
  -var "tailscale_login_server=${tailscale_login_server}"
  -var "tailscale_hostname=${tailscale_hostname}"
  -var "tailscale_extra_args=${tailscale_extra_args}"
  -var "tailscale_preauth_key_file=${tailscale_preauth_key_file}"
)

export CHECKPOINT_DISABLE=1
export PACKER_CACHE_DIR="${PACKER_CACHE_DIR:-${HOME}/.cache/packer}"
ensure_absolute_path PACKER_CACHE_DIR "${PACKER_CACHE_DIR}"
mkdir -p "${PACKER_CACHE_DIR}"

(
  cd "${template_dir}"
  print_command packer init .
  packer init .
  print_command packer fmt -check flanforge-base.pkr.hcl
  packer fmt -check flanforge-base.pkr.hcl
  if [ "${mode}" = validate-only ]; then
    print_command packer validate -syntax-only "${packer_arguments[@]}" flanforge-base.pkr.hcl
    packer validate -syntax-only "${packer_arguments[@]}" flanforge-base.pkr.hcl
  else
    print_command packer validate "${packer_arguments[@]}" flanforge-base.pkr.hcl
    packer validate "${packer_arguments[@]}" flanforge-base.pkr.hcl
  fi
)

if [ "${mode}" = validate-only ]; then
  echo "Packer syntax and formatting are valid."
  exit 0
fi

echo "==> source: ${source_image_url}"
echo "==> output: ${output_directory}/${image_name}.qcow2"
echo "==> guest: ${cpu_count} CPU, ${memory_mb} MiB RAM, ${disk_size_mb} MiB disk"
echo "==> Forgejo Runner: upstream ${forgejo_runner_version} (${forgejo_runner_sha256})"
if [ "${tailscale_enabled}" = true ]; then
  echo "==> Tailscale login: retained in the image (${tailscale_login_server})"
else
  echo "==> Tailscale login: none; the image stays unjoined"
fi

require_command qemu-img
require_command qemu-system-x86_64
[ -c /dev/kvm ] && [ -r /dev/kvm ] && [ -w /dev/kvm ] \
  || die "/dev/kvm must be readable and writable by the image builder"

if [ "${mode}" = dry-run ]; then
  print_command packer build "${packer_arguments[@]}" flanforge-base.pkr.hcl
  echo "Dry run complete; QEMU was not started."
  exit 0
fi

(
  cd "${template_dir}"
  print_command packer build "${packer_arguments[@]}" flanforge-base.pkr.hcl
  packer build "${packer_arguments[@]}" flanforge-base.pkr.hcl
)

image_path="${output_directory}/${image_name}.qcow2"
[ -f "${image_path}" ] && [ ! -L "${image_path}" ] \
  || die "Packer did not produce the expected regular qcow2 file"
[ -s "${guest_manifest_file}" ] || die "Packer did not return the guest manifest"
[ "$(file_size "${guest_manifest_file}")" -le 16384 ] \
  || die "guest manifest exceeds 16 KiB"

image_info="$(qemu-img info --output=json "${image_path}")"
printf '%s' "${image_info}" | jq -e \
  '.format == "qcow2" and ((."backing-filename" // "") == "")' >/dev/null \
  || die "output must be a standalone qcow2 without a backing file"
jq -e --arg version "${forgejo_runner_version}" --arg digest "${forgejo_runner_sha256}" \
  '.schema_version == 1 and .guest_contract_version == 2 and
   .forgejo_runner.version == $version and .forgejo_runner.sha256 == $digest and
   .podman.rootless == true and .podman.docker_api == true and
   .guest_agent.exec_enabled == true and
   (.guest_agent.block_rpcs | test("^[a-z0-9,-]*$")) and
   .job_account.name == "runner" and .job_account.uid == 2000' \
  "${guest_manifest_file}" >/dev/null || die "guest manifest contract mismatch"

manifest_result="$(write_image_manifest "${image_path}" "${image_info}" \
  "${output_directory}" "${image_name}" "${source_image_url}" \
  "${source_image_sha256}" "${guest_manifest_file}" "${project_root}" \
  "${build_provenance_file}" "${qemu_plugin_version}")"
IFS=$'\t' read -r manifest_path image_sha256 <<< "${manifest_result}"

echo "Built ${image_path}"
echo "Manifest: ${manifest_path}"
echo "SHA-256: ${image_sha256}"
