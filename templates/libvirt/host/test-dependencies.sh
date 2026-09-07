#!/usr/bin/env bash
# Derive private test routes and build the exact guest-side contract probe.
# shellcheck disable=SC2154 # Shared route derivation owns these outputs.

test_dependency_helper_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=dependencies.sh
source "${test_dependency_helper_dir}/dependencies.sh"

prepare_dependency_contract() {
  local manifest_file="$1"
  local services_root_domain="$2"
  local container_registry_mirror="$3"
  local expected_configured=false
  local manifest_configured

  derive_dependency_routes "${services_root_domain}" \
    "${container_registry_mirror}"
  [ -z "${dependency_proxy_url}" ] \
    && [ -z "${dependency_container_registry_url}" ] \
    || expected_configured=true
  manifest_configured="$(jq -er '
    .provenance.dependency_proxy.configured |
    if type == "boolean" then tostring
    else error("dependency configured state must be boolean") end
  ' "${manifest_file}")"
  [ "${manifest_configured}" = "${expected_configured}" ] \
    || die "template dependency inputs do not match the image manifest"

  manifest_dependency_proxy_url="${dependency_proxy_url}"
  manifest_cargo_route="${dependency_cargo_index_url}"
  manifest_npm_route="${dependency_npm_registry_url}"
  manifest_python_route="${dependency_python_index_url}"
  manifest_pytorch_route="${dependency_pytorch_index_url}"
  manifest_maven_route="${dependency_maven_repository_url}"
  manifest_gradle_plugins_route="${dependency_gradle_plugins_url}"
  manifest_google_maven_route="${dependency_google_maven_repository_url}"
  manifest_container_registry_route="${dependency_container_registry_url}"
  manifest_dependency_ca_sha256="$(jq -er \
    '.provenance.dependency_proxy.ca_sha256 // ""' "${manifest_file}")"
}

append_remote_assignment() {
  local name="$1"
  local value="$2"
  local quoted
  printf -v quoted '%q' "${value}"
  remote_dependency_contract+="${name}=${quoted};"
}

build_remote_dependency_contract() {
  remote_dependency_contract='set -Eeuo pipefail;'
  append_remote_assignment cargo "${manifest_cargo_route}"
  append_remote_assignment npm "${manifest_npm_route}"
  append_remote_assignment python "${manifest_python_route}"
  append_remote_assignment pytorch "${manifest_pytorch_route}"
  append_remote_assignment maven "${manifest_maven_route}"
  append_remote_assignment gradle_plugins "${manifest_gradle_plugins_route}"
  append_remote_assignment google_maven "${manifest_google_maven_route}"
  append_remote_assignment dependency_proxy "${manifest_dependency_proxy_url}"
  append_remote_assignment container_registry \
    "${manifest_container_registry_route}"
  append_remote_assignment dependency_ca_sha256 \
    "${manifest_dependency_ca_sha256}"
  remote_dependency_contract+='if [ -n "${cargo}" ]; then
  grep -Fqx '\''replace-with = "chilled-proxy"'\'' "$HOME/.cargo/config.toml";
  grep -Fqx "registry = \"sparse+${cargo}\"" "$HOME/.cargo/config.toml";
  grep -Fqx "registry=${npm}" "$HOME/.npmrc";
  grep -Fqx "index-url = ${python}" /etc/pip.conf;
  grep -Fqx "url = \"${python}\"" /etc/uv/uv.toml;
  grep -Fqx "      <url>${maven}</url>" "$HOME/.m2/settings.xml";
  grep -Fqx '\''      <mirrorOf>central</mirrorOf>'\'' "$HOME/.m2/settings.xml";
  grep -Fqx "      <url>${google_maven}</url>" "$HOME/.m2/settings.xml";
  grep -Fqx '\''      <mirrorOf>google</mirrorOf>'\'' "$HOME/.m2/settings.xml";
  ! grep -Eq '\''<mirrorOf>[[:space:]]*\*[[:space:]]*</mirrorOf>'\'' "$HOME/.m2/settings.xml";
  test -s /etc/gradle/init.d/chilled-proxy.init.gradle;
  cmp /etc/gradle/init.d/chilled-proxy.init.gradle "$HOME/.gradle/init.d/chilled-proxy.init.gradle";
  grep -Fqx "systemProp.chilled.proxy.url=${dependency_proxy}" "$HOME/.gradle/gradle.properties";
  test "${BUN_CONFIG_REGISTRY}" = "${npm}";
  test "${CARGO_REGISTRIES_CHILLED_PROXY_INDEX}" = "sparse+${cargo}";
  test "${PIP_INDEX_URL}" = "${python}";
  test "${UV_DEFAULT_INDEX}" = "${python}";
  test "${PYTORCH_REGISTRY}" = "${pytorch}";
  test "${FLANFORGE_PYTORCH_INDEX_URL}" = "${pytorch}";
  test "${CHILLED_PROXY_URL}" = "${dependency_proxy}";
  test "${GRADLE_USER_HOME}" = "$HOME/.gradle";
else
  test ! -e "$HOME/.config/flanforge/dependency-routing.sh";
  test ! -e "$HOME/.gradle/init.d/chilled-proxy.init.gradle";
  test ! -e "$HOME/.m2/settings.xml";
  test ! -e /etc/gradle/init.d/chilled-proxy.init.gradle;
fi;
if [ -n "${container_registry}" ]; then
  registry_location="${container_registry#https://}";
  grep -Fqx "location = \"${registry_location}\"" /etc/containers/registries.conf.d/50-flanforge-mirror.conf;
else
  test ! -e /etc/containers/registries.conf.d/50-flanforge-mirror.conf;
fi;
if [ -n "${dependency_ca_sha256}" ]; then
  printf "%s  %s\n" "${dependency_ca_sha256}" /etc/pki/ca-trust/source/anchors/flanforge-dependency-proxy.crt | sha256sum -c -;
else
  test ! -e /etc/pki/ca-trust/source/anchors/flanforge-dependency-proxy.crt;
fi'
}
