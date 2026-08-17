#!/usr/bin/env bash
# Wait for FlanForge capacity, then allocate.
#
# The daemon runs a bounded number of guests, so a request that arrives while
# another job holds the slot is answered busy immediately. That is the right
# answer for a client that can retry, and the wrong one for a release workflow
# nobody wants to trigger twice. This retries until capacity frees up.
#
# Wraps allocate.sh rather than reimplementing it, so both stay in step.
#
#   allocator-patient.sh                 wait up to FLANFORGE_WAIT seconds
#
# Environment, in addition to everything allocate.sh reads:
#   FLANFORGE_WAIT       total seconds to keep retrying (default 3600)
#   FLANFORGE_POLL       seconds between attempts (default 30)
#   FLANFORGE_ALLOCATE   path to allocate.sh (default: next to this script)
#
# Exit codes are allocate.sh's: 0 success, 2 usage, 3 rejected, 4 still busy
# when the wait budget ran out, 5 transport.
set -euo pipefail

EXIT_USAGE=2
EXIT_BUSY=4

allocate="${FLANFORGE_ALLOCATE:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/allocate.sh}"
[ -f "${allocate}" ] || {
  echo "error: allocate.sh not found at ${allocate}; set FLANFORGE_ALLOCATE" >&2
  exit "${EXIT_USAGE}"
}

wait_seconds="${FLANFORGE_WAIT:-3600}"
poll_seconds="${FLANFORGE_POLL:-30}"
for value in "${wait_seconds}" "${poll_seconds}"; do
  [[ "${value}" =~ ^[0-9]+$ ]] || {
    echo "error: FLANFORGE_WAIT and FLANFORGE_POLL must be whole seconds" >&2
    exit "${EXIT_USAGE}"
  }
done
[ "${poll_seconds}" -gt 0 ] || {
  echo "error: FLANFORGE_POLL must be greater than zero" >&2
  exit "${EXIT_USAGE}"
}

deadline=$(( SECONDS + wait_seconds ))
attempt=0
while :; do
  attempt=$(( attempt + 1 ))
  set +e
  bash "${allocate}" allocate "$@"
  status=$?
  set -e
  # Only busy is worth retrying: a policy rejection or a usage error will
  # answer the same way forever, and a transport failure needs a human.
  if [ "${status}" != "${EXIT_BUSY}" ]; then
    exit "${status}"
  fi
  remaining=$(( deadline - SECONDS ))
  if [ "${remaining}" -le 0 ]; then
    echo "error: capacity did not free within ${wait_seconds}s (${attempt} attempts)" >&2
    exit "${EXIT_BUSY}"
  fi
  sleep_for="${poll_seconds}"
  [ "${sleep_for}" -le "${remaining}" ] || sleep_for="${remaining}"
  echo "busy; retrying in ${sleep_for}s (${remaining}s of budget left)" >&2
  sleep "${sleep_for}"
done
