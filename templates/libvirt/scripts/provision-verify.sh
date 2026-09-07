#!/usr/bin/env bash
# Fail the build if the Linux job contract is incomplete or over-privileged.
set -Eeuo pipefail

trap 'echo "guest verification failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

# Rootless podman re-execs and chdirs back to the caller's cwd — the build
# account's 0700 home, which the job account cannot enter. Every path below
# is absolute, so a neutral cwd costs nothing.
cd /

runner_home=/home/runner
runner_uid=2000
subordinate_id_helper=/tmp/flanforge-subordinate-ids.sh
subordinate_id_range_file=/usr/local/share/flanforge/runner-subordinate-id-range
retained_login_marker=/usr/local/share/flanforge/tailscale-retained-login
block_rpcs_file=/usr/local/share/flanforge/qemu-ga-block-rpcs
job_account_file=/usr/local/share/flanforge/job-account
guest_ready_helper=/usr/local/libexec/flanforge-guest-ready
guest_recycle_helper=/usr/local/libexec/flanforge-guest-recycle
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

[ "$(. /etc/os-release; printf '%s' "${ID}")" = fedora ]

# The agent channel runs under qemu-ga's confined SELinux domain, which denies
# the readiness probes; the image must boot with SELinux off.
if [ -f /etc/selinux/config ]; then
  grep -Eq '^SELINUX=disabled$' /etc/selinux/config \
    || { echo "SELinux is not disabled for the image" >&2; exit 1; }
fi
if command -v getenforce >/dev/null; then
  [ "$(getenforce)" != Enforcing ] \
    || { echo "SELinux is still enforcing in the build" >&2; exit 1; }
fi

[ "$(id -u runner)" -eq "${runner_uid}" ]
[ "$(getent passwd runner | cut -d: -f6)" = "${runner_home}" ]
[ "$(stat -c '%U:%G:%a' "${runner_home}")" = runner:runner:700 ]
if id -nG runner | tr ' ' '\n' | grep -Eq '^(wheel|root|libvirt|qemu)$'; then
  echo "runner unexpectedly belongs to a privileged group" >&2
  exit 1
fi
if sudo -H -u runner sudo -n true >/dev/null 2>&1; then
  echo "runner unexpectedly has passwordless sudo" >&2
  exit 1
fi

[ -x /usr/local/bin/forgejo-runner ]
printf '%s  %s\n' "${FORGEJO_RUNNER_SHA256}" /usr/local/bin/forgejo-runner \
  | sha256sum -c -
runner_output="$(sudo -H -u runner /usr/local/bin/forgejo-runner --version)"
case "${runner_output}" in
  *"v${FORGEJO_RUNNER_VERSION}"*|*" ${FORGEJO_RUNNER_VERSION}"*) ;;
  *) echo "Forgejo Runner version mismatch: ${runner_output}" >&2; exit 1 ;;
esac
sudo -H -u runner /usr/local/bin/forgejo-runner one-job --help >/dev/null

command -v podman >/dev/null
command -v docker >/dev/null
command -v buildah >/dev/null
rpm -q docker-cli >/dev/null
! rpm -q podman-docker >/dev/null 2>&1
# The CLI must come alone: a root Docker daemon in the guest is a second,
# unsupervised container runtime beside rootless podman.
! rpm -q moby-engine >/dev/null 2>&1
LC_ALL=C file "$(command -v docker)" | grep -Eq 'ELF 64-bit.*x86-64'
docker --version | grep -Eq '^Docker version '
# The home is 0700 and this script runs as the build account, so every look
# inside it goes through sudo; an unprivileged test would fail on traversal.
sudo test -L "${runner_home}/.config/systemd/user/sockets.target.wants/podman.socket"
[ -f /var/lib/systemd/linger/runner ]
[ ! -e "${subordinate_id_helper}" ]
[ -f "${subordinate_id_range_file}" ] && [ ! -L "${subordinate_id_range_file}" ]
[ "$(wc -l < "${subordinate_id_range_file}")" -eq 1 ]
IFS=: read -r subordinate_id_start subordinate_id_count \
  < "${subordinate_id_range_file}"
