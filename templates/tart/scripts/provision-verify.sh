#!/bin/bash
# Fail the image build now if a required iOS/runner dependency is absent.
set -Eeuo pipefail

# Most checks below are bare tests, so name the failing line for the operator.
trap 'echo "verification failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

dependency_checksums_file="${FLANFORGE_DEPENDENCY_CHECKSUMS_FILE:-/tmp/flanforge-dependency-checksums.sh}"
[[ "${dependency_checksums_file}" =~ ^/tmp/flanforge-[A-Za-z0-9._-]{1,96}$ ]] \
  && [ -f "${dependency_checksums_file}" ] \
  && [ ! -L "${dependency_checksums_file}" ] \
  || { echo "invalid dependency checksum file" >&2; exit 1; }
# shellcheck source=../../shared/dependencies/checksums.sh
source "${dependency_checksums_file}"
[[ "${CHILLED_PROXY_GRADLE_SHA256:-}" =~ ^[a-f0-9]{64}$ ]] \
  || { echo "invalid Gradle init-script checksum" >&2; exit 1; }
rm -f "${dependency_checksums_file}"

echo "==> runner Xcode, SDKs, and simulator inventory"
sudo -H -u runner env HOME=/Users/runner /bin/bash <<'RUNNER_XCODE'
set -euo pipefail
xcodebuild -version
xcrun --sdk iphoneos --show-sdk-path >/dev/null
xcrun --sdk iphonesimulator --show-sdk-path >/dev/null
echo "==> installed simulator runtimes"
simulator_runtimes="$(xcrun simctl list runtimes)"
printf '%s\n' "${simulator_runtimes}"
printf '%s\n' "${simulator_runtimes}" \
  | awk '/^iOS / && $0 !~ /unavailable/ { found = 1 } END { exit found ? 0 : 1 }'
echo "==> available simulator devices"
xcrun simctl list devices available
RUNNER_XCODE

echo "==> runner account"
# The runner home is mode 0700 and owned by runner, so the provisioning admin
# account cannot traverse it: inspect its contents through sudo.
[ "$(dscl . -read /Users/runner NFSHomeDirectory | awk '{ print $2 }')" = /Users/runner ]
[ "$(stat -f '%Su:%Sg' /Users/runner)" = runner:staff ]
[ "$(stat -f '%Lp' /Users/runner)" = 700 ]
sudo -H -u runner test -w /Users/runner
sudo test -s /Users/runner/.ssh/authorized_keys
[ "$(sudo stat -f '%Su:%Sg' /Users/runner/.ssh)" = runner:staff ]
[ "$(sudo stat -f '%Lp' /Users/runner/.ssh)" = 700 ]
[ "$(sudo stat -f '%Su:%Sg' /Users/runner/.ssh/authorized_keys)" = runner:staff ]
[ "$(sudo stat -f '%Lp' /Users/runner/.ssh/authorized_keys)" = 600 ]
sudo test -x /Users/runner/bin/forgejo-runner
[ "$(sudo stat -f '%Su:%Sg' /Users/runner/bin/forgejo-runner)" = runner:staff ]
[ "$(sudo stat -f '%Lp' /Users/runner/bin/forgejo-runner)" = 700 ]
sudo env LC_ALL=C file /Users/runner/bin/forgejo-runner | grep -Eq 'Mach-O.*arm64'
# macOS preference daemons write root-owned files into a fresh account's own
# home, so only another login account's files indicate an inherited home.
if sudo find /Users/runner ! -user runner ! -user root -print -quit | grep -q .; then
  echo "runner home contains files owned by another account" >&2
  sudo find /Users/runner ! -user runner ! -user root -print >&2
  exit 1
fi
if id -Gn runner | tr ' ' '\n' | grep -Fxq admin; then
  echo "runner unexpectedly has administrator access" >&2
  exit 1
fi
if sudo -H -u runner sudo -n true >/dev/null 2>&1; then
  echo "runner unexpectedly has passwordless sudo access" >&2
  exit 1
fi

