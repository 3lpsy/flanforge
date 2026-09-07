#!/usr/bin/env bash
# Import, boot, verify, and exactly remove a disposable libvirt clone.
# shellcheck disable=SC2034 # Lifecycle helpers consume shared test state.
set -Eeuo pipefail
export LC_ALL=C

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../shared/env.sh
source "${template_dir}/../shared/env.sh"
# shellcheck source=host/common.sh
source "${template_dir}/host/common.sh"
# shellcheck source=host/test-runtime.sh
source "${template_dir}/host/test-runtime.sh"
# shellcheck source=host/test-dependencies.sh
source "${template_dir}/host/test-dependencies.sh"
# shellcheck source=host/test-tailscale.sh
source "${template_dir}/host/test-tailscale.sh"
remote_dependency_contract=""
remote_tailscale_contract=""

usage() {
  printf '%s\n' \
    "Usage:" \
    "  ${0##*/} [--execute] [--keep] [--image PATH] [--manifest PATH]" \
    "  ${0##*/} -h | --help | help" \
    "" \
    "Without --execute, validate the artifact and dedicated test libvirt" \
    "pool/network without creating anything. Destructive execution also" \
    "requires FLANFORGE_LIBVIRT_TEST_CONFIRM=destroy-test-resources." \
    "" \
    "The pool and network must be explicitly named and start with" \
    "'flanforge-test'. --keep leaves only the uniquely named test inventory." >&2
}

execute_test=false
keep_clone=false
image_input=""
manifest_input=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -h|--help|help) usage; exit 0 ;;
    --execute) execute_test=true; shift ;;
    --keep) keep_clone=true; shift ;;
    --image)
      [ "$#" -ge 2 ] || die "--image requires a path"
      image_input="$2"; shift 2
      ;;
    --manifest)
      [ "$#" -ge 2 ] || die "--manifest requires a path"
      manifest_input="$2"; shift 2
      ;;
    *) usage; die "unknown argument '$1'" ;;
  esac
done

load_template_environment "${template_dir}"

ensure_linux_x86_64
for command_name in base64 head jq qemu-img realpath sha256sum timeout virsh; do
  require_command "${command_name}"
done
timeout_version="$(timeout --version 2>/dev/null)" \
  || die "GNU coreutils timeout is required"
[[ "${timeout_version%%$'\n'*}" == *'GNU coreutils'* ]] \
  || die "GNU coreutils timeout is required"

libvirt_uri=qemu:///system
test_rpc_timeout_seconds="${LIBVIRT_TEST_RPC_TIMEOUT_SECONDS:-30}"
test_transfer_timeout_seconds="${LIBVIRT_TEST_TRANSFER_TIMEOUT_SECONDS:-1800}"
test_ssh_timeout_seconds="${LIBVIRT_TEST_SSH_TIMEOUT_SECONDS:-300}"
test_readiness_timeout_seconds="${LIBVIRT_TEST_READINESS_TIMEOUT_SECONDS:-120}"
test_readiness_probe_timeout_seconds="${LIBVIRT_TEST_READINESS_PROBE_TIMEOUT_SECONDS:-10}"
test_agent_timeout_seconds="${LIBVIRT_TEST_AGENT_TIMEOUT_SECONDS:-120}"
test_agent_probe_timeout_seconds="${LIBVIRT_TEST_AGENT_PROBE_TIMEOUT_SECONDS:-5}"
ensure_positive_integer LIBVIRT_TEST_RPC_TIMEOUT_SECONDS \
  "${test_rpc_timeout_seconds}" 300
ensure_positive_integer LIBVIRT_TEST_TRANSFER_TIMEOUT_SECONDS \
  "${test_transfer_timeout_seconds}" 7200
ensure_positive_integer LIBVIRT_TEST_SSH_TIMEOUT_SECONDS \
  "${test_ssh_timeout_seconds}" 1800
ensure_positive_integer LIBVIRT_TEST_READINESS_TIMEOUT_SECONDS \
  "${test_readiness_timeout_seconds}" 600
ensure_positive_integer LIBVIRT_TEST_READINESS_PROBE_TIMEOUT_SECONDS \
  "${test_readiness_probe_timeout_seconds}" 60
