#!/usr/bin/env bash
# Optionally bake dependency proxies and their private CA into the guest.
set -Eeuo pipefail

trap 'echo "dependency provisioning failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

runner_home=/home/runner
dependency_proxy_url="${CHILLED_PROXY_URL:-}"
cargo_index_url="${CARGO_INDEX_URL:-}"
npm_registry_url="${NPM_REGISTRY_URL:-}"
python_index_url="${PYTHON_INDEX_URL:-}"
pytorch_index_url="${PYTORCH_INDEX_URL:-}"
maven_repository_url="${MAVEN_REPOSITORY_URL:-}"
google_maven_repository_url="${GOOGLE_MAVEN_REPOSITORY_URL:-}"
registry_mirror="${CONTAINER_REGISTRY_MIRROR:-}"
ca_file="${FLANFORGE_DEPENDENCY_CA_STAGING_FILE:-/tmp/flanforge-dependency-ca.pem}"
gradle_init_file="${FLANFORGE_GRADLE_INIT_FILE:-/tmp/flanforge-chilled-proxy.init.gradle}"
[[ "${ca_file}" =~ ^/tmp/flanforge-[A-Za-z0-9._-]{1,96}$ ]] \
  && [ -f "${ca_file}" ] && [ ! -L "${ca_file}" ] \
  || { echo "invalid dependency CA staging file" >&2; exit 1; }
[[ "${gradle_init_file}" =~ ^/tmp/flanforge-[A-Za-z0-9._-]{1,96}$ ]] \
  && [ -f "${gradle_init_file}" ] \
  && [ ! -L "${gradle_init_file}" ] \
  || { echo "invalid Gradle init script" >&2; exit 1; }