[[ "${subordinate_id_start}" =~ ^[0-9]+$ ]]
[ "${subordinate_id_count}" -eq 65536 ]
[ "${subordinate_id_start}" -ge 100000 ]
[ "${subordinate_id_start}" -le 4294901760 ]
awk -F: -v runner_start="${subordinate_id_start}" \
  -v runner_count="${subordinate_id_count}" '
  /^[[:space:]]*($|#)/ { next }
  NF != 3 || $1 !~ /^[A-Za-z0-9_.][A-Za-z0-9_.-]*[$]?$/ ||
    $2 !~ /^[0-9]+$/ || $3 !~ /^[0-9]+$/ ||
    $3 == 0 || $2 + $3 > 4294967296 { invalid = 1; next }
  $1 == "runner" {
    if (FILENAME == ARGV[1]) { uid_count++ } else { gid_count++ }
    if ($2 != runner_start || $3 != runner_count) { invalid = 1 }
    next
  }
  {
    runner_end = runner_start + runner_count
    entry_end = $2 + $3
    if (runner_start < entry_end && $2 < runner_end) { invalid = 1 }
  }
  END { exit invalid || uid_count != 1 || gid_count != 1 }
' /etc/subuid /etc/subgid
sudo install -d -m 0700 -o runner -g runner "/run/user/${runner_uid}"
sudo -H -u runner env XDG_RUNTIME_DIR="/run/user/${runner_uid}" \
  podman info --format json | jq -e '.host.security.rootless == true' >/dev/null
sudo -H -u runner env XDG_RUNTIME_DIR="/run/user/${runner_uid}" \
  buildah version >/dev/null

systemctl is-enabled sshd.service >/dev/null
systemctl is-enabled qemu-guest-agent.service >/dev/null
# Sandboxing here could hide captured guest state from the retention marker or
# clone-identity checks, silently defeating their verification.
for property in PrivateTmp ProtectHome ProtectSystem PrivateDevices \
  ProtectKernelTunables; do
  value="$(systemctl show qemu-guest-agent.service -p "${property}" --value)"
  case "${value}" in
    ""|no|off) ;;
    *)
      echo "qemu-guest-agent.service ${property}=${value} isolates the exec channel" >&2
      exit 1
      ;;
  esac
done
# The unit names the live variable: $QEMU_GA_ARGS on QEMU >= 9.1 packaging,
# a bare $BLOCK_RPCS or $BLACKLIST_RPC list before it.
block_rpcs_variable=""
for candidate in QEMU_GA_ARGS BLOCK_RPCS BLACKLIST_RPC; do
  if systemctl cat qemu-guest-agent.service \
    | grep -Eq "[\$]\{${candidate}\}|[\$]${candidate}([^A-Za-z0-9_]|\$)"; then
    [ -z "${block_rpcs_variable}" ] \
      || { echo "qemu-ga block-list variable is ambiguous" >&2; exit 1; }
    block_rpcs_variable="${candidate}"
  fi
done
[ -n "${block_rpcs_variable}" ] \
  || { echo "qemu-ga block-list variable is absent" >&2; exit 1; }
[ "$(grep -Ec "^[[:space:]]*${block_rpcs_variable}=" /etc/sysconfig/qemu-ga)" -eq 1 ]
block_rpcs_line="$(grep -E "^[[:space:]]*${block_rpcs_variable}=" \
  /etc/sysconfig/qemu-ga)"
block_rpcs="${block_rpcs_line#*=}"
if [ "${block_rpcs_variable}" = QEMU_GA_ARGS ]; then
  [[ "${block_rpcs}" =~ ^--block-rpcs=[a-z0-9,-]*$ ]]
  block_rpcs="${block_rpcs#--block-rpcs=}"