ensure_positive_integer LIBVIRT_TEST_AGENT_TIMEOUT_SECONDS \
  "${test_agent_timeout_seconds}" 600
ensure_positive_integer LIBVIRT_TEST_AGENT_PROBE_TIMEOUT_SECONDS \
  "${test_agent_probe_timeout_seconds}" 60
[ "${test_readiness_probe_timeout_seconds}" -le "${test_readiness_timeout_seconds}" ] \
  || die "readiness probe timeout must not exceed the readiness deadline"
test_pool="${FLANFORGE_LIBVIRT_TEST_POOL:-}"
test_network="${FLANFORGE_LIBVIRT_TEST_NETWORK:-}"
[ -n "${test_pool}" ] || die "FLANFORGE_LIBVIRT_TEST_POOL is required"
[ -n "${test_network}" ] || die "FLANFORGE_LIBVIRT_TEST_NETWORK is required"
ensure_safe_name FLANFORGE_LIBVIRT_TEST_POOL "${test_pool}"
ensure_safe_name FLANFORGE_LIBVIRT_TEST_NETWORK "${test_network}"
[[ "${test_pool}" =~ ^flanforge-test([._-][A-Za-z0-9._-]+)?$ ]] \
  || die "test pool must start with 'flanforge-test'"
[[ "${test_network}" =~ ^flanforge-test([._-][A-Za-z0-9._-]+)?$ ]] \
  || die "test network must start with 'flanforge-test'"

image_input="${image_input:-${LIBVIRT_TEST_IMAGE_FILE:-${template_dir}/output/flanforge-base.qcow2}}"
[[ "${image_input}" != *$'\n'* && "${image_input}" != *$'\r'* ]] \
  || die "image path contains unsupported characters"
[ -f "${image_input}" ] && [ ! -L "${image_input}" ] \
  || die "image must be an existing regular file, not a symlink"
image_file="$(realpath "${image_input}")"
manifest_input="${manifest_input:-${LIBVIRT_TEST_MANIFEST_FILE:-${image_file%.qcow2}.manifest.json}}"
[ -f "${manifest_input}" ] && [ ! -L "${manifest_input}" ] \
  || die "manifest must be an existing regular file, not a symlink"
manifest_file="$(realpath "${manifest_input}")"
[ "$(file_size "${manifest_file}")" -le 65536 ] \
  || die "manifest exceeds 64 KiB"

expected_image_name="$(basename "${image_file}")"
expected_image_sha256="$(jq -er --arg file "${expected_image_name}" \
  '.schema_version == 1 and .image.file == $file and .image.format == "qcow2" |
   if . then input_filename else error("image contract mismatch") end' \
  "${manifest_file}" >/dev/null && jq -er '.image.sha256' "${manifest_file}")" \
  || die "manifest does not describe the selected image"
[[ "${expected_image_sha256}" =~ ^[a-f0-9]{64}$ ]] \
  || die "manifest image digest is invalid"
printf '%s  %s\n' "${expected_image_sha256}" "${image_file}" \
  | run_bounded "${test_transfer_timeout_seconds}" sha256sum -c -

if ! image_info="$(run_bounded "${test_rpc_timeout_seconds}" \
  qemu-img info --output=json "${image_file}" | head -c 1048577)"; then
  die "qemu-img could not inspect the selected image within its bound"
fi
[ "${#image_info}" -le 1048576 ] \
  || die "qemu-img metadata exceeds 1 MiB"
printf '%s' "${image_info}" | jq -e \
  '.format == "qcow2" and ((."backing-filename" // "") == "")' >/dev/null \
  || die "test image must be a standalone qcow2"
