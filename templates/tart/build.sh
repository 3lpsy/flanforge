#!/usr/bin/env bash
# Manually build the retained flanforge-base Tart template on Apple Silicon.
set -euo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd "${template_dir}/../.." && pwd)"
# shellcheck source=../shared/env.sh
source "${template_dir}/../shared/env.sh"
# shellcheck source=../shared/dependencies/routes.sh
source "${template_dir}/../shared/dependencies/routes.sh"
# shellcheck source=../shared/tailscale/arguments.sh
source "${template_dir}/../shared/tailscale/arguments.sh"

die() {
  echo "error: $*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    "Usage:" \
    "  ${0##*/} [--dry-run] [--skip-smoke] [forgejo-runner-darwin-arm64]" \
    "  ${0##*/} -h | --help | help" \
    "" \
    "Build the retained FlanForge Tart template. The optional positional" \
    "binary overrides FLANFORGE_RUNNER_BINARY_FILE." \
    "--dry-run performs setup, init, format checking, validation, and the" \
    "base-image pull, then prints but does not execute the Packer build." \
    "--skip-smoke retains the template without running the job-path check." \
    "" \
    "Settings may be added to templates/tart/.env as trusted shell syntax." \
    "See templates/tart/.env.example for a complete configuration." \
    "A root .env remains a deprecated fallback for one release." \
    "" \
    "Environment:" \
    "  FLANFORGE_RUNNER_BINARY_FILE          Local Darwin/ARM64 runner (required without argument)" \
    "  FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE   Existing guest public key (default: create expected key pair)" \
    "  TART_BASE_IMAGE                       Source Tart image (default: Tahoe/Xcode latest)" \
    "  TART_VM_NAME                          Retained template name (default: flanforge-base)" \
    "  TART_VM_CPU                           Virtual CPU count (default: 8)" \
    "  TART_VM_MEMORY_GB                     Memory in GiB (default: 12)" \
    "  TART_VM_DISK_GB                       Disk override; 0 inherits base (default: 0)" \
    "  TART_HOME                             Tart storage (default: ~/.tart)" \
    "  PACKER_TMP_DIR                        Large temporary files (default: TART_HOME/packer-tmp)" \
    "  PACKER_CACHE_DIR                      Packer cache (default: TART_HOME/packer-cache)" \
    "  SERVICES_ROOT_DOMAIN                  Optional dependency-proxy root domain" \
    "  FLANFORGE_PREWARM_ENABLED             Prewarm one simulator (default: true)" \
    "  FLANFORGE_PREWARM_TARGET              Exact simulator name (default: iPhone 17 Pro)" \
    "  FLANFORGE_PRIVACY_GRANTS_ENABLED      Grant runner capture/input/automation (default: true)" \
    "  RUST_TOOLCHAIN                        Rust toolchain (default: stable)" \
    "  JUST_VERSION                          just version (default: 1.57.0)" \
    "  NEXTEST_VERSION                       cargo-nextest version (default: 0.9.140)" \
    "  SCCACHE_VERSION                       sccache version (default: 0.17.0)" \
    "  FLANFORGE_TAILSCALE_ENABLED           Retain an authenticated node (default: false)" \
    "  FLANFORGE_TAILSCALE_PREAUTH_KEY       Sensitive pre-auth key (required when enabled)" \
    "  FLANFORGE_TAILSCALE_LOGIN_SERVER      Headscale/Tailscale HTTP(S) origin (required when enabled)" \
    "  FLANFORGE_TAILSCALE_HOSTNAME          Optional node hostname" \
    "  FLANFORGE_TAILSCALE_EXTRA_ARGS        String of additional tailscale up arguments" \
    "  FLANFORGE_ADMIN_PASSWORD              New template admin password (required by default)" \
    "  DISABLE_ADMIN_PASSWORD_CHANGE         Keep the base-image password (default: false)" \
    "  FLANFORGE_SMOKE_ENABLED               Gate retention on the job-path check (default: true)" \
    "  FLANFORGE_SMOKE_KEEP_FAILED           Keep a template that fails it (default: false)" \
    "" \
    "Default guest identity:" \
    "  ~/Library/Application Support/flanforge/guest-ssh-key" >&2
}

print_packer() {
  printf '==> + packer'
  printf ' %q' "$@"
  printf '\n'
}

run_packer() {
  print_packer "$@"
  packer "$@"
}

dry_run=false
skip_smoke=false
positional_arguments=()
# Counted separately: bash 3.2 (macOS) treats an empty array as unset, so
# `${#positional_arguments[@]}` would abort under `set -u`.
positional_argument_count=0
for argument in "$@"; do
  case "${argument}" in
    -h|--help|help)
      usage
      exit 0
      ;;
    --dry-run)
      dry_run=true
      ;;
    --skip-smoke)
      skip_smoke=true
      ;;
    -*)
      usage
      die "unknown option '${argument}'"
      ;;
    *)
      positional_arguments+=("${argument}")
      positional_argument_count=$((positional_argument_count + 1))
      ;;
  esac
