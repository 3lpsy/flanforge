#!/usr/bin/env bash
# Exercise the shared template-time Tailscale argument boundary.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../../shared/tailscale/arguments.sh
source "${template_dir}/../shared/tailscale/arguments.sh"

parse_tailscale_extra_arguments "--accept-dns=true --shields-up"
[ "${TAILSCALE_EXTRA_ARGUMENT_COUNT}" -eq 2 ]
[ "${TAILSCALE_EXTRA_ARGUMENTS[0]}" = "--accept-dns=true" ]
[ "${TAILSCALE_EXTRA_ARGUMENTS[1]}" = "--shields-up" ]

for reserved in auth-key authkey login-server operator hostname; do
  for prefix in - --; do
    if parse_tailscale_extra_arguments "${prefix}${reserved}=override"; then
      echo "reserved Tailscale flag was accepted: ${prefix}${reserved}" >&2
      exit 1
    fi
    if parse_tailscale_extra_arguments "${prefix}${reserved} override"; then
      echo "split reserved Tailscale flag was accepted: ${prefix}${reserved}" >&2
      exit 1
    fi
  done
done

if parse_tailscale_extra_arguments $'--accept-dns=true\t--shields-up'; then
  echo "control character was accepted in Tailscale arguments" >&2
  exit 1
fi

echo "Tailscale argument fake tests passed"