virtual_bytes="$(printf '%s' "${image_info}" | jq -er '."virtual-size"')"
jq -e --argjson virtual_bytes "${virtual_bytes}" \
  '.image.virtual_bytes == $virtual_bytes and
   .guest.guest_contract_version == 2 and
   .guest.guest_agent.exec_enabled == true and
   .guest.job_account.uid == 2000 and
   .guest.podman.rootless == true and .guest.podman.docker_api == true and
   (.provenance.runner_release.api_url | startswith("https://")) and
   (.provenance.runner_release.download_base_url | startswith("https://")) and
   (.provenance.runner_release.signing_primary_fingerprint |
     test("^[A-F0-9]{40}$")) and
   (.provenance.dependency_proxy | keys == ["ca_sha256", "configured"]) and
   (.provenance.dependency_proxy.configured | type == "boolean") and
   (.provenance.dependency_proxy.ca_sha256 == null or
     (.provenance.dependency_proxy.ca_sha256 |
       test("^[a-f0-9]{64}$"))) and
   (.guest.dependency_proxy_configured ==
     .provenance.dependency_proxy.configured)' \
  "${manifest_file}" >/dev/null || die "manifest guest or virtual-size contract mismatch"

prepare_dependency_contract "${manifest_file}" \
  "${SERVICES_ROOT_DOMAIN:-}" "${FLANFORGE_CONTAINER_REGISTRY_MIRROR:-}"

run_test_virsh pool-info "${test_pool}" \
  | grep -Eq '^State:[[:space:]]+running$' || die "test pool is not active"
run_test_virsh net-info "${test_network}" \
  | grep -Eq '^Active:[[:space:]]+yes$' || die "test network is not active"

test_cpu="${LIBVIRT_TEST_CPU:-2}"
test_memory_mb="${LIBVIRT_TEST_MEMORY_MB:-4096}"
container_image_digest=7c8cb692ae09657cbc4a3f3cbd0e8d5a2690ba38386aaaf252dbb060bf5eb2e6
container_image="${LIBVIRT_TEST_CONTAINER_IMAGE:-docker.io/library/alpine:3.22@sha256:${container_image_digest}}"
ensure_positive_integer LIBVIRT_TEST_CPU "${test_cpu}" 16
ensure_positive_integer LIBVIRT_TEST_MEMORY_MB "${test_memory_mb}" 32768
[ "${test_memory_mb}" -ge 2048 ] || die "LIBVIRT_TEST_MEMORY_MB must be at least 2048"
[[ "${container_image}" =~ ^[A-Za-z0-9][A-Za-z0-9./:@_-]{0,255}$ ]] \
  || die "LIBVIRT_TEST_CONTAINER_IMAGE is not a safe OCI reference"

echo "==> image and manifest verified"
echo "==> test pool: ${test_pool}"
echo "==> test network: ${test_network}"
if [ "${execute_test}" != true ]; then
  echo "Preflight complete; no domain or volume was created."
  echo "Set FLANFORGE_LIBVIRT_TEST_CONFIRM=destroy-test-resources and add --execute to run."
  exit 0
fi
[ "${FLANFORGE_LIBVIRT_TEST_CONFIRM:-}" = destroy-test-resources ] \
  || die "set FLANFORGE_LIBVIRT_TEST_CONFIRM=destroy-test-resources before --execute"
for command_name in cloud-localds ssh ssh-keygen virt-install; do
  require_command "${command_name}"
done

test_work_dir="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-libvirt-test.XXXXXX")" \
  || die "cannot create a private test directory"
chmod 0700 "${test_work_dir}"
trap on_test_exit EXIT
trap 'exit 130' HUP INT TERM
test_identity="${test_work_dir}/guest-ssh-key"
ssh-keygen -q -t ed25519 -N '' -C flanforge-libvirt-test -f "${test_identity}"
test_public_key="$(< "${test_identity}.pub")"
test_host_identity="${test_work_dir}/guest-host-key"
ssh-keygen -q -t ed25519 -N '' -C flanforge-libvirt-test-host \
  -f "${test_host_identity}"
test_host_key_alias=flanforge-libvirt-test-guest
read -r test_host_key_type test_host_key_data _ < "${test_host_identity}.pub"
[ "${test_host_key_type}" = ssh-ed25519 ] \
  && [[ "${test_host_key_data}" =~ ^[A-Za-z0-9+/]+={0,2}$ ]] \
  || die "generated guest host key is invalid"
known_hosts="${test_work_dir}/known_hosts"
printf '%s %s %s\n' "${test_host_key_alias}" "${test_host_key_type}" \
  "${test_host_key_data}" > "${known_hosts}"