fi
[[ "${block_rpcs}" =~ ^[a-z0-9,-]*$ ]]
for unblocked in guest-exec guest-exec-status; do
  if printf '%s' "${block_rpcs}" | grep -Eq "(^|,)${unblocked}(,|$)"; then
    echo "qemu-ga still blocks ${unblocked}" >&2
    exit 1
  fi
done
for blocked in guest-file-open guest-file-close guest-file-read \
  guest-file-write guest-file-seek guest-file-flush; do
  printf '%s' "${block_rpcs}" | grep -Eq "(^|,)${blocked}(,|$)" \
    || { echo "qemu-ga no longer blocks ${blocked}" >&2; exit 1; }
done
[ -f "${block_rpcs_file}" ] && [ ! -L "${block_rpcs_file}" ]
[ "$(stat -c '%U:%G:%a' "${block_rpcs_file}")" = root:root:644 ]
[ "$(cat "${block_rpcs_file}")" = "${block_rpcs}" ]
[ -f "${job_account_file}" ] && [ ! -L "${job_account_file}" ]
[ "$(stat -c '%U:%G:%a' "${job_account_file}")" = root:root:644 ]
[ "$(cat "${job_account_file}")" = "runner:${runner_uid}" ]
[ -f "${guest_ready_helper}" ] && [ ! -L "${guest_ready_helper}" ]
[ "$(stat -c '%U:%G:%a' "${guest_ready_helper}")" = root:root:755 ]
bash -n "${guest_ready_helper}"
"${guest_ready_helper}" --self-test \
  | jq -e '.schema == 1 and .contract == 2 and .job_account.name == "runner"' >/dev/null
[ -f "${guest_recycle_helper}" ] && [ ! -L "${guest_recycle_helper}" ]
[ "$(stat -c '%U:%G:%a' "${guest_recycle_helper}")" = root:root:755 ]
bash -n "${guest_recycle_helper}"
"${guest_recycle_helper}" --self-test \
  | jq -e '.schema == 1 and .contract == 1 and .job_account.name == "runner"' >/dev/null
# The daemon's helper allow-list names these exact paths; a moved binary would
# fail every allocation rather than fall back.
for absolute in /usr/sbin/runuser /usr/bin/systemd-run /usr/bin/systemctl \
  /usr/bin/timeout /usr/bin/jq /usr/bin/cloud-init /usr/bin/pkill \
  /usr/bin/pgrep /usr/bin/loginctl /usr/bin/df; do
  [ -x "${absolute}" ] \
    || { echo "guest channel requires ${absolute}" >&2; exit 1; }
done
systemctl is-enabled tailscaled.service >/dev/null
systemctl is-active tailscaled.service >/dev/null
systemctl is-enabled flanforge-tailscale-operator.service >/dev/null
systemctl is-active flanforge-tailscale-operator.service >/dev/null
[ "$(stat -c '%U:%G:%a' \
  /etc/systemd/system/flanforge-tailscale-operator.service)" = root:root:644 ]
grep -Fqx 'Requires=tailscaled.service' \
  /etc/systemd/system/flanforge-tailscale-operator.service
grep -Fqx 'After=tailscaled.service' \
  /etc/systemd/system/flanforge-tailscale-operator.service
grep -Fqx 'Before=sshd.service' \
  /etc/systemd/system/flanforge-tailscale-operator.service
grep -Fqx 'ExecStart=/usr/bin/tailscale set --operator=runner' \
  /etc/systemd/system/flanforge-tailscale-operator.service
[ "$(command -v tailscale)" = /usr/bin/tailscale ]
# Fedora 42 merged /usr/sbin into /usr/bin; either resolution is the packaged one.
[[ "$(command -v tailscaled)" =~ ^/usr/s?bin/tailscaled$ ]]
rpm -q tailscale >/dev/null
tailscale version | head -n 1
# The CLI writes help to stderr, and `set` is the verb the boot unit runs.
tailscale set --help 2>&1 | grep -Fq -- '--operator'
sudo -H -u runner /usr/bin/tailscale set --operator=runner
sudo -H -u runner /usr/bin/tailscale debug prefs \
  | grep -Eq '"OperatorUser"[[:space:]]*:[[:space:]]*"runner"'