done

load_tart_template_environment "${template_dir}" "${project_root}"

[ "${positional_argument_count}" -le 1 ] || {
  usage
  die "at most one Forgejo Runner binary may be provided"
}

command -v tart >/dev/null 2>&1 \
  || die "tart is required (brew install cirruslabs/cli/tart)"
command -v packer >/dev/null 2>&1 \
  || die "packer is required (brew install packer)"
command -v file >/dev/null 2>&1 \
  || die "file is required"
command -v ssh-keygen >/dev/null 2>&1 \
  || die "ssh-keygen is required"

runner_binary_input="${positional_arguments[0]:-${FLANFORGE_RUNNER_BINARY_FILE:-}}"
[ -n "${runner_binary_input}" ] || {
  usage
  die "provide a local Forgejo Runner binary or set FLANFORGE_RUNNER_BINARY_FILE"
}
[[ "${runner_binary_input}" != *$'\n'* && "${runner_binary_input}" != *$'\r'* ]] \
  || die "Forgejo Runner binary path contains unsupported characters"
[ -f "${runner_binary_input}" ] \
  || die "Forgejo Runner binary not found at '${runner_binary_input}'"
[ -s "${runner_binary_input}" ] \
  || die "Forgejo Runner binary is empty"
runner_binary_dir="$(cd "$(dirname "${runner_binary_input}")" && pwd -P)"
runner_binary="${runner_binary_dir}/$(basename "${runner_binary_input}")"
LC_ALL=C file "${runner_binary}" | grep -Eq 'Mach-O.*arm64' \
  || die "Forgejo Runner binary must be a Darwin ARM64 Mach-O executable"

runner_pin_file="${project_root}/ci/forgejo-runner.env"
[ -f "${runner_pin_file}" ] \
  || die "runner pin not found at '${runner_pin_file}'"
[ "$(grep -c '^FORGEJO_RUNNER_VERSION=' "${runner_pin_file}")" -eq 1 ] \
  || die "runner pin must contain exactly one version"
