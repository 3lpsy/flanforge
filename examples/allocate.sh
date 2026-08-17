#!/usr/bin/env bash
# Request and release FlanForge macOS allocations from a Forgejo Actions job.
#
# Drop this into a repository and call it from the allocator job. Every setting
# comes from the environment; the first argument selects the operation.
#
#   allocate.sh allocate            request an allocation, print its label
#   allocate.sh status <id>         print one allocation as JSON
#   allocate.sh cancel <id>         release an allocation
#
# Required environment:
#   FLANFORGED_URL       daemon base URL, e.g. https://mac.example:9843
#   FLANFORGE_PROFILE    profile name to request (allocate only)
#
# Optional environment:
#   FLANFORGE_AUDIENCE   OIDC audience (default: flanforged)
#   FLANFORGE_TIMEOUT    seconds to await the allocation (default: 900)
#   FLANFORGE_WARM       1/true/yes to clone the project's warm image
#   FLANFORGE_CPU_COUNT  guest CPUs, bounded by the profile
#   FLANFORGE_MEMORY_MB  guest memory, bounded by the profile
#
# The workflow must set `enable-openid-connect: true`, which is what makes
# Forgejo inject the identity endpoint this script exchanges for a token.
# GitHub's `permissions: id-token: write` does nothing on Forgejo.
#
# Exit codes: 2 usage, 3 rejected, 4 busy, 5 transport.
set -euo pipefail

EXIT_USAGE=2
EXIT_REJECTED=3
EXIT_BUSY=4
EXIT_TRANSPORT=5

die() {
  echo "error: $1" >&2
  exit "$2"
}

# Reads one flat string field. Enough for the few fields used here; prefer jq
# when the image has it.
json_string() {
  local key="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r --arg key "${key}" '.[$key] // empty'
  else
    sed -n "s/.*\"${key}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n 1
  fi
}

identity_token() {
  [ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ] && [ -n "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}" ] \
    || die "no Actions identity endpoint; the workflow needs 'enable-openid-connect: true'" "${EXIT_USAGE}"
  local separator='?'
  case "${ACTIONS_ID_TOKEN_REQUEST_URL}" in
    *\?*) separator='&' ;;
  esac
  local token
  token="$(curl -fsS -H "Authorization: bearer ${ACTIONS_ID_TOKEN_REQUEST_TOKEN}" \
    "${ACTIONS_ID_TOKEN_REQUEST_URL}${separator}audience=${FLANFORGE_AUDIENCE:-flanforged}" \
    | json_string value)" || die "cannot obtain an identity token" "${EXIT_TRANSPORT}"
  [ -n "${token}" ] || die "identity endpoint returned no token" "${EXIT_TRANSPORT}"
  printf '%s' "${token}"
}

# Sends one authenticated request and maps the status to an exit code.
call() {
  local method="$1" path="$2" token="$3" timeout="$4" body="${5:-}"
  local response status
  response="$(mktemp)"
  trap 'rm -f "${response}"' RETURN
  local -a arguments=(
    -sS -o "${response}" -w '%{http_code}'
    --max-time "${timeout}"
    -X "${method}"
    -H "Authorization: Bearer ${token}"
    -H 'Accept: application/json'
  )
  if [ -n "${body}" ]; then
    arguments+=(-H 'Content-Type: application/json' --data "${body}")
  fi
  status="$(curl "${arguments[@]}" "${url}${path}")" \
    || die "cannot reach ${url}" "${EXIT_TRANSPORT}"
  case "${status}" in
    2*) cat "${response}"; [ -s "${response}" ] && echo ;;
    401|403) die "rejected (${status}): $(head -c 200 "${response}")" "${EXIT_REJECTED}" ;;
    409) die "busy (${status}): $(head -c 200 "${response}")" "${EXIT_BUSY}" ;;
    *) die "request failed (${status}): $(head -c 200 "${response}")" "${EXIT_TRANSPORT}" ;;
  esac
}

command="${1:-}"
[ -n "${command}" ] || die "usage: ${0##*/} allocate|status <id>|cancel <id>" "${EXIT_USAGE}"
[ -n "${FLANFORGED_URL:-}" ] || die "FLANFORGED_URL is required" "${EXIT_USAGE}"
url="${FLANFORGED_URL%/}"
case "${url}" in
  http://*|https://*) ;;
  *) die "FLANFORGED_URL must be an http(s) URL" "${EXIT_USAGE}" ;;
esac

case "${command}" in
  allocate)
    [ -n "${FLANFORGE_PROFILE:-}" ] || die "FLANFORGE_PROFILE is required" "${EXIT_USAGE}"
    [ -n "${GITHUB_REPOSITORY:-}" ] || die "GITHUB_REPOSITORY is required" "${EXIT_USAGE}"
    body="$(printf '{"profile":"%s","repository":"%s","run_id":%s,"run_attempt":%s' \
      "${FLANFORGE_PROFILE}" "${GITHUB_REPOSITORY}" \
      "${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}" \
      "${GITHUB_RUN_ATTEMPT:-1}")"
    # Opt in to the project's warm image; omitted means the trusted base, cold.
    case "${FLANFORGE_WARM:-}" in
      1|true|yes|TRUE|YES) body="${body},\"warm\":true" ;;
    esac
    for pair in "cpu_count:${FLANFORGE_CPU_COUNT:-}" "memory_mb:${FLANFORGE_MEMORY_MB:-}"; do
      value="${pair#*:}"
      [ -n "${value}" ] || continue
      case "${value}" in
        ''|*[!0-9]*) die "${pair%%:*} must be a whole number" "${EXIT_USAGE}" ;;
      esac
      body="${body},\"${pair%%:*}\":${value}"
    done
    body="${body}}"
    # Returns once the guest is booted and its runner is registered, so this
    # blocks for the profile's boot budget.
    # Assign first: a failure inside a command substitution used as an
    # argument would not stop the script.
    token="$(identity_token)"
    allocation="$(call POST /v1/allocations "${token}" "${FLANFORGE_TIMEOUT:-900}" "${body}")"
    label="$(printf '%s' "${allocation}" | json_string runner_label)"
    id="$(printf '%s' "${allocation}" | json_string id)"
    [ -n "${label}" ] && [ -n "${id}" ] \
      || die "allocation response is missing its label or id" "${EXIT_TRANSPORT}"
    echo "allocated ${id} in state $(printf '%s' "${allocation}" | json_string state)" >&2
    echo "runner_label=${label}"
    echo "allocation_id=${id}"
    if [ -n "${GITHUB_OUTPUT:-}" ]; then
      {
        echo "runner_label=${label}"
        echo "allocation_id=${id}"
      } >> "${GITHUB_OUTPUT}"
    fi
    ;;
  status)
    id="${2:-${FLANFORGE_ALLOCATION_ID:-}}"
    [ -n "${id}" ] || die "an allocation id is required" "${EXIT_USAGE}"
    token="$(identity_token)"
    call GET "/v1/allocations/${id}" "${token}" 30
    ;;
  cancel)
    id="${2:-${FLANFORGE_ALLOCATION_ID:-}}"
    [ -n "${id}" ] || die "an allocation id is required" "${EXIT_USAGE}"
    token="$(identity_token)"
    call DELETE "/v1/allocations/${id}" "${token}" 30 >/dev/null
    echo "cancelled ${id}" >&2
    ;;
  *)
    die "unknown command '${command}'" "${EXIT_USAGE}"
    ;;
esac
