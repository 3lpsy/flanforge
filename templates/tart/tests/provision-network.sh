#!/usr/bin/env bash
# Render Tart dependency files without starting a VM.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-tart-network.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-tart-network."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

fake_bin="${test_root}/bin"
mkdir -p "${fake_bin}"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "${fake_bin}/brew"
chmod 0700 "${fake_bin}/brew"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in -H) shift ;; -u) shift 2 ;; *) break ;; esac' \
  'done' \
  'command_name="$1"; shift' \
  'map_path() {' \
  '  case "$1" in' \
  '    /Users/*|/etc/*) printf "%s%s" "${FAKE_GUEST_ROOT}" "$1" ;;' \
  '    *) printf "%s" "$1" ;;' \
  '  esac' \
  '}' \
  'case "${command_name}" in' \
  '  chown) exit 0 ;;' \
  '  install)' \
  '    arguments=()' \
  '    while [ "$#" -gt 0 ]; do' \
  '      case "$1" in' \
  '        -o|-g) shift 2 ;;' \
  '        /Users/*|/etc/*) arguments+=("$(map_path "$1")"); shift ;;' \
  '        *) arguments+=("$1"); shift ;;' \
  '      esac' \
  '    done' \
  '    exec /usr/bin/install "${arguments[@]}" ;;' \
  '  mkdir|tee|cp|grep|rm|chmod)' \
  '    arguments=()' \
  '    for argument in "$@"; do arguments+=("$(map_path "${argument}")"); done' \
  '    exec "$(command -v "${command_name}")" "${arguments[@]}" ;;' \
  '  *) echo "unexpected sudo command: ${command_name}" >&2; exit 2 ;;' \
  'esac' > "${fake_bin}/sudo"
chmod 0700 "${fake_bin}/sudo"

run_fixture() {
  local fixture="$1"
  local domain="$2"
  local guest_root="${test_root}/${fixture}"
  local route_helper
  local gradle_source
  local status=0
  mkdir -p "${guest_root}/Users/runner"
  route_helper="$(mktemp /tmp/flanforge-tart-routes.XXXXXX)"
  gradle_source="$(mktemp /tmp/flanforge-tart-gradle.XXXXXX)"
  cp "${template_dir}/../shared/dependencies/routes.sh" "${route_helper}"
  cp "${template_dir}/../shared/dependencies/chilled-proxy.init.gradle" \
    "${gradle_source}"
  env PATH="${fake_bin}:${PATH}" FAKE_GUEST_ROOT="${guest_root}" \
    FLANFORGE_BREW_BINARY="${fake_bin}/brew" \
    FLANFORGE_DEPENDENCY_ROUTE_HELPER="${route_helper}" \
    FLANFORGE_GRADLE_INIT_FILE="${gradle_source}" \
    SERVICES_ROOT_DOMAIN="${domain}" \
    bash "${template_dir}/scripts/provision-network.sh" >/dev/null || status=$?
  rm -f "${route_helper}" "${gradle_source}"
  return "${status}"
}

run_fixture configured example.invalid
configured_root="${test_root}/configured"
grep -Fqx 'replace-with = "chilled-proxy"' \
  "${configured_root}/Users/runner/.cargo/config.toml"
grep -Fqx 'registry=https://deps.example.invalid/npm/' \
  "${configured_root}/Users/runner/.npmrc"
grep -Fqx 'index-url = https://deps.example.invalid/pypi/simple/' \
  "${configured_root}/etc/pip.conf"
grep -Fqx 'url = "https://deps.example.invalid/pypi/simple/"' \
  "${configured_root}/etc/uv/uv.toml"
if grep -q 'SERVICES_ROOT_DOMAIN' \
  "${configured_root}/Users/runner/.config/flanforge/dependency-routing.zsh"; then
  echo "configured Tart routing leaked its build-domain input" >&2
  exit 1
fi
grep -Fqx '      <mirrorOf>central</mirrorOf>' \
  "${configured_root}/Users/runner/.m2/settings.xml"
grep -Fqx '      <mirrorOf>google</mirrorOf>' \
  "${configured_root}/Users/runner/.m2/settings.xml"
[ "$(head -n 1 "${configured_root}/Users/runner/.zshenv")" = \
  'source "$HOME/.config/flanforge/dependency-routing.zsh"' ]
SERVICES_ROOT_DOMAIN="" HOME="${configured_root}/Users/runner" \
  bash -c '. "$HOME/.config/flanforge/dependency-routing.zsh";
    test "${BUN_CONFIG_REGISTRY}" = https://deps.example.invalid/npm/;
    test "${UV_DEFAULT_INDEX}" = https://deps.example.invalid/pypi/simple/;
    test "${PYTORCH_REGISTRY}" = https://deps.example.invalid/pytorch/simple/;
    test -z "${SERVICES_ROOT_DOMAIN:-}"'
cmp "${configured_root}/etc/gradle/init.d/chilled-proxy.init.gradle" \
  "${configured_root}/Users/runner/.gradle/init.d/chilled-proxy.init.gradle"

run_fixture unset ""
[ ! -e "${test_root}/unset/Users/runner/.config/flanforge/dependency-routing.zsh" ]
[ ! -e "${test_root}/unset/etc/gradle/init.d/chilled-proxy.init.gradle" ]
if grep -R -q 'SERVICES_ROOT_DOMAIN' "${test_root}/unset"; then
  echo "unset Tart routing leaked its build-domain input" >&2
  exit 1
fi

if run_fixture malformed example..invalid >/dev/null 2>&1; then
  echo "malformed Tart dependency domain was accepted" >&2
  exit 1
fi

hostile_gradle="$(mktemp /tmp/flanforge-tart-hostile.XXXXXX)"
if env FLANFORGE_BREW_BINARY="${fake_bin}/brew" \
  FLANFORGE_DEPENDENCY_ROUTE_HELPER=/etc/passwd \
  FLANFORGE_GRADLE_INIT_FILE="${hostile_gradle}" \
  SERVICES_ROOT_DOMAIN=example.invalid \
  bash "${template_dir}/scripts/provision-network.sh" >/dev/null 2>&1; then
  echo "unsafe Tart dependency helper path was accepted" >&2
  exit 1
fi
rm -f "${hostile_gradle}"

echo "Tart dependency provisioning fake tests passed"
