#!/usr/bin/env bash
# Parse bounded Tailscale options while reserving enrollment-owned flags.
# shellcheck disable=SC2034 # Outputs are consumed by sourcing callers.

TAILSCALE_EXTRA_ARGUMENTS=()
TAILSCALE_EXTRA_ARGUMENT_COUNT=0

parse_tailscale_extra_arguments() {
  local value="$1"
  local argument
  local option
  local -a parsed

  TAILSCALE_EXTRA_ARGUMENTS=()
  TAILSCALE_EXTRA_ARGUMENT_COUNT=0
  [ "${#value}" -le 4096 ] || return 1
  if LC_ALL=C grep -q '[[:cntrl:]]' <<< "${value}"; then
    return 1
  fi
  [[ "${value}" =~ [^[:space:]] ]] || return 0
  read -r -a parsed <<< "${value}"
  [ "${#parsed[@]}" -le 64 ] || return 1

  for argument in "${parsed[@]}"; do
    [ "${#argument}" -le 512 ] || return 1
    case "${argument}" in
      -*)
        option="${argument#-}"
        option="${option#-}"
        option="${option%%=*}"
        case "${option}" in
          auth-key|authkey|login-server|operator|hostname) return 1 ;;
        esac
        ;;
    esac
  done

  TAILSCALE_EXTRA_ARGUMENTS=("${parsed[@]}")
  TAILSCALE_EXTRA_ARGUMENT_COUNT="${#parsed[@]}"
}
