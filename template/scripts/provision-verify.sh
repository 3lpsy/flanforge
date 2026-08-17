#!/bin/bash
# Fail the image build now if a required iOS/runner dependency is absent.
set -Eeuo pipefail

# Most checks below are bare tests, so name the failing line for the operator.
trap 'echo "verification failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

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

[[ "${FORGEJO_RUNNER_VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || { echo "invalid expected Forgejo Runner version" >&2; exit 1; }
runner_version="$(sudo -H -u runner env HOME=/Users/runner \
  /Users/runner/bin/forgejo-runner --version)"
# Upstream's build stamps the version with or without a leading "v" depending
# on how the tag is resolved; require the exact version either way.
case "${runner_version}" in
  *"v${FORGEJO_RUNNER_VERSION}"*|*" ${FORGEJO_RUNNER_VERSION}"*) ;;
  *)
    echo "Forgejo Runner version does not match ${FORGEJO_RUNNER_VERSION}" >&2
    printf 'reported: %s\n' "${runner_version}" >&2
    exit 1
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

echo "==> provisioning secrets removed"
if [ -e /tmp/flanforge-tailscale-preauth-key ]; then
  echo "tailnet join key survived provisioning" >&2
  exit 1
fi

echo "flanforge-base provisioning verified"