echo "==> runner privacy grants"
# Only asserted when they were asked for: an operator may remove the grants
# deliberately, and that must not fail the build.
case "${PRIVACY_GRANTS_ENABLED}" in
  true)
    runner_tcc_db="/Users/runner/Library/Application Support/com.apple.TCC/TCC.db"
    sudo test -f "${runner_tcc_db}"
    [ "$(sudo stat -f '%Su:%Sg' "${runner_tcc_db}")" = runner:staff ]
    granted_services="$(sudo sqlite3 "${runner_tcc_db}" \
      'SELECT DISTINCT service FROM access WHERE auth_value = 2;')"
    for expected_service in kTCCServiceAccessibility kTCCServiceAppleEvents \
      kTCCServicePostEvent kTCCServiceScreenCapture; do
      printf '%s\n' "${granted_services}" | grep -Fxq "${expected_service}" \
        || { echo "runner is missing ${expected_service}" >&2; exit 1; }
    done
    ;;
  false)
    echo "    grants intentionally disabled; not asserted"
    ;;
  *)
    echo "PRIVACY_GRANTS_ENABLED must be true or false" >&2
    exit 1
    ;;
esac

[[ "${FORGEJO_RUNNER_VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || { echo "invalid expected Forgejo Runner version" >&2; exit 1; }
runner_version="$(sudo -H -u runner env HOME=/Users/runner \
  /Users/runner/bin/forgejo-runner --version)"
# The binary has to run and report a version; which version is the pin's
# business, not this check's. A mismatch is worth seeing, not worth failing
# the build over.
case "${runner_version}" in
  *[0-9].[0-9]*) ;;
  *)
    echo "Forgejo Runner did not report a usable version" >&2
    printf 'reported: %s\n' "${runner_version}" >&2
    exit 1
    ;;
esac
case "${runner_version}" in
  *"v${FORGEJO_RUNNER_VERSION}"*|*" ${FORGEJO_RUNNER_VERSION}"*) ;;
  *)
    printf 'note: runner reports %s, pin is %s\n' \
      "${runner_version}" "${FORGEJO_RUNNER_VERSION}" >&2
    ;;
esac

echo "==> native iOS and CI tools"
sudo -H -u runner env HOME=/Users/runner /bin/zsh -lc '
  set -e
  rustc --version
  cargo --version
  rustup component list --installed | grep -E "^(rustfmt|clippy)-" >/dev/null
  rustup target list --installed | grep -Fx aarch64-apple-ios >/dev/null
  rustup target list --installed | grep -Fx aarch64-apple-ios-sim >/dev/null
  just --version
  cargo nextest --version
  sccache --version
  node --version
  bun --version
  python3 --version
  uv --version
  xcodegen --version
  typeshare --version
'

echo "==> dependency routing"
if [ -n "${SERVICES_ROOT_DOMAIN:-}" ]; then
  dependency_proxy_url="https://deps.${SERVICES_ROOT_DOMAIN}"
  cargo_index_url="${dependency_proxy_url}/crates/index/"
  npm_registry_url="${dependency_proxy_url}/npm/"
  python_index_url="${dependency_proxy_url}/pypi/simple/"
  pytorch_index_url="${dependency_proxy_url}/pytorch/simple/"
  maven_repository_url="${dependency_proxy_url}/maven"
  google_maven_repository_url="${dependency_proxy_url}/google-maven"
  [ "$(sudo head -n 1 /Users/runner/.zshenv)" = \
    'source "$HOME/.config/flanforge/dependency-routing.zsh"' ]
  sudo grep -Fqx 'replace-with = "chilled-proxy"' \
    /Users/runner/.cargo/config.toml
  sudo grep -Fqx "registry = \"sparse+${cargo_index_url}\"" \
    /Users/runner/.cargo/config.toml
  sudo grep -Fqx "registry=${npm_registry_url}" /Users/runner/.npmrc
  grep -Fqx "index-url = ${python_index_url}" /etc/pip.conf
  grep -Fqx "url = \"${python_index_url}\"" /etc/uv/uv.toml
  sudo grep -Fqx "      <url>${maven_repository_url}</url>" \
    /Users/runner/.m2/settings.xml
  sudo grep -Fqx '      <mirrorOf>central</mirrorOf>' \
    /Users/runner/.m2/settings.xml
  sudo grep -Fqx "      <url>${google_maven_repository_url}</url>" \
    /Users/runner/.m2/settings.xml
  sudo grep -Fqx '      <mirrorOf>google</mirrorOf>' \
    /Users/runner/.m2/settings.xml
  ! sudo grep -Eq '<mirrorOf>[[:space:]]*\*[[:space:]]*</mirrorOf>' \
    /Users/runner/.m2/settings.xml
  grep -Fqx "systemProp.chilled.proxy.url=${dependency_proxy_url}" \
    /etc/gradle/gradle.properties
  sudo cmp /etc/gradle/init.d/chilled-proxy.init.gradle \
    /Users/runner/.gradle/init.d/chilled-proxy.init.gradle
  printf '%s  %s\n' "${CHILLED_PROXY_GRADLE_SHA256}" \
    /etc/gradle/init.d/chilled-proxy.init.gradle | shasum -a 256 -c
  sudo -H -u runner env HOME=/Users/runner /bin/zsh -c \
    "test \"\${BUN_CONFIG_REGISTRY}\" = '${npm_registry_url}' &&
     test \"\${PIP_INDEX_URL}\" = '${python_index_url}' &&
     test \"\${UV_DEFAULT_INDEX}\" = '${python_index_url}' &&
     test \"\${PYTORCH_REGISTRY}\" = '${pytorch_index_url}' &&
     test \"\${FLANFORGE_PYTORCH_INDEX_URL}\" = '${pytorch_index_url}' &&
     test \"\${CHILLED_PROXY_URL}\" = '${dependency_proxy_url}' &&
     test -z \"\${SERVICES_ROOT_DOMAIN:-}\" &&
     test \"\${GRADLE_USER_HOME}\" = /Users/runner/.gradle"
