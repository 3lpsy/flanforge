#!/usr/bin/env bash
# Derive the fixed package routes from the optional template-build domain.
# shellcheck disable=SC2034 # Route outputs are consumed by sourcing callers.

dependency_proxy_url=""
dependency_cargo_index_url=""
dependency_npm_registry_url=""
dependency_python_index_url=""
dependency_pytorch_index_url=""
dependency_maven_repository_url=""
dependency_gradle_plugins_url=""
dependency_google_maven_repository_url=""

is_valid_dependency_domain() {
  local value="$1"
  local label
  local -a labels

  [ -n "${value}" ] && [ "${#value}" -le 248 ] || return 1
  [[ "${value}" != .* && "${value}" != *. && "${value}" != *..* ]] || return 1
  IFS=. read -r -a labels <<< "${value}"
  for label in "${labels[@]}"; do
    [ "${#label}" -le 63 ] \
      && [[ "${label}" =~ ^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$ ]] \
      || return 1
  done
}

derive_dependency_proxy_routes() {
  local services_root_domain="$1"

  dependency_proxy_url=""
  dependency_cargo_index_url=""
  dependency_npm_registry_url=""
  dependency_python_index_url=""
  dependency_pytorch_index_url=""
  dependency_maven_repository_url=""
  dependency_gradle_plugins_url=""
  dependency_google_maven_repository_url=""
  [ -z "${services_root_domain}" ] && return
  is_valid_dependency_domain "${services_root_domain}" || return 1

  dependency_proxy_url="https://deps.${services_root_domain}"
  dependency_cargo_index_url="${dependency_proxy_url}/crates/index/"
  dependency_npm_registry_url="${dependency_proxy_url}/npm/"
  dependency_python_index_url="${dependency_proxy_url}/pypi/simple/"
  dependency_pytorch_index_url="${dependency_proxy_url}/pytorch/simple/"
  dependency_maven_repository_url="${dependency_proxy_url}/maven"
  dependency_gradle_plugins_url="${dependency_proxy_url}/gradle-plugins"
  dependency_google_maven_repository_url="${dependency_proxy_url}/google-maven"
}