chmod 0600 "${known_hosts}"
stamp="$(date -u +%Y%m%d%H%M%S)"
test_name="flanforge-test-${stamp}-$$"
ensure_safe_name test-domain "${test_name}"
base_volume="${test_name}-base.qcow2"
root_volume="${test_name}-root.qcow2"
seed_volume="${test_name}-seed.iso"
guest_address=""

run_test_virsh dominfo "${test_name}" >/dev/null 2>&1 \
  && die "test domain already exists"
for volume_name in "${base_volume}" "${root_volume}" "${seed_volume}"; do
  run_test_virsh vol-info --pool "${test_pool}" \
    "${volume_name}" >/dev/null 2>&1 && die "test volume ${volume_name} already exists"
done

user_data="${test_work_dir}/user-data"
meta_data="${test_work_dir}/meta-data"
seed_image="${test_work_dir}/seed.iso"
printf '%s\n' \
  "instance-id: ${test_name}" \
  "local-hostname: ${test_name}" > "${meta_data}"
{
  printf '%s\n' \
    '#cloud-config' \
    'disable_root: true' \
    'ssh_deletekeys: true' \
    'ssh_genkeytypes: []' \
    'ssh_keys:' \
    '  ed25519_private: |'
  sed 's/^/    /' "${test_host_identity}"
  printf '%s\n' \
    "  ed25519_public: ${test_host_key_type} ${test_host_key_data}" \
    'ssh_pwauth: false' \
    'users:' \
    '  - name: runner' \
    '    lock_passwd: true' \
    '    shell: /bin/bash' \
    '    ssh_authorized_keys:' \
    "      - ${test_public_key}"
} > "${user_data}"
cloud-localds "${seed_image}" "${user_data}" "${meta_data}"

echo "==> importing unique base volume"
base_volume_key=""
create_volume "${base_volume}" "${virtual_bytes}B" qcow2 base_volume_key
run_test_virsh_with_timeout "${test_transfer_timeout_seconds}" vol-upload \
  "${base_volume_key}" "${image_file}"
run_test_virsh pool-refresh "${test_pool}" >/dev/null
create_overlay "${root_volume}" "${virtual_bytes}B" \
  "${base_volume_key}"
seed_volume_key=""
create_volume "${seed_volume}" "$(file_size "${seed_image}")B" raw seed_volume_key
run_test_virsh_with_timeout "${test_transfer_timeout_seconds}" vol-upload \
  "${seed_volume_key}" "${seed_image}"

domain_xml="${test_work_dir}/domain.xml"
test_domain_uuid="$(< /proc/sys/kernel/random/uuid)"
[[ "${test_domain_uuid}" =~ ^[a-f0-9]{8}-([a-f0-9]{4}-){3}[a-f0-9]{12}$ ]] \
  || die "kernel did not provide a valid domain UUID"
run_bounded "${test_rpc_timeout_seconds}" virt-install \
  --connect "${libvirt_uri}" --name "${test_name}" \
  --uuid "${test_domain_uuid}" \
  --description "FlanForge disposable template test" \
  --memory "${test_memory_mb}" --vcpus "${test_cpu}" --import \
  --disk "vol=${test_pool}/${root_volume},bus=virtio" \
  --disk "vol=${test_pool}/${seed_volume},device=cdrom" \
  --network "network=${test_network},model=virtio" \
  --channel unix,target.type=virtio,target.name=org.qemu.guest_agent.0 \
  --os-variant fedora-unknown --graphics none --noautoconsole \
  --print-xml > "${domain_xml}"
test_domain_definition_attempted=true
run_test_virsh define "${domain_xml}" >/dev/null
test_domain_defined=true
[ "$(run_test_virsh domuuid "${test_domain_uuid}")" = "${test_domain_uuid}" ] \
  || die "defined domain UUID does not match the generated identity"
test_domain_identity_verified=true
run_test_virsh start "${test_domain_uuid}" >/dev/null

# The guest-agent gate, before SSH: it is the only dynamic proof that the
# block-list rewrite took effect and that the transient-unit job contract holds,
# because qemu-ga cannot run inside Packer.
wait_for_guest_agent \
  || die "guest agent did not answer within ${test_agent_timeout_seconds} seconds"