else
  sudo test ! -e /Users/runner/.config/flanforge/dependency-routing.zsh
  sudo test ! -e /Users/runner/.gradle/init.d/chilled-proxy.init.gradle
  sudo test ! -e /Users/runner/.m2/settings.xml
  test ! -e /etc/gradle/init.d/chilled-proxy.init.gradle
  sudo -H -u runner env HOME=/Users/runner /bin/zsh -c \
    'test -z "${SERVICES_ROOT_DOMAIN:-}"'
fi

echo "==> Tailscale installed and daemon loaded"
/opt/homebrew/bin/tailscale version | head -n 1
sudo launchctl print system/com.tailscale.tailscaled >/dev/null
sudo /opt/homebrew/bin/tailscale debug prefs \
  | grep -Eq '"OperatorUser"[[:space:]]*:[[:space:]]*"runner"'
tailscale_running=false
if sudo /opt/homebrew/bin/tailscale status --json 2>/dev/null \
  | grep -Eq '"BackendState"[[:space:]]*:[[:space:]]*"Running"'; then
  tailscale_running=true
fi
if [ "${TAILSCALE_ENABLED}" = true ] && [ "${tailscale_running}" != true ]; then
  echo "Tailscale must be Running when template enrollment is enabled" >&2
  exit 1
fi
if [ "${TAILSCALE_ENABLED}" = false ] && [ "${tailscale_running}" = true ]; then
  echo "Tailscale must be logged out when template enrollment is disabled" >&2
  exit 1
fi

guest_recycle_helper=/usr/local/libexec/flanforge-guest-recycle
[ -f "${guest_recycle_helper}" ] && [ ! -L "${guest_recycle_helper}" ] \
  || { echo "the guest recycle helper is missing" >&2; exit 1; }
[ "$(stat -f '%Su:%Sg:%Lp' "${guest_recycle_helper}")" = root:wheel:755 ] \
  || { echo "the guest recycle helper has the wrong ownership" >&2; exit 1; }
bash -n "${guest_recycle_helper}"
# The same contract the libvirt sibling reports, so the daemon reads one shape.
"${guest_recycle_helper}" --self-test \
  | /usr/local/bin/jq -e '.schema == 1 and .contract == 1 and .job_account.name == "runner"' \
    >/dev/null
echo "==> guest recycle helper baked"

echo "==> provisioning secrets removed"
if [ -e /tmp/flanforge-tailscale-preauth-key ]; then
  echo "tailnet join key survived provisioning" >&2
  exit 1
fi

echo "flanforge-base provisioning verified"