is_valid_https_route() {
  local route="$1"
  local remainder
  local authority
  local host
  local port=""
  local label
  local -a labels

  [ "${#route}" -le 2048 ] && [[ "${route}" == https://* ]] || return 1
  remainder="${route#https://}"
  authority="${remainder%%/*}"
  [[ "${authority}" != *@* && "${authority}" != *:*:* ]] || return 1
  host="${authority%%:*}"
  if [[ "${authority}" == *:* ]]; then
    port="${authority##*:}"
    [[ "${port}" =~ ^[0-9]{1,5}$ ]] || return 1
    [ "$((10#${port}))" -ge 1 ] && [ "$((10#${port}))" -le 65535 ] || return 1
  fi
  [ -n "${host}" ] && [ "${#host}" -le 253 ] \
    && [[ "${host}" != .* && "${host}" != *. && "${host}" != *..* ]] || return 1
  IFS=. read -r -a labels <<< "${host}"
  for label in "${labels[@]}"; do
    [ "${#label}" -le 63 ] \
      && [[ "${label}" =~ ^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$ ]] \
      || return 1
  done
  [[ "${remainder}" =~ ^[A-Za-z0-9.-]+(:[0-9]{1,5})?(/[A-Za-z0-9._~/%+-]+)*/?$ ]]
}

if [ -s "${ca_file}" ]; then
  openssl x509 -in "${ca_file}" -noout >/dev/null
  sudo install -m 0644 -o root -g root "${ca_file}" \
    /etc/pki/ca-trust/source/anchors/flanforge-dependency-proxy.crt
  sudo update-ca-trust
fi
sudo rm -f "${ca_file}"

route_count=0
for route in "${dependency_proxy_url}" "${cargo_index_url}" \
  "${npm_registry_url}" "${python_index_url}" "${pytorch_index_url}" \
  "${maven_repository_url}" "${google_maven_repository_url}"; do
  if [ -n "${route}" ]; then
    route_count=$((route_count + 1))
    is_valid_https_route "${route}" \
      || { echo "invalid dependency route" >&2; exit 1; }
  fi
done
[ "${route_count}" -eq 0 ] || [ "${route_count}" -eq 7 ] \
  || { echo "all language dependency routes must be configured together" >&2; exit 1; }

if [ "${route_count}" -eq 7 ]; then
  [ "${cargo_index_url}" = "${dependency_proxy_url}/crates/index/" ]
  [ "${npm_registry_url}" = "${dependency_proxy_url}/npm/" ]
  [ "${python_index_url}" = "${dependency_proxy_url}/pypi/simple/" ]
  [ "${pytorch_index_url}" = "${dependency_proxy_url}/pytorch/simple/" ]
  [ "${maven_repository_url}" = "${dependency_proxy_url}/maven" ]
  [ "${google_maven_repository_url}" = "${dependency_proxy_url}/google-maven" ]
  [ -s "${gradle_init_file}" ]

  sudo install -d -m 0700 -o runner -g runner \
    "${runner_home}/.cargo" "${runner_home}/.config/flanforge" \
    "${runner_home}/.gradle/init.d" "${runner_home}/.m2"
  sudo install -d -m 0755 /etc/gradle/init.d /etc/uv

  sudo tee "${runner_home}/.cargo/config.toml" >/dev/null <<EOF
[source.crates-io]
replace-with = "chilled-proxy"

[source.chilled-proxy]
registry = "sparse+${cargo_index_url}"
EOF

  sudo tee /etc/pip.conf >/dev/null <<EOF
[global]
index-url = ${python_index_url}
EOF

  sudo tee /etc/uv/uv.toml >/dev/null <<EOF
[[index]]
url = "${python_index_url}"
default = true
EOF

  sudo tee "${runner_home}/.npmrc" >/dev/null <<EOF
registry=${npm_registry_url}
EOF


  sudo tee "${runner_home}/.m2/settings.xml" >/dev/null <<EOF
<settings xmlns="http://maven.apache.org/SETTINGS/1.0.0">
  <mirrors>
    <mirror>
      <id>chilled-proxy-central</id>
      <name>chilled-proxy cooldown gate (Maven Central)</name>
      <url>${maven_repository_url}</url>
      <mirrorOf>central</mirrorOf>
    </mirror>
    <mirror>
      <id>chilled-proxy-google</id>
      <name>chilled-proxy cooldown gate (Google Maven)</name>
      <url>${google_maven_repository_url}</url>
      <mirrorOf>google</mirrorOf>
    </mirror>
  </mirrors>
</settings>
EOF

  sudo tee /etc/gradle/gradle.properties >/dev/null <<EOF
systemProp.chilled.proxy.url=${dependency_proxy_url}
EOF
  sudo cp /etc/gradle/gradle.properties "${runner_home}/.gradle/gradle.properties"
  sudo install -m 0644 -o root -g root "${gradle_init_file}" \
    /etc/gradle/init.d/chilled-proxy.init.gradle
  sudo install -m 0600 -o runner -g runner "${gradle_init_file}" \
    "${runner_home}/.gradle/init.d/chilled-proxy.init.gradle"

  sudo tee "${runner_home}/.config/flanforge/dependency-routing.sh" >/dev/null <<EOF
export BUN_CONFIG_REGISTRY=${npm_registry_url}
export CARGO_REGISTRIES_CHILLED_PROXY_INDEX=sparse+${cargo_index_url}
export CHILLED_PROXY_URL=${dependency_proxy_url}
export FLANFORGE_PYTORCH_INDEX_URL=${pytorch_index_url}
export GRADLE_USER_HOME=${runner_home}/.gradle
export PIP_INDEX_URL=${python_index_url}
export PYTORCH_REGISTRY=${pytorch_index_url}
export UV_DEFAULT_INDEX=${python_index_url}
export npm_config_registry=${npm_registry_url}
EOF

  source_line='source "$HOME/.config/flanforge/dependency-routing.sh"'
  for shell_file in .bash_profile .bashrc; do
    # The staging file belongs to the build user, so it is written without
    # sudo: fs.protected_regular denies root an O_CREAT open of another
    # user's file under sticky /tmp. Only the 0600 source needs sudo to read.
    shell_file_tmp="$(mktemp "/tmp/flanforge-runner-${shell_file#.}.XXXXXX")"
    {
      printf '%s\n' "${source_line}"
      sudo grep -Fvx "${source_line}" \
        "${runner_home}/${shell_file}" 2>/dev/null || true
    } > "${shell_file_tmp}"
    sudo install -m 0600 -o runner -g runner "${shell_file_tmp}" \
      "${runner_home}/${shell_file}"
    rm -f "${shell_file_tmp}"
  done
fi
sudo rm -f "${gradle_init_file}"

if [ -n "${registry_mirror}" ]; then
  [[ "${registry_mirror}" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]{0,253}[A-Za-z0-9])?(:[0-9]{1,5})?(/[A-Za-z0-9._/-]+)?$ ]] \
    && is_valid_https_route "https://${registry_mirror}" \
    || { echo "invalid container registry mirror" >&2; exit 1; }
  sudo install -d -m 0755 /etc/containers/registries.conf.d
  sudo tee /etc/containers/registries.conf.d/50-flanforge-mirror.conf >/dev/null <<EOF
[[registry]]
location = "docker.io"

[[registry.mirror]]
location = "${registry_mirror}"
insecure = false
EOF
fi

sudo chown -R runner:runner "${runner_home}"
sudo chmod 0700 "${runner_home}"
sudo find "${runner_home}" -type f -exec chmod go-w {} +

echo "==> dependency proxy configuration complete"
