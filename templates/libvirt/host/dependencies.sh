#!/usr/bin/env bash
# Derive generic guest routes from optional template-build conveniences.
# shellcheck disable=SC2154 # The shared derivation owns route output globals.

dependency_helper_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../shared/dependencies/routes.sh
source "${dependency_helper_dir}/../../shared/dependencies/routes.sh"

dependency_container_registry_url=""

derive_dependency_routes() {
  local services_root_domain="$1"
  local container_registry_mirror="$2"

  derive_dependency_proxy_routes "${services_root_domain}" \
    || die "SERVICES_ROOT_DOMAIN must be a valid DNS name"
  ensure_registry_location "${container_registry_mirror}"

  dependency_container_registry_url=""
  if [ -n "${container_registry_mirror}" ]; then
    dependency_container_registry_url="https://${container_registry_mirror}"
  fi

  local route
  for route in "${dependency_proxy_url}" "${dependency_cargo_index_url}" \
    "${dependency_npm_registry_url}" "${dependency_python_index_url}" \
    "${dependency_pytorch_index_url}" "${dependency_maven_repository_url}" \
    "${dependency_gradle_plugins_url}" \
    "${dependency_google_maven_repository_url}" \
    "${dependency_container_registry_url}"; do
    [ -z "${route}" ] || ensure_https_url "dependency route" "${route}"
  done
}