echo "==> guest agent is answering"

agent_ready_reply="$(run_test_agent_exec "$(jq -cn '{
  execute: "guest-exec",
  arguments: {
    path: "/usr/local/libexec/flanforge-guest-ready",
    arg: ["--wait-seconds", "60"],
    "capture-output": true
  }}')")" || die "the baked readiness helper did not finish over guest-exec"
printf '%s' "${agent_ready_reply}" | jq -e '.return.exitcode == 0' >/dev/null \
  || die "the readiness helper reported the guest unready: ${agent_ready_reply}"
printf '%s' "${agent_ready_reply}" \
  | jq -r '.return."out-data" // ""' | base64 -d \
  | jq -e '.schema == 1 and .contract == 2 and .ready == true' >/dev/null \
  || die "the readiness helper did not report the expected contract"

# stdin fidelity: the two secrets the daemon sends reach the guest this way.
agent_nonce="$(< /proc/sys/kernel/random/uuid)"
agent_cat_reply="$(run_test_agent_exec "$(jq -cn --arg data "${agent_nonce}" '{
  execute: "guest-exec",
  arguments: {
    path: "/usr/bin/cat",
    arg: [],
    "input-data": ($data | @base64),
    "capture-output": true
  }}')")" || die "guest-exec could not run cat"
[ "$(printf '%s' "${agent_cat_reply}" | jq -r '.return."out-data" // ""' | base64 -d)" \
  = "${agent_nonce}" ] || die "guest-exec did not carry stdin through to the guest"

# Exit-code fidelity: every guest step the daemon runs is judged by this alone.
agent_false_reply="$(run_test_agent_exec "$(jq -cn '{
  execute: "guest-exec",
  arguments: {path: "/usr/bin/false", arg: []}}')")" \
  || die "guest-exec could not run false"
printf '%s' "${agent_false_reply}" | jq -e '.return.exitcode == 1' >/dev/null \
  || die "guest-exec did not report a non-zero exit code"

# The transient-unit contract: --pipe carries stdin, --wait propagates the exit
# status, systemctl stop reaches the whole cgroup, and RuntimeMaxSec is a
# guest-side deadline PID 1 enforces without the daemon.
agent_unit="flanforge-job-$(< /proc/sys/kernel/random/uuid).service"
agent_unit_reply="$(run_test_agent_exec "$(jq -cn --arg unit "${agent_unit}" '{
  execute: "guest-exec",
  arguments: {
    path: "/usr/bin/systemd-run",
    arg: ["--unit=" + $unit, "--pipe", "--wait",
          "--property=RuntimeMaxSec=60", "--property=TimeoutStopSec=10",
          "/bin/sh", "-c", "cat >/dev/null; exit 7"],
    "input-data": ("ignored" | @base64)
  }}')")" || die "systemd-run did not finish over guest-exec"
printf '%s' "${agent_unit_reply}" | jq -e '.return.exitcode == 7' >/dev/null \
  || die "systemd-run --wait did not propagate the unit exit status"

agent_stop_unit="flanforge-job-$(< /proc/sys/kernel/random/uuid).service"
run_test_agent_command "$(jq -cn --arg unit "${agent_stop_unit}" '{
  execute: "guest-exec",
  arguments: {
    path: "/usr/bin/systemd-run",
    arg: ["--unit=" + $unit, "--pipe", "--wait",
          "--property=RuntimeMaxSec=600", "--property=TimeoutStopSec=10",
          "/bin/sleep", "300"]}}')" >/dev/null \
  || die "could not start the long-running test unit"
sleep 2
run_test_agent_exec "$(jq -cn --arg unit "${agent_stop_unit}" '{
  execute: "guest-exec",
  arguments: {path: "/usr/bin/systemctl", arg: ["stop", $unit]}}')" >/dev/null \
  || die "systemctl stop did not finish over guest-exec"
agent_stopped_reply="$(run_test_agent_exec "$(jq -cn --arg unit "${agent_stop_unit}" '{
  execute: "guest-exec",
  arguments: {path: "/usr/bin/systemctl", arg: ["is-active", $unit]}}')")" \
  || die "could not read the stopped unit state"
