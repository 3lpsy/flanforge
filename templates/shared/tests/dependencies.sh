#!/usr/bin/env bash
# Keep the declared dependency-configuration digest coupled to its asset.
set -Eeuo pipefail

shared_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../dependencies/checksums.sh
source "${shared_dir}/dependencies/checksums.sh"

[[ "${CHILLED_PROXY_GRADLE_SHA256}" =~ ^[a-f0-9]{64}$ ]] \
  || { echo "invalid shared Gradle checksum" >&2; exit 1; }
printf '%s  %s\n' "${CHILLED_PROXY_GRADLE_SHA256}" \
  "${shared_dir}/dependencies/chilled-proxy.init.gradle" | sha256sum -c -

echo "shared dependency checksum test passed"
