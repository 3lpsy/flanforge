#!/bin/bash
# Optionally persist deployment-specific dependency proxy routing for every
# runner shell. Tailscale has a separate provisioner and lifecycle.
# shellcheck disable=SC2154 # Sourced dependency derivation owns route outputs.
set -euo pipefail

brew_binary="${FLANFORGE_BREW_BINARY:-/opt/homebrew/bin/brew}"
[[ "${brew_binary}" = /* ]] && [ -x "${brew_binary}" ] \
  || { echo "invalid Homebrew binary" >&2; exit 1; }
eval "$("${brew_binary}" shellenv)"
export HOMEBREW_NO_AUTO_UPDATE=1

domain="${SERVICES_ROOT_DOMAIN:-}"
route_helper="${FLANFORGE_DEPENDENCY_ROUTE_HELPER:-/tmp/flanforge-dependency-routes.sh}"
gradle_init_file="${FLANFORGE_GRADLE_INIT_FILE:-/tmp/flanforge-chilled-proxy.init.gradle}"
[[ "${route_helper}" =~ ^/tmp/flanforge-[A-Za-z0-9._-]{1,96}$ \
  && "${gradle_init_file}" =~ ^/tmp/flanforge-[A-Za-z0-9._-]{1,96}$ ]] \
  || { echo "dependency helper paths are unsafe" >&2; exit 1; }
[ -f "${route_helper}" ] && [ ! -L "${route_helper}" ] \
  || { echo "missing dependency-route helper" >&2; exit 1; }
[ -f "${gradle_init_file}" ] && [ ! -L "${gradle_init_file}" ] \
  || { echo "missing Gradle init script" >&2; exit 1; }
# shellcheck source=../../shared/dependencies/routes.sh
source "${route_helper}"
derive_dependency_proxy_routes "${domain}" \
  || { echo "invalid SERVICES_ROOT_DOMAIN" >&2; exit 1; }
if [ -z "${domain}" ]; then
  rm -f "${route_helper}" "${gradle_init_file}"
  echo "==> dependency routing not baked"
  exit 0
fi

runner_home=/Users/runner
sudo mkdir -p "${runner_home}/.config/flanforge" "${runner_home}/.cargo" \
  "${runner_home}/.gradle/init.d" "${runner_home}/.m2" \
  /etc/gradle/init.d /etc/uv

route_file="${runner_home}/.config/flanforge/dependency-routing.zsh"
sudo tee "${route_file}" >/dev/null <<EOF
export BUN_CONFIG_REGISTRY=${dependency_npm_registry_url}
export CARGO_REGISTRIES_CHILLED_PROXY_INDEX=sparse+${dependency_cargo_index_url}
export CHILLED_PROXY_URL=${dependency_proxy_url}
export FLANFORGE_PYTORCH_INDEX_URL=${dependency_pytorch_index_url}
export GRADLE_USER_HOME=${runner_home}/.gradle
export PIP_INDEX_URL=${dependency_python_index_url}
export PYTORCH_REGISTRY=${dependency_pytorch_index_url}
export UV_DEFAULT_INDEX=${dependency_python_index_url}
export npm_config_registry=${dependency_npm_registry_url}
EOF

sudo tee "${runner_home}/.cargo/config.toml" >/dev/null <<EOF
[source.crates-io]
replace-with = "chilled-proxy"

[source.chilled-proxy]
registry = "sparse+${dependency_cargo_index_url}"
EOF

sudo tee "${runner_home}/.npmrc" >/dev/null <<EOF
registry=${dependency_npm_registry_url}
EOF

sudo tee /etc/pip.conf >/dev/null <<EOF
[global]
index-url = ${dependency_python_index_url}
EOF

sudo tee /etc/uv/uv.toml >/dev/null <<EOF
[[index]]
url = "${dependency_python_index_url}"
default = true
EOF

sudo tee "${runner_home}/.m2/settings.xml" >/dev/null <<EOF
<settings xmlns="http://maven.apache.org/SETTINGS/1.0.0">
  <mirrors>
    <mirror>
      <id>chilled-proxy-central</id>
      <url>${dependency_maven_repository_url}</url>
      <mirrorOf>central</mirrorOf>
    </mirror>
    <mirror>
      <id>chilled-proxy-google</id>
      <url>${dependency_google_maven_repository_url}</url>
      <mirrorOf>google</mirrorOf>
    </mirror>
  </mirrors>
</settings>
EOF

sudo tee /etc/gradle/gradle.properties >/dev/null <<EOF
systemProp.chilled.proxy.url=${dependency_proxy_url}
EOF
sudo cp /etc/gradle/gradle.properties "${runner_home}/.gradle/gradle.properties"
sudo install -m 0644 -o root -g wheel "${gradle_init_file}" \
  /etc/gradle/init.d/chilled-proxy.init.gradle
sudo install -m 0600 -o runner -g staff "${gradle_init_file}" \
  "${runner_home}/.gradle/init.d/chilled-proxy.init.gradle"

source_line='source "$HOME/.config/flanforge/dependency-routing.zsh"'
zshenv_tmp="$(mktemp /tmp/flanforge-runner-zshenv.XXXXXX)"
{
  printf '%s\n' "${source_line}"
  sudo grep -Fvx "${source_line}" "${runner_home}/.zshenv" 2>/dev/null || true
} | sudo tee "${zshenv_tmp}" >/dev/null
sudo install -m 0600 -o runner -g staff "${zshenv_tmp}" \
  "${runner_home}/.zshenv"
sudo rm -f "${zshenv_tmp}" "${route_helper}" "${gradle_init_file}"

sudo chown -R runner:staff \
  "${runner_home}/.config" "${runner_home}/.cargo" \
  "${runner_home}/.gradle" "${runner_home}/.m2" \
  "${runner_home}/.npmrc" "${runner_home}/.zshenv"
sudo chmod 0600 "${route_file}" "${runner_home}/.cargo/config.toml" \
  "${runner_home}/.npmrc" "${runner_home}/.m2/settings.xml" \
  "${runner_home}/.gradle/gradle.properties"