printf '%s' "${agent_stopped_reply}" | jq -e '.return.exitcode != 0' >/dev/null \
  || die "the guest job unit survived systemctl stop"

agent_deadline_unit="flanforge-job-$(< /proc/sys/kernel/random/uuid).service"
agent_deadline_reply="$(run_test_agent_exec "$(jq -cn --arg unit "${agent_deadline_unit}" '{
  execute: "guest-exec",
  arguments: {
    path: "/usr/bin/systemd-run",
    arg: ["--unit=" + $unit, "--pipe", "--wait",
          "--property=RuntimeMaxSec=5", "--property=TimeoutStopSec=10",
          "/bin/sleep", "300"]}}')")" \
  || die "the guest-side deadline did not end its unit"
printf '%s' "${agent_deadline_reply}" | jq -e '.return.exitcode != 0' >/dev/null \
  || die "RuntimeMaxSec did not end the unit"

# Minimisation, not containment: the file RPCs stay blocked.
run_test_agent_command '{"execute":"guest-file-open","arguments":{"path":"/etc/hostname"}}' \
  | jq -e '.error' >/dev/null \
  || die "guest-file-open is no longer blocked"

# The README states this socket's owner and mode; a drift is a threat-model
# change rather than a cosmetic one.
agent_socket="$(sudo find /var/lib/libvirt/qemu/channel/target \
  -name org.qemu.guest_agent.0 -path "*${test_name}*" 2>/dev/null | head -n 1 || true)"
if [ -n "${agent_socket}" ]; then
  echo "==> guest agent socket: $(sudo stat -c '%U:%G:%a' "${agent_socket}" || true)"
else
  echo "warning: could not locate the guest agent channel socket" >&2
fi
agent_checks_ran=true
echo "==> guest agent channel verified"

mac_address="$(run_test_virsh domiflist "${test_domain_uuid}" \
  | awk '$2 == "network" { print $5; exit }')"
[[ "${mac_address}" =~ ^([a-f0-9]{2}:){5}[a-f0-9]{2}$ ]] \
  || die "cannot identify the test domain MAC address"
guest_address="$(wait_for_guest_address "${mac_address}")" \
  || die "guest did not receive an IPv4 lease within 180 seconds"
echo "==> guest address: ${guest_address}"

ssh_options=(
  -F /dev/null
  -o GlobalKnownHostsFile=/dev/null
  -o BatchMode=yes
  -o ConnectTimeout=10
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=yes
  -o HostKeyAlias="${test_host_key_alias}"
  -o UserKnownHostsFile="${known_hosts}"
  -o ForwardAgent=no
  -o ForwardX11=no
  -o ControlMaster=no
  -o ControlPath=none
  -o PermitLocalCommand=no
  -o ProxyCommand=none
  -i "${test_identity}"
)
run_remote_with_timeout() {
  local maximum_seconds="$1"
  shift
  run_test_ssh_with_timeout "${maximum_seconds}" \
    "${ssh_options[@]}" "runner@${guest_address}" "$@"
}
run_remote() {
  run_remote_with_timeout "${test_ssh_timeout_seconds}" "$@"
}

readiness_deadline=$((SECONDS + test_readiness_timeout_seconds))
runner_is_ready=false
while [ "${SECONDS}" -lt "${readiness_deadline}" ]; do
  readiness_remaining=$((readiness_deadline - SECONDS))
  readiness_probe_bound="${test_readiness_probe_timeout_seconds}"
  if [ "${readiness_probe_bound}" -gt "${readiness_remaining}" ]; then
    readiness_probe_bound="${readiness_remaining}"
  fi
  if run_remote_with_timeout "${readiness_probe_bound}" \
    true >/dev/null 2>&1; then
    runner_is_ready=true
    break
  fi
  readiness_remaining=$((readiness_deadline - SECONDS))
  if [ "${readiness_remaining}" -gt 0 ]; then
    readiness_sleep=2
    [ "${readiness_sleep}" -le "${readiness_remaining}" ] \
      || readiness_sleep="${readiness_remaining}"
    sleep "${readiness_sleep}"
  fi
done
[ "${runner_is_ready}" = true ] \
  || die "runner SSH was not ready within ${test_readiness_timeout_seconds} seconds"