tailscale_status="$(sudo -H -u runner /usr/bin/tailscale status --json)"
if [ "${TAILSCALE_ENABLED:-false}" = true ]; then
  [ -f "${retained_login_marker}" ] && [ ! -L "${retained_login_marker}" ]
  [ "$(stat -c '%U:%G:%a' "${retained_login_marker}")" = root:root:644 ]
  printf '%s' "${tailscale_status}" | jq -e '
    .BackendState == "Running" and
    ((.TailscaleIPs // []) | length > 0) and
    ((.HaveNodeKey // false) == true)
  ' >/dev/null
else
  [ ! -e "${retained_login_marker}" ]
  printf '%s' "${tailscale_status}" | jq -e '
    .BackendState != "Running" and
    ((.TailscaleIPs // []) | length == 0) and
    ((.HaveNodeKey // false) == false) and
    ((.CurrentTailnet // null) == null)
  ' >/dev/null
fi
privileged_record=/usr/local/share/flanforge/privileged-account
if [ "${PRIVILEGED_ACCOUNT_ENABLED:-true}" = true ]; then
  [ -f "${privileged_record}" ] && [ ! -L "${privileged_record}" ]
  [ "$(stat -c '%U:%G:%a' "${privileged_record}")" = root:root:644 ]
  IFS=: read -r privileged_name privileged_uid < "${privileged_record}"
  [ "${privileged_name}" = prunner ]
  [ "$(id -u prunner)" -eq "${privileged_uid}" ]
  [ "$(stat -c '%U:%G:%a' /home/prunner)" = prunner:prunner:700 ]
  sudo test ! -e /home/prunner/.ssh/authorized_keys
  # /etc/sudoers.d denies unprivileged traversal, so every look goes via sudo.
  [ "$(sudo stat -c '%U:%G:%a' /etc/sudoers.d/50-flanforge-prunner)" = root:root:440 ]
  sudo visudo -cf /etc/sudoers.d/50-flanforge-prunner >/dev/null
  # NOPASSWD pairs with a locked password; password mode must gate sudo.
  if sudo grep -Fq 'NOPASSWD' /etc/sudoers.d/50-flanforge-prunner; then
    [ "$(sudo passwd -S prunner | awk '{print $2}')" = L ] \
      || [ "$(sudo passwd -S prunner | awk '{print $2}')" = LK ]
    sudo -H -u prunner sudo -n true
  else
    [ "$(sudo passwd -S prunner | awk '{print $2}')" = P ] \
      || [ "$(sudo passwd -S prunner | awk '{print $2}')" = PS ]
    ! sudo -H -u prunner sudo -n true >/dev/null 2>&1
  fi
else
  ! getent passwd prunner >/dev/null
  test ! -e "${privileged_record}"
  sudo test ! -e /etc/sudoers.d/50-flanforge-prunner
fi
test ! -e /tmp/flanforge-privileged-sudo-password
test ! -e /tmp/flanforge-tailscale-preauth-key
test ! -S /run/libvirt/libvirt-sock
test ! -S /run/podman/podman.sock
test ! -e /dev/kvm
test ! -e /etc/flanforge
test ! -e /var/lib/flanforge
test ! -e /run/flanforge
sudo test ! -e "${runner_home}/.ssh/authorized_keys"
sudo test ! -e "${runner_home}/.config/containers/auth.json"
sudo test ! -e /root/.config/containers/auth.json

proxy_configured=false
language_route_count=0
for route in "${CHILLED_PROXY_URL:-}" "${CARGO_INDEX_URL:-}" \
  "${NPM_REGISTRY_URL:-}" "${PYTHON_INDEX_URL:-}" \
  "${PYTORCH_INDEX_URL:-}" "${MAVEN_REPOSITORY_URL:-}" \
  "${GOOGLE_MAVEN_REPOSITORY_URL:-}"; do
  [ -z "${route}" ] || language_route_count=$((language_route_count + 1))
done
[ "${language_route_count}" -eq 0 ] || [ "${language_route_count}" -eq 7 ]
if [ "${language_route_count}" -eq 7 ]; then
  proxy_configured=true
  [ "$(sudo head -n 1 "${runner_home}/.bashrc")" = \
    'source "$HOME/.config/flanforge/dependency-routing.sh"' ]
  sudo grep -Fqx 'replace-with = "chilled-proxy"' \
    "${runner_home}/.cargo/config.toml"
  sudo grep -Fqx "registry = \"sparse+${CARGO_INDEX_URL}\"" \
    "${runner_home}/.cargo/config.toml"
  sudo grep -Fqx "registry=${NPM_REGISTRY_URL}" "${runner_home}/.npmrc"
  grep -Fqx "index-url = ${PYTHON_INDEX_URL}" /etc/pip.conf
  grep -Fqx "url = \"${PYTHON_INDEX_URL}\"" /etc/uv/uv.toml
  grep -Fqx 'default = true' /etc/uv/uv.toml
  sudo grep -Fqx "      <url>${MAVEN_REPOSITORY_URL}</url>" \
    "${runner_home}/.m2/settings.xml"
  sudo grep -Fqx '      <mirrorOf>central</mirrorOf>' \
    "${runner_home}/.m2/settings.xml"
  sudo grep -Fqx "      <url>${GOOGLE_MAVEN_REPOSITORY_URL}</url>" \
    "${runner_home}/.m2/settings.xml"
  sudo grep -Fqx '      <mirrorOf>google</mirrorOf>' \
    "${runner_home}/.m2/settings.xml"
  ! sudo grep -Eq '<mirrorOf>[[:space:]]*\*[[:space:]]*</mirrorOf>' \
    "${runner_home}/.m2/settings.xml"
  grep -Fqx "systemProp.chilled.proxy.url=${CHILLED_PROXY_URL}" \
    /etc/gradle/gradle.properties
  sudo cmp /etc/gradle/gradle.properties "${runner_home}/.gradle/gradle.properties"
  sudo cmp /etc/gradle/init.d/chilled-proxy.init.gradle \
    "${runner_home}/.gradle/init.d/chilled-proxy.init.gradle"
  printf '%s  %s\n' "${CHILLED_PROXY_GRADLE_SHA256}" \
    /etc/gradle/init.d/chilled-proxy.init.gradle | sha256sum -c -
  sudo grep -Fqx "export BUN_CONFIG_REGISTRY=${NPM_REGISTRY_URL}" \
    "${runner_home}/.config/flanforge/dependency-routing.sh"
  sudo grep -Fqx "export CHILLED_PROXY_URL=${CHILLED_PROXY_URL}" \
    "${runner_home}/.config/flanforge/dependency-routing.sh"
  sudo grep -Fqx "export PIP_INDEX_URL=${PYTHON_INDEX_URL}" \
    "${runner_home}/.config/flanforge/dependency-routing.sh"
  sudo grep -Fqx "export PYTORCH_REGISTRY=${PYTORCH_INDEX_URL}" \
    "${runner_home}/.config/flanforge/dependency-routing.sh"
  sudo grep -Fqx "export UV_DEFAULT_INDEX=${PYTHON_INDEX_URL}" \
    "${runner_home}/.config/flanforge/dependency-routing.sh"
  sudo -H -u runner env HOME="${runner_home}" \
    BASH_ENV="${runner_home}/.bashrc" /bin/bash -c \
    "test \"\${BUN_CONFIG_REGISTRY}\" = '${NPM_REGISTRY_URL}' &&
     test \"\${PIP_INDEX_URL}\" = '${PYTHON_INDEX_URL}' &&
     test \"\${UV_DEFAULT_INDEX}\" = '${PYTHON_INDEX_URL}' &&
     test \"\${PYTORCH_REGISTRY}\" = '${PYTORCH_INDEX_URL}' &&
     test \"\${FLANFORGE_PYTORCH_INDEX_URL}\" = '${PYTORCH_INDEX_URL}' &&
     test \"\${CHILLED_PROXY_URL}\" = '${CHILLED_PROXY_URL}' &&
     test \"\${GRADLE_USER_HOME}\" = '${runner_home}/.gradle'"
  # `runuser -l` is what the agent channel uses, and it reads .bash_profile
  # rather than .bashrc; without this the routing would be silently lost.
  sudo /usr/sbin/runuser -l runner -c \
    "test \"\${PIP_INDEX_URL}\" = '${PYTHON_INDEX_URL}' &&
     test \"\${UV_DEFAULT_INDEX}\" = '${PYTHON_INDEX_URL}' &&
     test \"\${CHILLED_PROXY_URL}\" = '${CHILLED_PROXY_URL}' &&
     test \"\${BUN_CONFIG_REGISTRY}\" = '${NPM_REGISTRY_URL}' &&
     test \"\${GRADLE_USER_HOME}\" = '${runner_home}/.gradle'"
else
  sudo test ! -e "${runner_home}/.config/flanforge/dependency-routing.sh"
  sudo test ! -e "${runner_home}/.gradle/init.d/chilled-proxy.init.gradle"
  sudo test ! -e "${runner_home}/.m2/settings.xml"
  test ! -e /etc/gradle/init.d/chilled-proxy.init.gradle
fi
if [ -n "${CONTAINER_REGISTRY_MIRROR:-}" ]; then
  proxy_configured=true
  test -s /etc/containers/registries.conf.d/50-flanforge-mirror.conf
fi

os_id="$(. /etc/os-release; printf '%s' "${ID}")"
os_version="$(. /etc/os-release; printf '%s' "${VERSION_ID}")"
podman_version="$(podman version --format '{{.Client.Version}}')"
privileged_account_json=null
if [ "${PRIVILEGED_ACCOUNT_ENABLED:-true}" = true ]; then
  privileged_account_json="$(jq -n --argjson uid "${privileged_uid}" \
    '{name: "prunner", uid: $uid}')"
fi
sudo jq -n \
  --arg os_id "${os_id}" \
  --arg os_version "${os_version}" \
  --arg runner_sha256 "${FORGEJO_RUNNER_SHA256}" \
  --arg runner_version "${FORGEJO_RUNNER_VERSION}" \
  --arg podman_version "${podman_version}" \
  --arg block_rpcs "${block_rpcs}" \
  --argjson job_account_uid "${runner_uid}" \
  --argjson privileged_account "${privileged_account_json}" \
  --argjson dependency_proxy_configured "${proxy_configured}" \
  '{
    schema_version: 1,
    guest_contract_version: 2,
    os: {id: $os_id, version_id: $os_version, architecture: "x86_64"},
    forgejo_runner: {version: $runner_version, sha256: $runner_sha256},
    podman: {version: $podman_version, rootless: true, docker_api: true},
    guest_agent: {exec_enabled: true, block_rpcs: $block_rpcs},
    job_account: {name: "runner", uid: $job_account_uid},
    privileged_account: $privileged_account,
    dependency_proxy_configured: $dependency_proxy_configured
  }' | sudo tee /usr/local/share/flanforge/guest-manifest.json >/dev/null

sudo cp /usr/local/share/flanforge/guest-manifest.json \
  /tmp/flanforge-guest-manifest.json
sudo chown packer:packer /tmp/flanforge-guest-manifest.json
sudo chmod 0600 /tmp/flanforge-guest-manifest.json

echo "==> Linux guest contract verified"
