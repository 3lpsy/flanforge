#!/usr/bin/env bash
# Shared host-side validation for libvirt template tooling.

die() {
  echo "error: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

run_bounded() {
  local maximum_seconds="$1"
  shift
  [[ "${maximum_seconds}" =~ ^[1-9][0-9]*$ ]] || return 2
  [ "$#" -gt 0 ] || return 2
  timeout --foreground --kill-after=5s "${maximum_seconds}s" "$@"
}

ensure_linux_x86_64() {
  [ "$(uname -s)" = Linux ] || die "the libvirt template requires Linux"
  [ "$(uname -m)" = x86_64 ] || die "the first libvirt template supports x86_64 only"
}

ensure_safe_name() {
  local label="$1"
  local value="$2"
  [[ "${value}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] \
    || die "${label} must be a safe name of at most 64 characters"
}

ensure_positive_integer() {
  local label="$1"
  local value="$2"
  local maximum="$3"
  [[ "${value}" =~ ^[1-9][0-9]*$ ]] \
    || die "${label} must be a positive integer"
  [ "${value}" -le "${maximum}" ] \
    || die "${label} must not exceed ${maximum}"
}

ensure_https_url() {
  local label="$1"
  local value="$2"
  local authority
  local host
  local path=""
  local port=""
  local remainder
  [ "${#value}" -le 2048 ] && [[ "${value}" == https://* ]] \
    || die "${label} must be a credential-free HTTPS URL"
  remainder="${value#https://}"
  authority="${remainder%%/*}"
  [[ "${authority}" != *:*:* ]] \
    || die "${label} does not support IPv6 URL literals"
  host="${authority%%:*}"
  if [[ "${authority}" == *:* ]]; then
    port="${authority##*:}"
  fi
  is_valid_domain "${host}" \
    || die "${label} URL host is not a valid DNS name"
  if [ -n "${port}" ]; then
    is_valid_port "${port}" || die "${label} URL port must be between 1 and 65535"
  elif [[ "${authority}" == *:* ]]; then
    die "${label} URL port is empty"
  fi
  if [[ "${remainder}" == */* ]]; then
    path="${remainder#*/}"
    [[ "${path}" =~ ^[A-Za-z0-9._~:/@%+-]*$ ]] \
      || die "${label} URL path has unsupported characters"
    case "${path}" in
      *'//'*) die "${label} URL path is not normalized" ;;
    esac
    case "/${path}/" in
      *'/./'*|*'/../'*) die "${label} URL path contains a dot segment" ;;
    esac
  fi
}

ensure_domain() {
  local label="$1"
  local value="$2"
  [ -z "${value}" ] && return
  is_valid_domain "${value}" || die "${label} must be a valid DNS name"
}

is_valid_domain() {
  local value="$1"
  local domain_label
  local -a domain_labels
  [ -n "${value}" ] && [ "${#value}" -le 253 ] || return 1
  [[ "${value}" != .* && "${value}" != *. && "${value}" != *..* ]] || return 1
  IFS=. read -r -a domain_labels <<< "${value}"
  for domain_label in "${domain_labels[@]}"; do
    [ "${#domain_label}" -le 63 ] || return 1
    [[ "${domain_label}" =~ ^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$ ]] \
      || return 1
  done
}

is_valid_port() {
  local value="$1"
  [[ "${value}" =~ ^[0-9]{1,5}$ ]] \
    && [ "$((10#${value}))" -ge 1 ] && [ "$((10#${value}))" -le 65535 ]
}

# Structural only, matching the daemon: a pre-auth key stays an opaque
# credential, so no vendor prefix is assumed and the value is never printed.
is_preauth_key_shaped() {
  local value="$1"
  [ "${#value}" -ge 8 ] && [ "${#value}" -le 512 ] \
    && [ -z "${value//[[:graph:]]/}" ]
}

is_valid_host_label() {
  local value="$1"
  [[ "${value}" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$ ]]
}

ensure_registry_location() {
  local value="$1"
  local authority
  local host
  local path=""
  local port=""
  local segment
  local -a segments
  [ -z "${value}" ] && return
  [ "${#value}" -le 512 ] \
    || die "container registry mirror must not exceed 512 characters"
  authority="${value%%/*}"
  if [[ "${value}" == */* ]]; then
    path="${value#*/}"
  fi
  [[ "${authority}" != *:*:* ]] \
    || die "container registry mirror does not support IPv6 literals"
  host="${authority%%:*}"
  if [[ "${authority}" == *:* ]]; then
    port="${authority##*:}"
  fi
  is_valid_domain "${host}" \
    || die "container registry mirror host is not a valid DNS name"
  if [ -n "${port}" ]; then
    is_valid_port "${port}" \
      || die "container registry mirror port must be between 1 and 65535"
  elif [[ "${authority}" == *:* ]]; then
    die "container registry mirror port is empty"
  fi
  if [[ "${value}" == */* ]]; then
    [ -n "${path}" ] || die "container registry mirror path is empty"
    case "/${path}/" in
      *'//'*) die "container registry mirror path is not normalized" ;;
    esac
    IFS=/ read -r -a segments <<< "${path}"
    for segment in "${segments[@]}"; do
      [ "${segment}" != . ] && [ "${segment}" != .. ] \
        && [ -n "${segment}" ] && [ "${#segment}" -le 128 ] \
        && [[ "${segment}" =~ ^[A-Za-z0-9._-]+$ ]] \
        || die "container registry mirror path has an invalid segment"
    done
  fi
}

ensure_absolute_path() {
  local label="$1"
  local value="$2"
  [[ "${value}" = /* ]] || die "${label} must be absolute"
  [[ "${value}" != *$'\n'* && "${value}" != *$'\r'* ]] \
    || die "${label} contains unsupported characters"
  case "/${value#/}/" in
    *'/../'*|*'/./'*|*'//'*) die "${label} must be normalized" ;;
  esac
  [ "${value}" != / ] || die "${label} must not be the filesystem root"
}

sha256_file() {
  sha256sum "$1" | awk '{ print $1 }'
}

file_size() {
  stat -c '%s' "$1"
}

print_command() {
  printf '==> +'
  printf ' %q' "$@"
  printf '\n'
}
