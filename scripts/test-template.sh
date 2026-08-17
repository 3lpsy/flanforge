#!/usr/bin/env bash
# Clone the retained Tart template, exercise it over SSH exactly as the daemon
# does, and delete the clone. This is the job-time path: the runner account
# logging in over SSH, not the administrator session Packer provisions through.
set -Eeuo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

die() {
  echo "error: $*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    "Usage:" \
    "  ${0##*/} [--keep] [template-name]" \
    "  ${0##*/} -h | --help | help" \
    "" \
    "Clone the built template, run read-only checks over SSH as the runner" \
    "account, then delete the clone. The simulator is inventoried but never" \
    "booted, so this stays fast." \
    "" \
    "--keep leaves the clone running for manual inspection and prints how to" \
    "reach it. The project-root .env is sourced as trusted shell syntax." \
    "" \
    "Environment:" \
    "  TART_VM_NAME                          Template to clone (default: flanforge-base)" \
    "  TART_HOME                             Tart storage (default: ~/.tart)" \
    "  FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE   Guest public key; its private half is used" \
    "  FLANFORGE_PREWARM_TARGET              Simulator expected present (default: iPhone 17 Pro)" \
    "  FLANFORGE_TAILSCALE_ENABLED           Also check the retained tailnet login" \
    "  FLANFORGE_TEST_GIT_HOST               Forge host to reach on 443 and 22" \
    "  SERVICES_ROOT_DOMAIN                  Also check baked dependency routing" >&2
}

keep_clone=false
positional_argument_count=0
template_argument=""
for argument in "$@"; do
  case "${argument}" in
    -h|--help|help)
      usage
      exit 0
      ;;
    --keep)
      keep_clone=true
      ;;
    -*)
      usage
      die "unknown option '${argument}'"
      ;;
    *)
      template_argument="${argument}"
      positional_argument_count=$((positional_argument_count + 1))
      ;;
  esac
done

env_file="${project_root}/.env"
if [ -f "${env_file}" ]; then
  echo "==> loading settings from ${env_file}"
  set -a
  # shellcheck disable=SC1090
  source "${env_file}"
  set +a
fi

[ "${positional_argument_count}" -le 1 ] || {
  usage
  die "at most one template name may be provided"
}

command -v tart >/dev/null 2>&1 \
  || die "tart is required (brew install cirruslabs/cli/tart)"
command -v ssh >/dev/null 2>&1 || die "ssh is required"
command -v ssh-keygen >/dev/null 2>&1 || die "ssh-keygen is required"

template_name="${template_argument:-${TART_VM_NAME:-flanforge-base}}"
[[ "${template_name}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] \
  || die "template name contains unsupported characters"
tart get "${template_name}" >/dev/null 2>&1 \
  || die "Tart VM '${template_name}' not found; build it with build-template.sh"

prewarm_target="${FLANFORGE_PREWARM_TARGET:-iPhone 17 Pro}"
tailscale_enabled="${FLANFORGE_TAILSCALE_ENABLED:-false}"
services_root_domain="${SERVICES_ROOT_DOMAIN:-}"

# Reachability of the forge is a tailnet policy question, so it is only checked
# when the deployment names a host to try.
git_host="${FLANFORGE_TEST_GIT_HOST:-}"
if [ -n "${git_host}" ]; then
  [[ "${git_host}" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]{0,253}[A-Za-z0-9])?$ ]] \
    || die "FLANFORGE_TEST_GIT_HOST must be a hostname"
fi