# Exit 2 is "done with recoverable errors" — first boot finished, warnings only.
run_remote 'cloud-init status --wait >/dev/null || [ "$?" -eq 2 ]'
run_remote 'test "$(id -u)" -eq 2000 && test "$HOME" = /home/runner'
run_remote '! sudo -n true >/dev/null 2>&1'
run_remote '! id -nG | tr " " "\n" | grep -Eq "^(wheel|root|libvirt|qemu)$"'
run_remote 'forgejo-runner one-job --help >/dev/null'
# The finalizer must not remove what the guest channel depends on, and the
# recorded block list must be the one the agent is actually running with.
run_remote 'set -Eeuo pipefail; test -x /usr/local/libexec/flanforge-guest-ready; test -f /usr/local/share/flanforge/job-account; test -f /usr/local/share/flanforge/qemu-ga-block-rpcs; grep -Fqx runner:2000 /usr/local/share/flanforge/job-account'
run_remote '/usr/local/libexec/flanforge-guest-ready --self-test | jq -e ".schema == 1 and .contract == 2" >/dev/null'
run_remote 'set -Eeuo pipefail; for path in /etc/flanforge /var/lib/flanforge /run/flanforge /home/packer/.ssh/authorized_keys /etc/sudoers.d/90-cloud-init-users /home/runner/.runner /home/runner/.config/forgejo /home/runner/.config/act_runner /home/runner/.config/containers/auth.json /root/.config/containers/auth.json /root/.ssh/authorized_keys /run/libvirt/libvirt-sock /run/podman/podman.sock /dev/kvm; do test ! -e "${path}"; done'
build_remote_dependency_contract
run_remote "${remote_dependency_contract}"
run_remote 'set -Eeuo pipefail; rpm -q docker-cli >/dev/null; ! rpm -q podman-docker >/dev/null 2>&1; ! rpm -q moby-engine >/dev/null 2>&1; file "$(command -v docker)" | grep -Eq "ELF 64-bit.*x86-64"; docker --version | grep -Eq "^Docker version "'
# The privileged account is self-describing through its record; the job
# account can confirm existence but not the sudo grant, which the build proves.
run_remote 'set -Eeuo pipefail; record=/usr/local/share/flanforge/privileged-account; if [ -f "${record}" ]; then IFS=: read -r name uid < "${record}"; test "${name}" = prunner; test "$(id -u prunner)" -eq "${uid}"; fi; test ! -e /tmp/flanforge-privileged-sudo-password'
run_remote 'test "${DOCKER_HOST:-}" = unix:///run/user/2000/podman/podman.sock; test "${CONTAINER_HOST:-}" = unix:///run/user/2000/podman/podman.sock'
build_remote_tailscale_contract
run_remote "${remote_tailscale_contract}"
run_remote 'systemctl --user is-active podman.socket >/dev/null'
run_remote 'test -S /run/user/2000/podman/podman.sock'
run_remote "podman run --rm '${container_image}' true"
run_remote 'set -Eeuo pipefail; docker version --format "{{.Client.Version}} {{.Server.Version}}" | grep -Eq "^[^ ]+ [^ ]+$"'
run_remote "docker run --rm '${container_image}' true"
run_remote "set -Eeuo pipefail; container=\$(buildah from '${container_image}'); trap 'buildah rm \"\${container}\" >/dev/null 2>&1 || true' EXIT; buildah run \"\${container}\" true"
run_remote 'curl --fail --silent --unix-socket /run/user/2000/podman/podman.sock http://d/_ping | grep -Fxq OK'
run_remote 'set -Eeuo pipefail; systemctl --user stop podman.socket podman.service; test ! -S /run/user/2000/podman/podman.sock; ! docker info >/dev/null 2>&1; systemctl --user start podman.socket; systemctl --user is-active podman.socket >/dev/null; docker version >/dev/null'

[ "${agent_checks_ran}" = true ] || die "the guest agent checks did not run"

echo "==> guest host key"
ssh-keygen -l -f "${known_hosts}"
echo "Template clone passed the guest-agent, runner, Tailscale, rootless Podman, and Docker API checks."
test_body_succeeded=true
