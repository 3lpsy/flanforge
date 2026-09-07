#!/usr/bin/env bash
# Exercise shared hostname and registry validation with hostile fixtures.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../host/common.sh
source "${template_dir}/host/common.sh"

for valid_domain in example.invalid deps.example.invalid localhost 127.0.0.1; do
  is_valid_domain "${valid_domain}" \
    || die "valid domain was rejected: ${valid_domain}"
done

long_label="$(printf 'a%.0s' {1..64})"
long_domain="$(printf 'abcd%.0s' {1..64})"
for invalid_domain in .example.invalid example.invalid. example..invalid \
  -example.invalid example-.invalid bad_name.invalid "${long_label}.invalid" \
  "${long_domain}"; do
  if is_valid_domain "${invalid_domain}"; then
    die "invalid domain was accepted: ${invalid_domain}"
  fi
done

for valid_url in https://example.invalid \
  https://api.example.invalid:65535/releases/latest; do
  (ensure_https_url fixture "${valid_url}") \
    || die "valid HTTPS URL was rejected: ${valid_url}"
done
for invalid_url in https://example..invalid/path https://example.invalid:0/path \
  https://example.invalid:65536/path https://user@example.invalid/path \
  https://example.invalid/path//nested https://example.invalid/path/../private; do
  if (ensure_https_url fixture "${invalid_url}") >/dev/null 2>&1; then
    die "invalid HTTPS URL was accepted: ${invalid_url}"
  fi
done

for valid_registry in registry.example.invalid \
  registry.example.invalid:65535 \
  registry.example.invalid:5000/cache/docker_hub; do
  (ensure_registry_location "${valid_registry}") \
    || die "valid registry was rejected: ${valid_registry}"
done

for invalid_registry in https://registry.example.invalid \
  registry..example.invalid/cache registry.example.invalid:0 \
  registry.example.invalid:65536 registry.example.invalid:notaport \
  registry.example.invalid:/cache registry.example.invalid/cache//nested \
  registry.example.invalid/cache/../private registry.example.invalid/cache/ \
  registry.example.invalid/cache/'bad value'; do
  if (ensure_registry_location "${invalid_registry}") >/dev/null 2>&1; then
    die "invalid registry was accepted: ${invalid_registry}"
  fi
done

for valid_key in "12345678" "$(printf 'k%.0s' {1..512})" 'opaque-key.with~punct'; do
  is_preauth_key_shaped "${valid_key}" \
    || die "valid pre-auth key shape was rejected"
done
for invalid_key in "" "short7" "$(printf 'k%.0s' {1..513})" \
  'key with spaces' "$(printf 'key\twith\ttabs')" "$(printf 'key\nwith\nnewline')"; do
  if is_preauth_key_shaped "${invalid_key}"; then
    die "invalid pre-auth key shape was accepted"
  fi
done

for valid_label in a a-b flanforge-linux-ci "$(printf 'a%.0s' {1..63})"; do
  is_valid_host_label "${valid_label}" \
    || die "valid host label was rejected: ${valid_label}"
done
for invalid_label in "" -leading trailing- has.dot has_underscore \
  "$(printf 'a%.0s' {1..64})"; do
  if is_valid_host_label "${invalid_label}"; then
    die "invalid host label was accepted: ${invalid_label}"
  fi
done

echo "common validation fake tests passed"
