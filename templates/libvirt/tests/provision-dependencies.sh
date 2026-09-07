#!/usr/bin/env bash
# Render Linux dependency files through a fake privilege boundary.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-provision-deps.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-provision-deps."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

fake_bin="${test_root}/bin"
mkdir -p "${fake_bin}"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in -H) shift ;; -u) shift 2 ;; *) break ;; esac' \
  'done' \
  'command_name="$1"; shift' \
  'map_path() {' \
  '  case "$1" in' \
  '    /etc/*|/home/*|/root/*|/usr/*|/var/*) printf "%s%s" "${FAKE_GUEST_ROOT}" "$1" ;;' \
  '    *) printf "%s" "$1" ;;' \
  '  esac' \
  '}' \
  'case "${command_name}" in' \
  '  chown|update-ca-trust) exit 0 ;;' \
  '  install)' \
  '    arguments=()' \
  '    while [ "$#" -gt 0 ]; do' \
  '      case "$1" in' \
  '        -o|-g) shift 2 ;;' \
  '        /etc/*|/home/*|/root/*|/usr/*|/var/*) arguments+=("$(map_path "$1")"); shift ;;' \
  '        *) arguments+=("$1"); shift ;;' \
  '      esac' \
  '    done' \
  '    exec /usr/bin/install "${arguments[@]}" ;;' \
  '  tee|cp|rm|chmod|grep|find)' \
  '    arguments=()' \
  '    for argument in "$@"; do arguments+=("$(map_path "${argument}")"); done' \
  '    exec "$(command -v "${command_name}")" "${arguments[@]}" ;;' \
  '  *) echo "unexpected sudo command: ${command_name}" >&2; exit 2 ;;' \
  'esac' > "${fake_bin}/sudo"
chmod 0700 "${fake_bin}/sudo"

run_fixture() {
  local fixture="$1"
  local guest_root="${test_root}/${fixture}"
  local ca_source
  local gradle_source
  mkdir -p "${guest_root}/home/runner"
  ca_source="$(mktemp /tmp/flanforge-linux-ca.XXXXXX)"
  gradle_source="$(mktemp /tmp/flanforge-linux-gradle.XXXXXX)"
  cp "${template_dir}/../shared/dependencies/chilled-proxy.init.gradle" \
    "${gradle_source}"

  if [ "${fixture}" = configured ]; then
    env PATH="${fake_bin}:${PATH}" FAKE_GUEST_ROOT="${guest_root}" \
      FLANFORGE_GRADLE_INIT_FILE="${gradle_source}" \
      FLANFORGE_DEPENDENCY_CA_STAGING_FILE="${ca_source}" \
      CHILLED_PROXY_URL=https://deps.example.invalid \
      CARGO_INDEX_URL=https://deps.example.invalid/crates/index/ \
      NPM_REGISTRY_URL=https://deps.example.invalid/npm/ \
      PYTHON_INDEX_URL=https://deps.example.invalid/pypi/simple/ \
      PYTORCH_INDEX_URL=https://deps.example.invalid/pytorch/simple/ \
      MAVEN_REPOSITORY_URL=https://deps.example.invalid/maven \
      GOOGLE_MAVEN_REPOSITORY_URL=https://deps.example.invalid/google-maven \
      CONTAINER_REGISTRY_MIRROR=registry.example.invalid/dockerhub \
      bash "${template_dir}/scripts/provision-dependencies.sh" >/dev/null
  else
    env -u CHILLED_PROXY_URL -u CARGO_INDEX_URL -u NPM_REGISTRY_URL \
      -u PYTHON_INDEX_URL -u PYTORCH_INDEX_URL -u MAVEN_REPOSITORY_URL \
      -u GOOGLE_MAVEN_REPOSITORY_URL -u CONTAINER_REGISTRY_MIRROR \
      PATH="${fake_bin}:${PATH}" FAKE_GUEST_ROOT="${guest_root}" \
      FLANFORGE_GRADLE_INIT_FILE="${gradle_source}" \
      FLANFORGE_DEPENDENCY_CA_STAGING_FILE="${ca_source}" \
      bash "${template_dir}/scripts/provision-dependencies.sh" >/dev/null
  fi
}

run_fixture configured
configured_root="${test_root}/configured"
grep -Fqx 'replace-with = "chilled-proxy"' \
  "${configured_root}/home/runner/.cargo/config.toml"
grep -Fqx 'registry=https://deps.example.invalid/npm/' \
  "${configured_root}/home/runner/.npmrc"
grep -Fqx 'index-url = https://deps.example.invalid/pypi/simple/' \
  "${configured_root}/etc/pip.conf"
grep -Fqx 'url = "https://deps.example.invalid/pypi/simple/"' \
  "${configured_root}/etc/uv/uv.toml"
grep -Fqx '      <mirrorOf>central</mirrorOf>' \
  "${configured_root}/home/runner/.m2/settings.xml"
grep -Fqx '      <mirrorOf>google</mirrorOf>' \
  "${configured_root}/home/runner/.m2/settings.xml"
[ "$(head -n 1 "${configured_root}/home/runner/.bashrc")" = \
  'source "$HOME/.config/flanforge/dependency-routing.sh"' ]
BASH_ENV="${configured_root}/home/runner/.bashrc" \
  HOME="${configured_root}/home/runner" bash -c \
  'test "${BUN_CONFIG_REGISTRY}" = https://deps.example.invalid/npm/ &&
   test "${UV_DEFAULT_INDEX}" = https://deps.example.invalid/pypi/simple/ &&
   test "${PYTORCH_REGISTRY}" = https://deps.example.invalid/pytorch/simple/'
cmp "${configured_root}/etc/gradle/init.d/chilled-proxy.init.gradle" \
  "${configured_root}/home/runner/.gradle/init.d/chilled-proxy.init.gradle"
grep -Fqx 'location = "registry.example.invalid/dockerhub"' \
  "${configured_root}/etc/containers/registries.conf.d/50-flanforge-mirror.conf"

run_fixture unset
unset_root="${test_root}/unset"
[ ! -e "${unset_root}/home/runner/.config/flanforge/dependency-routing.sh" ]
[ ! -e "${unset_root}/home/runner/.m2/settings.xml" ]
[ ! -e "${unset_root}/etc/gradle/init.d/chilled-proxy.init.gradle" ]

hostile_ca=/etc/passwd
hostile_gradle="$(mktemp /tmp/flanforge-linux-hostile.XXXXXX)"
if env PATH="${fake_bin}:${PATH}" FAKE_GUEST_ROOT="${test_root}/hostile" \
  FLANFORGE_DEPENDENCY_CA_STAGING_FILE="${hostile_ca}" \
  FLANFORGE_GRADLE_INIT_FILE="${hostile_gradle}" \
  bash "${template_dir}/scripts/provision-dependencies.sh" >/dev/null 2>&1; then
  echo "unsafe dependency staging path was accepted" >&2
  exit 1
fi
rm -f "${hostile_gradle}"

echo "Linux dependency provisioning fake tests passed"