runner_version="$(sed -n 's/^FORGEJO_RUNNER_VERSION=//p' "${runner_pin_file}")"
[[ "${runner_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "Forgejo Runner version pin is invalid"

# Routed through the dependency proxy when a services domain is configured.
base_registry="${TART_BASE_REGISTRY:-}"
if [ -z "${base_registry}" ] && [ -n "${SERVICES_ROOT_DOMAIN:-}" ]; then
  base_registry="registry-ghcr.${SERVICES_ROOT_DOMAIN}"
fi
base_image="${TART_BASE_IMAGE:-${base_registry:-ghcr.io}/cirruslabs/macos-tahoe-xcode:latest}"
vm_name="${TART_VM_NAME:-flanforge-base}"
cpu_count="${TART_VM_CPU:-8}"
memory_gb="${TART_VM_MEMORY_GB:-12}"
disk_size_gb="${TART_VM_DISK_GB:-0}"
services_root_domain="${SERVICES_ROOT_DOMAIN:-}"
prewarm_enabled="${FLANFORGE_PREWARM_ENABLED:-true}"
prewarm_target="${FLANFORGE_PREWARM_TARGET:-iPhone 17 Pro}"
privacy_grants_enabled="${FLANFORGE_PRIVACY_GRANTS_ENABLED:-true}"
rust_toolchain="${RUST_TOOLCHAIN:-stable}"
just_version="${JUST_VERSION:-1.57.0}"
nextest_version="${NEXTEST_VERSION:-0.9.140}"
sccache_version="${SCCACHE_VERSION:-0.17.0}"
tailscale_enabled="${FLANFORGE_TAILSCALE_ENABLED:-false}"
tailscale_preauth_key="${FLANFORGE_TAILSCALE_PREAUTH_KEY:-}"
tailscale_login_server="${FLANFORGE_TAILSCALE_LOGIN_SERVER:-}"
tailscale_hostname="${FLANFORGE_TAILSCALE_HOSTNAME:-}"
tailscale_extra_args="${FLANFORGE_TAILSCALE_EXTRA_ARGS:-}"
admin_password="${FLANFORGE_ADMIN_PASSWORD:-}"
disable_admin_password_change="${DISABLE_ADMIN_PASSWORD_CHANGE:-false}"
smoke_enabled="${FLANFORGE_SMOKE_ENABLED:-true}"
smoke_keep_failed="${FLANFORGE_SMOKE_KEEP_FAILED:-false}"
if [ "${skip_smoke}" = true ]; then
  smoke_enabled=false
fi
# Do not expose the secrets to unrelated child processes such as Tart. Both the
# Tailscale key and the admin password are uploaded through private temporary
# files, so neither reaches a Packer or guest command line.
unset FLANFORGE_TAILSCALE_PREAUTH_KEY FLANFORGE_ADMIN_PASSWORD
default_guest_identity="${HOME}/Library/Application Support/flanforge/guest-ssh-key"
guest_key="${FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE:-}"
generated_guest_key=false
repaired_guest_public_key=false

sync_default_guest_public_key() {
  local current_fingerprint
  local derived_fingerprint
  local temporary_public_key

  temporary_public_key="$(mktemp "${guest_key}.tmp.XXXXXX")" \
    || die "cannot create a temporary guest public key"
  if ! ssh-keygen -y -P "" -f "${default_guest_identity}" > "${temporary_public_key}" 2>/dev/null; then
    rm -f "${temporary_public_key}"
    die "default guest private key must be an unencrypted SSH identity"
  fi
  chmod 0644 "${temporary_public_key}"

  derived_fingerprint="$(ssh-keygen -l -f "${temporary_public_key}" 2>/dev/null | awk 'NR == 1 { print $2 }')"
  [ -n "${derived_fingerprint}" ] || {
    rm -f "${temporary_public_key}"
    die "cannot derive the default guest public key"
  }

  current_fingerprint=""
  if [ -f "${guest_key}" ] \
    && [ "$(awk 'NF { count += 1 } END { print count + 0 }' "${guest_key}")" -eq 1 ]; then
    current_fingerprint="$(ssh-keygen -l -f "${guest_key}" 2>/dev/null | awk 'NR == 1 { print $2 }')"
  fi

  if [ "${derived_fingerprint}" = "${current_fingerprint}" ]; then
    rm -f "${temporary_public_key}"
    return
  fi

  if ! mv -f "${temporary_public_key}" "${guest_key}"; then
    rm -f "${temporary_public_key}"
    die "cannot replace the default guest public key"
  fi
  repaired_guest_public_key=true
}

if [ -z "${guest_key}" ]; then
  guest_identity="${default_guest_identity}"
  guest_key="${guest_identity}.pub"
  guest_identity_dir="$(dirname "${guest_identity}")"
  mkdir -p "${guest_identity_dir}"
  chmod 0700 "${guest_identity_dir}"

  if [ ! -e "${guest_identity}" ] && [ ! -e "${guest_key}" ]; then
    ssh-keygen -q -t ed25519 -N "" -C "flanforge-tart-guest" -f "${guest_identity}"
    generated_guest_key=true
  fi

  [ -f "${guest_identity}" ] \
    || die "default guest public key exists without its expected private key at '${guest_identity}'"
  if [ -e "${guest_key}" ] && [ ! -f "${guest_key}" ]; then
    die "default guest public key path is not a regular file at '${guest_key}'"
  fi
  sync_default_guest_public_key
fi

[[ "${base_image}" =~ ^[A-Za-z0-9][A-Za-z0-9./:@_-]+$ ]] \
  || die "TART_BASE_IMAGE contains unsupported characters"
[[ "${vm_name}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] \
  || die "TART_VM_NAME must be a safe Tart VM name"
[[ "${cpu_count}" =~ ^[1-9][0-9]*$ ]] \
  || die "TART_VM_CPU must be a positive integer"
[[ "${memory_gb}" =~ ^[1-9][0-9]*$ ]] \
  || die "TART_VM_MEMORY_GB must be a positive integer"
[[ "${disk_size_gb}" =~ ^[0-9]+$ ]] \
  || die "TART_VM_DISK_GB must be zero or a positive integer"
[[ "${prewarm_enabled}" =~ ^(true|false)$ ]] \
  || die "FLANFORGE_PREWARM_ENABLED must be true or false"
[[ "${prewarm_target}" =~ ^[A-Za-z0-9][A-Za-z0-9._()\ -]{0,79}$ ]] \
  || die "FLANFORGE_PREWARM_TARGET contains unsupported characters"
[[ "${privacy_grants_enabled}" =~ ^(true|false)$ ]] \
  || die "FLANFORGE_PRIVACY_GRANTS_ENABLED must be true or false"
[[ "${rust_toolchain}" =~ ^(stable|beta|nightly|[0-9]+\.[0-9]+\.[0-9]+)(-[A-Za-z0-9._-]+)?$ ]] \
  || die "RUST_TOOLCHAIN contains unsupported characters"
[[ "${just_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "JUST_VERSION must be a semantic version"
[[ "${nextest_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "NEXTEST_VERSION must be a semantic version"
[[ "${sccache_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "SCCACHE_VERSION must be a semantic version"
[[ "${tailscale_enabled}" =~ ^(true|false)$ ]] \
  || die "FLANFORGE_TAILSCALE_ENABLED must be true or false"
if [ "${tailscale_enabled}" = true ]; then
  [ -n "${tailscale_preauth_key}" ] \
    || die "FLANFORGE_TAILSCALE_PREAUTH_KEY is required when Tailscale is enabled"
  [[ "${tailscale_login_server}" =~ ^https?://[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?(:[0-9]{1,5})?/?$ ]] \
    || die "FLANFORGE_TAILSCALE_LOGIN_SERVER must be an HTTP(S) origin"
fi
if [ -n "${tailscale_hostname}" ]; then
  [[ "${tailscale_hostname}" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$ ]] \
    || die "FLANFORGE_TAILSCALE_HOSTNAME must be a valid host label"
fi
parse_tailscale_extra_arguments "${tailscale_extra_args}" \
  || die "FLANFORGE_TAILSCALE_EXTRA_ARGS contains invalid or reserved options"
[[ "${disable_admin_password_change}" =~ ^(true|false)$ ]] \
  || die "DISABLE_ADMIN_PASSWORD_CHANGE must be true or false"
[[ "${smoke_enabled}" =~ ^(true|false)$ ]] \
  || die "FLANFORGE_SMOKE_ENABLED must be true or false"
[[ "${smoke_keep_failed}" =~ ^(true|false)$ ]] \
  || die "FLANFORGE_SMOKE_KEEP_FAILED must be true or false"
[ ! -f "${template_dir}/smoke.sh" ] || [ -x "${template_dir}/smoke.sh" ] \
  || die "templates/tart/smoke.sh is not executable"
if [ "${smoke_enabled}" = true ] && [ ! -f "${template_dir}/smoke.sh" ]; then
  die "the job-path smoke check is enabled but templates/tart/smoke.sh is missing"
fi
if [ "${disable_admin_password_change}" = false ]; then
  [ "${#admin_password}" -ge 16 ] && [ "${#admin_password}" -le 128 ] \
    || die "FLANFORGE_ADMIN_PASSWORD must contain 16 to 128 characters"
  printf '%s' "${admin_password}" | LC_ALL=C grep -Eq '^[[:graph:]]{16,128}$' \
    || die "FLANFORGE_ADMIN_PASSWORD must contain only visible ASCII characters"
fi
if [ -n "${services_root_domain}" ]; then
  is_valid_dependency_domain "${services_root_domain}" \
    || die "SERVICES_ROOT_DOMAIN must be a DNS name"
fi
[[ "${guest_key}" = /* ]] \
  || die "FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE must be an absolute path"
[ -f "${guest_key}" ] \
  || die "guest SSH public key not found at '${guest_key}'"
[ "$(awk 'NF { count += 1 } END { print count + 0 }' "${guest_key}")" -eq 1 ] \
  || die "guest SSH public key file must contain exactly one non-empty line"
ssh-keygen -l -f "${guest_key}" >/dev/null 2>&1 \
  || die "guest SSH public key is invalid"

if [ "${generated_guest_key}" = true ]; then
  echo "==> created guest SSH identity"
  echo "    private key: ${default_guest_identity}"
  echo "    public key:  ${guest_key}"
  echo "    config path: guest.ssh.identity_file = \"${default_guest_identity}\""
  echo "    fingerprint: $(ssh-keygen -l -f "${guest_key}")"
elif [ "${repaired_guest_public_key}" = true ]; then
  echo "==> repaired guest SSH public key from its private identity"
  echo "    private key: ${default_guest_identity}"
  echo "    public key:  ${guest_key}"
  echo "    fingerprint: $(ssh-keygen -l -f "${guest_key}")"
fi

if tart get "${vm_name}" >/dev/null 2>&1; then
  die "Tart VM '${vm_name}' already exists; delete/rename it explicitly or set TART_VM_NAME"
fi

# Packer and Tart can create large temporary copies. PACKER_TMP_DIR is the
# user-facing override; TMPDIR is what Packer and the Tart plugin consume.
storage_root="${TART_HOME:-${HOME}/.tart}"
packer_tmp_dir="${PACKER_TMP_DIR:-${storage_root}/packer-tmp}"
packer_cache_dir="${PACKER_CACHE_DIR:-${storage_root}/packer-cache}"
[[ "${packer_tmp_dir}" = /* ]] || die "PACKER_TMP_DIR must be absolute"
[[ "${packer_cache_dir}" = /* ]] || die "PACKER_CACHE_DIR must be absolute"
mkdir -p "${packer_tmp_dir}" "${packer_cache_dir}"
chmod 700 "${packer_tmp_dir}"
export TMPDIR="${packer_tmp_dir}"
export PACKER_CACHE_DIR="${packer_cache_dir}"

admin_password_file="$(mktemp "${TMPDIR}/flanforge-admin-password.XXXXXX")" \
  || die "cannot create temporary admin password file"
tailscale_preauth_key_file="$(mktemp "${TMPDIR}/flanforge-tailscale-key.XXXXXX")" \
  || die "cannot create temporary tailnet key file"
cleanup_secrets() {
  rm -f "${admin_password_file}" "${tailscale_preauth_key_file}"
}
trap cleanup_secrets EXIT
trap 'exit 130' HUP INT TERM
chmod 0600 "${admin_password_file}" "${tailscale_preauth_key_file}"
if [ "${disable_admin_password_change}" = false ]; then
  printf '%s\n' "${admin_password}" > "${admin_password_file}"
fi
if [ "${tailscale_enabled}" = true ]; then
  printf '%s\n' "${tailscale_preauth_key}" > "${tailscale_preauth_key_file}"
fi
unset admin_password tailscale_preauth_key

echo "==> base: ${base_image}"
echo "==> output: ${vm_name} (${cpu_count} CPU, ${memory_gb} GB RAM, disk override ${disk_size_gb} GB)"
echo "==> Forgejo Runner: ${runner_binary} (expected v${runner_version})"
echo "==> simulator prewarm: ${prewarm_enabled} (${prewarm_target})"
echo "==> runner privacy grants: ${privacy_grants_enabled}"
echo "==> job-path smoke check before retention: ${smoke_enabled}"
if [ "${tailscale_enabled}" = true ]; then
  echo "==> Tailscale template enrollment: enabled (${tailscale_login_server})"
else
  echo "==> Tailscale template enrollment: disabled"
fi
if [ "${disable_admin_password_change}" = true ]; then
  echo "==> admin password change: explicitly disabled"
else
  echo "==> admin password change: enabled"
fi
echo "==> Packer temp: ${TMPDIR}"
echo "==> Packer cache: ${PACKER_CACHE_DIR}"
if [ -n "${services_root_domain}" ]; then
  echo "==> dependency routing: configured"
else
  echo "==> dependency routing: not baked; workflows must provide Cargo registry configuration"
fi

packer_vars=(
  -var "base_image=${base_image}"
  -var "vm_name=${vm_name}"
  -var "cpu_count=${cpu_count}"
  -var "memory_gb=${memory_gb}"
  -var "disk_size_gb=${disk_size_gb}"
  -var "services_root_domain=${services_root_domain}"
  -var "guest_ssh_public_key_file=${guest_key}"
  -var "forgejo_runner_binary_file=${runner_binary}"
  -var "forgejo_runner_version=${runner_version}"
  -var "prewarm_enabled=${prewarm_enabled}"
  -var "prewarm_target=${prewarm_target}"
  -var "privacy_grants_enabled=${privacy_grants_enabled}"
  -var "rust_toolchain=${rust_toolchain}"
  -var "just_version=${just_version}"
  -var "nextest_version=${nextest_version}"
  -var "sccache_version=${sccache_version}"
  -var "tailscale_enabled=${tailscale_enabled}"
  -var "tailscale_login_server=${tailscale_login_server}"
  -var "tailscale_hostname=${tailscale_hostname}"
  -var "tailscale_extra_args=${tailscale_extra_args}"
  -var "tailscale_preauth_key_file=${tailscale_preauth_key_file}"
  -var "admin_password_file=${admin_password_file}"
  -var "disable_admin_password_change=${disable_admin_password_change}"
)

# Validate before the tens-of-GB base pull.
(
  cd "${template_dir}"
  run_packer init .
  run_packer fmt -check flanforge-base.pkr.hcl
  run_packer validate "${packer_vars[@]}" flanforge-base.pkr.hcl
)

# Pre-pull so the large Xcode image download is visible and independently
# resumable before Packer begins provisioning.
tart pull "${base_image}"

(
  cd "${template_dir}"
  if [ "${dry_run}" = true ]; then
    print_packer build "${packer_vars[@]}" flanforge-base.pkr.hcl
  else
    run_packer build "${packer_vars[@]}" flanforge-base.pkr.hcl
  fi
)

echo
if [ "${dry_run}" != true ]; then
  if [ "${smoke_enabled}" = true ]; then
    # Provisioning only ever saw the bootstrap account's session, so the guest
    # is unproven until the job-time path runs. The check works on a clone.
    if ! "${template_dir}/smoke.sh" "${vm_name}"; then
      if [ "${smoke_keep_failed}" = true ]; then
        die "'${vm_name}' failed the job-path smoke check and was kept for inspection"
      fi
      echo "==> deleting '${vm_name}': it failed the job-path smoke check" >&2
      tart delete "${vm_name}" >/dev/null 2>&1 \
        || echo "warning: could not delete '${vm_name}'; remove it manually" >&2
      die "'${vm_name}' failed the job-path smoke check and was not retained"
    fi
  else
    echo "==> job-path smoke check skipped; the template is unproven for real jobs"
  fi
fi

if [ "${dry_run}" = true ]; then
  echo "Dry run complete; Packer build was not executed."
else
  echo "Built stopped Tart template '${vm_name}'."
  if [ "${tailscale_enabled}" = true ]; then
    echo "Tailscale is authenticated and will reconnect on boot; clones share the retained node identity."
  else
    echo "Tailscale is installed but intentionally logged out."
  fi
fi