runner_pin_file="${project_root}/ci/forgejo-runner.env"
[ -f "${runner_pin_file}" ] || die "runner pin not found at '${runner_pin_file}'"
runner_version="$(sed -n 's/^FORGEJO_RUNNER_VERSION=//p' "${runner_pin_file}")"
[[ "${runner_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "Forgejo Runner version pin is invalid"

# The daemon authenticates with the private half of the provisioned guest key.
default_guest_identity="${HOME}/Library/Application Support/flanforge/guest-ssh-key"
guest_public_key="${FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE:-${default_guest_identity}.pub}"
guest_identity="${guest_public_key%.pub}"
[ -f "${guest_identity}" ] \
  || die "guest SSH private key not found at '${guest_identity}'"

clone_name="${template_name}-test-$$"
known_hosts_file=""
clone_created=false

cleanup() {
  local status=$?
  if [ -n "${known_hosts_file}" ]; then
    rm -f "${known_hosts_file}"
  fi
  if [ "${clone_created}" = true ] && [ "${keep_clone}" != true ]; then
    echo "==> deleting ${clone_name}"
    tart stop "${clone_name}" >/dev/null 2>&1 || true
    tart delete "${clone_name}" >/dev/null 2>&1 \
      || echo "warning: could not delete '${clone_name}'; remove it manually" >&2
  fi
  return "${status}"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

known_hosts_file="$(mktemp "${TMPDIR:-/tmp}/flanforge-test-known-hosts.XXXXXX")" \
  || die "cannot create a temporary known_hosts file"
chmod 0600 "${known_hosts_file}"

echo "==> cloning ${template_name} -> ${clone_name}"
tart clone "${template_name}" "${clone_name}"
clone_created=true

echo "==> starting ${clone_name}"
tart run --no-graphics "${clone_name}" >/dev/null 2>&1 &
tart_pid=$!

guest_ip=""
for _ in $(seq 1 60); do
  if ! kill -0 "${tart_pid}" 2>/dev/null; then
    die "the Tart process exited before the guest was reachable"
  fi
  guest_ip="$(tart ip "${clone_name}" 2>/dev/null || true)"
  if [ -n "${guest_ip}" ]; then
    break
  fi
  sleep 2
done
[ -n "${guest_ip}" ] || die "guest did not report an address within 120s"
echo "==> guest address: ${guest_ip}"

ssh_options=(
  -o BatchMode=yes
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=accept-new
  -o UserKnownHostsFile="${known_hosts_file}"
  -o ConnectTimeout=10
  -i "${guest_identity}"
)

run_remote() {
  ssh "${ssh_options[@]}" "runner@${guest_ip}" "$@"
}

for _ in $(seq 1 30); do
  if run_remote true >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
run_remote true >/dev/null 2>&1 \
  || die "cannot reach the guest as 'runner'; the identity or account is wrong"

checks_run=0
checks_failed=0

check() {
  local label="$1"
  shift
  checks_run=$((checks_run + 1))
  local output
  if output="$(run_remote "$@" 2>&1)"; then
    printf '  ok   %s\n' "${label}"
    if [ -n "${output}" ]; then
      printf '%s\n' "${output}" | head -n 8 | sed 's/^/         /'
    fi
  else
    checks_failed=$((checks_failed + 1))
    printf '  FAIL %s\n' "${label}"
    printf '%s\n' "${output}" | head -n 6 | sed 's/^/         /'
  fi
}

echo
echo "==> account"
check "logs in as runner" 'test "$(id -un)" = runner'
check "home is /Users/runner" 'test "$HOME" = /Users/runner'
check "is not an administrator" '! id -Gn | tr " " "\n" | grep -Fxq admin'
check "has no passwordless sudo" '! sudo -n true 2>/dev/null'

echo
echo "==> login environment"
check "login PATH" 'zsh -lc "echo \$PATH"'
check "Homebrew is on the login PATH" 'zsh -lc "command -v brew"'
check "Cargo is on the login PATH" 'zsh -lc "command -v cargo"'

echo
echo "==> Forgejo Runner"
check "binary is present and executable" 'test -x /Users/runner/bin/forgejo-runner'
check "binary runs and reports v${runner_version}" \
  "/Users/runner/bin/forgejo-runner --version | grep -Eq 'v?${runner_version}' && /Users/runner/bin/forgejo-runner --version"

echo
echo "==> Apple toolchain"
check "xcodebuild" 'xcodebuild -version'
check "iphoneos SDK" 'xcrun --sdk iphoneos --show-sdk-path'
check "iphonesimulator SDK" 'xcrun --sdk iphonesimulator --show-sdk-path'
check "an iOS runtime is installed" \
  'xcrun simctl list runtimes | awk "/^iOS / && \$0 !~ /unavailable/ { found = 1 } END { exit found ? 0 : 1 }"'
check "prewarm target '${prewarm_target}' exists" \
  "xcrun simctl list devices available | grep -F '${prewarm_target}'"

echo
echo "==> CI tools"
check "rustc" 'zsh -lc "rustc --version"'
check "iOS Rust targets" \
  'zsh -lc "rustup target list --installed | grep -Fx aarch64-apple-ios && rustup target list --installed | grep -Fx aarch64-apple-ios-sim"'
check "just" 'zsh -lc "just --version"'
check "cargo-nextest" 'zsh -lc "cargo nextest --version"'
check "sccache" 'zsh -lc "sccache --version"'
check "node" 'zsh -lc "node --version"'
check "bun" 'zsh -lc "bun --version"'
check "python3" 'zsh -lc "python3 --version"'
check "uv" 'zsh -lc "uv --version"'
check "xcodegen" 'zsh -lc "xcodegen --version"'
check "typeshare" 'zsh -lc "typeshare --version"'

if [ -n "${services_root_domain}" ]; then
  echo
  echo "==> dependency routing"
  check "Cargo registry configuration is baked" 'test -s /Users/runner/.cargo/config.toml'
fi

echo
echo "==> Tailscale"
check "CLI is present" '/opt/homebrew/bin/tailscale version'
# The runner account can only reach the daemon when it is the configured
# operator, which is the property a fresh login is most likely to have lost.
check "runner is the Tailscale operator" '/opt/homebrew/bin/tailscale debug prefs >/dev/null'
if [ "${tailscale_enabled}" = true ]; then
  check "retained login is Running" \
    '/opt/homebrew/bin/tailscale status --json | grep -Eq "\"BackendState\"[[:space:]]*:[[:space:]]*\"Running\""'
  check "status" '/opt/homebrew/bin/tailscale status'
  if [ -n "${git_host}" ]; then
    # Git over the tailnet is what every job needs first: the web API for
    # checkout and artifacts, and SSH for push.
    check "reaches ${git_host}:443" "nc -z -G 5 -w 5 '${git_host}' 443"
    check "reaches ${git_host}:22" "nc -z -G 5 -w 5 '${git_host}' 22"
  fi
else
  check "is logged out as configured" \
    '! /opt/homebrew/bin/tailscale status --json 2>/dev/null | grep -Eq "\"BackendState\"[[:space:]]*:[[:space:]]*\"Running\""'
fi

echo
echo "==> guest host key"
# Clones inherit the template's host key, so this is the value to pin in the
# daemon's guest.ssh_known_hosts_file.
if [ -s "${known_hosts_file}" ]; then
  ssh-keygen -l -f "${known_hosts_file}" | sed 's/^/    /'
  echo "    pin these keys under the configured guest.ssh_host_key_alias"
else
  echo "    warning: no host key was recorded" >&2
fi

echo
if [ "${keep_clone}" = true ]; then
  echo "Clone '${clone_name}' left running at ${guest_ip}."
  echo "  ssh -i '${guest_identity}' runner@${guest_ip}"
  echo "  tart stop '${clone_name}' && tart delete '${clone_name}'"
fi

echo "${checks_run} checks run, ${checks_failed} failed."
[ "${checks_failed}" -eq 0 ] || exit 1
echo "Template '${template_name}' is usable over the job-time path."
