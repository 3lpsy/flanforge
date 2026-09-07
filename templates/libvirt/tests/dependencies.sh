#!/usr/bin/env bash
# Exercise dependency-route derivation without building a guest.
# shellcheck disable=SC2154 # Sourced dependency derivation owns route outputs.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../host/common.sh
source "${template_dir}/host/common.sh"
# shellcheck source=../host/dependencies.sh
source "${template_dir}/host/dependencies.sh"
# shellcheck source=../host/test-dependencies.sh
source "${template_dir}/host/test-dependencies.sh"
# shellcheck source=../../shared/dependencies/checksums.sh
source "${template_dir}/../shared/dependencies/checksums.sh"

derive_dependency_routes "" ""
for route in "${dependency_proxy_url}" "${dependency_cargo_index_url}" \
  "${dependency_npm_registry_url}" "${dependency_python_index_url}" \
  "${dependency_pytorch_index_url}" "${dependency_maven_repository_url}" \
  "${dependency_gradle_plugins_url}" \
  "${dependency_google_maven_repository_url}" \
  "${dependency_container_registry_url}"; do
  [ -z "${route}" ] || die "unset dependency routing emitted a route"
done

derive_dependency_routes proxy.example.invalid \
  registry.example.invalid:5000/dockerhub
[ "${dependency_proxy_url}" = https://deps.proxy.example.invalid ]
[ "${dependency_cargo_index_url}" = \
  https://deps.proxy.example.invalid/crates/index/ ]
[ "${dependency_npm_registry_url}" = \
  https://deps.proxy.example.invalid/npm/ ]
[ "${dependency_python_index_url}" = \
  https://deps.proxy.example.invalid/pypi/simple/ ]
[ "${dependency_pytorch_index_url}" = \
  https://deps.proxy.example.invalid/pytorch/simple/ ]
[ "${dependency_maven_repository_url}" = \
  https://deps.proxy.example.invalid/maven ]
[ "${dependency_gradle_plugins_url}" = \
  https://deps.proxy.example.invalid/gradle-plugins ]
[ "${dependency_google_maven_repository_url}" = \
  https://deps.proxy.example.invalid/google-maven ]
[ "${dependency_container_registry_url}" = \
  https://registry.example.invalid:5000/dockerhub ]

contract_manifest="$(mktemp "${TMPDIR:-/tmp}/flanforge-dependency-manifest.XXXXXX")"
trap 'rm -f "${contract_manifest}" "${contract_manifest}.mismatch"' EXIT
jq -n '{provenance: {dependency_proxy: {
  configured: true,
  ca_sha256: null
}}}' > "${contract_manifest}"
prepare_dependency_contract "${contract_manifest}" proxy.example.invalid \
  registry.example.invalid:5000/dockerhub
[ "${manifest_cargo_route}" = \
  https://deps.proxy.example.invalid/crates/index/ ]
[ "${manifest_container_registry_route}" = \
  https://registry.example.invalid:5000/dockerhub ]
jq '.provenance.dependency_proxy.configured = false' "${contract_manifest}" \
  > "${contract_manifest}.mismatch"
if (prepare_dependency_contract "${contract_manifest}.mismatch" \
  proxy.example.invalid registry.example.invalid:5000/dockerhub) \
  >/dev/null 2>&1; then
  die "manifest accepted mismatched private template inputs"
fi
prepare_dependency_contract "${contract_manifest}.mismatch" "" ""
[ -z "${manifest_cargo_route}" ]
[ -z "${manifest_container_registry_route}" ]
rm -f "${contract_manifest}.mismatch"

long_label="$(printf 'a%.0s' {1..64})"
oversized_root="$(printf 'a%.0s' {1..61}).$(printf 'b%.0s' {1..61}).$(printf 'c%.0s' {1..61}).$(printf 'd%.0s' {1..61}).a"
for invalid_domain in .example.invalid example.invalid. example..invalid \
  -example.invalid example-.invalid bad_name.invalid "${long_label}.invalid" \
  "${oversized_root}" 'example.invalid;touch /tmp/not-allowed'; do
  if (derive_dependency_routes "${invalid_domain}" "") >/dev/null 2>&1; then
    die "invalid root domain was accepted: ${invalid_domain}"
  fi
done

printf '%s  %s\n' "${CHILLED_PROXY_GRADLE_SHA256}" \
  "${template_dir}/../shared/dependencies/chilled-proxy.init.gradle" \
  | sha256sum -c -

grep -Fq '[source.crates-io]' \
  "${template_dir}/scripts/provision-dependencies.sh"
grep -Fq 'replace-with = "chilled-proxy"' \
  "${template_dir}/scripts/provision-dependencies.sh"
grep -Fq '<mirrorOf>central</mirrorOf>' \
  "${template_dir}/scripts/provision-dependencies.sh"
grep -Fq '<mirrorOf>google</mirrorOf>' \
  "${template_dir}/scripts/provision-dependencies.sh"
if grep -Eq '<mirrorOf>[[:space:]]*\*[[:space:]]*</mirrorOf>' \
  "${template_dir}/scripts/provision-dependencies.sh"; then
  die "Maven configuration contains a wildcard mirror"
fi
grep -Fq 'export BUN_CONFIG_REGISTRY=' \
  "${template_dir}/scripts/provision-dependencies.sh"
grep -Fq 'export UV_DEFAULT_INDEX=' \
  "${template_dir}/scripts/provision-dependencies.sh"
grep -Fq 'export PYTORCH_REGISTRY=' \
  "${template_dir}/scripts/provision-dependencies.sh"

echo "dependency routing fake tests passed"
