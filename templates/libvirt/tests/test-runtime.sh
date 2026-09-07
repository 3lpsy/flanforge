#!/usr/bin/env bash
# Exercise destructive lifecycle helpers without a libvirt connection.
# shellcheck disable=SC2034 # Sourced lifecycle helpers consume fixture state.
set -Eeuo pipefail
export LC_ALL=C

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../host/common.sh
source "${template_dir}/host/common.sh"
# shellcheck source=../host/test-runtime.sh
source "${template_dir}/host/test-runtime.sh"

test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-runtime-tests.XXXXXX")"
fixture_work_dir="${TMPDIR:-/tmp}/flanforge-libvirt-test.unit.$$"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-runtime-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
  if [[ "${fixture_work_dir}" == "${TMPDIR:-/tmp}/flanforge-libvirt-test."* ]]; then
    find "${fixture_work_dir}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

fake_bin="${test_root}/bin"
fake_log="${test_root}/virsh.log"
fake_state="${test_root}/virsh-state"
fake_timeout_log="${test_root}/timeout.log"
mkdir -p "${fake_bin}"
export FAKE_VIRSH_LOG="${fake_log}"
export FAKE_VIRSH_STATE="${fake_state}"
export FAKE_TIMEOUT_LOG="${fake_timeout_log}"
export PATH="${fake_bin}:${PATH}"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -u' \
  'printf "%s\n" "$*" >> "${FAKE_TIMEOUT_LOG}"' \
  'if [ -n "${FAKE_TIMEOUT_FAIL_MATCH:-}" ] && [[ " $* " == *" ${FAKE_TIMEOUT_FAIL_MATCH} "* ]]; then' \
  '  exit 124' \
  'fi' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in --foreground|--kill-after=*) shift ;; *) break ;; esac' \
  'done' \
  '[ "$#" -ge 2 ] || exit 2' \
  'shift' \
  'exec "$@"' > "${fake_bin}/timeout"
chmod 0700 "${fake_bin}/timeout"

write_fake_virsh() {
  local scenario="$1"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -u' \
    'printf "LC_ALL=%s %s\n" "${LC_ALL:-}" "$*" >> "${FAKE_VIRSH_LOG}"' \
    'command_name="${3:-}"' \
    "scenario='${scenario}'" \
    'case "${scenario}:${command_name}" in' \
    '  create-failed:vol-create-as) exit 1 ;;' \
    '  create-failed:vol-info) echo "error: permission denied" >&2; exit 1 ;;' \
    '  identity:vol-create-as) exit 0 ;;' \
    '  identity:vol-key) exit 1 ;;' \
    '  cleanup:dominfo) if [ -e "${FAKE_VIRSH_STATE}.domain-deleted" ]; then echo "error: Domain not found: fixture" >&2; exit 1; fi; exit 0 ;;' \
    '  cleanup:domstate) printf "%s\n" "shut off"; exit 0 ;;' \
    '  cleanup:undefine) touch "${FAKE_VIRSH_STATE}.domain-deleted"; exit 0 ;;' \
    '  cleanup:vol-info) if [ -e "${FAKE_VIRSH_STATE}.volume-deleted" ]; then echo "error: Storage volume not found: fixture" >&2; exit 1; fi; exit 0 ;;' \
    '  cleanup:vol-delete) touch "${FAKE_VIRSH_STATE}.volume-deleted"; exit 0 ;;' \
    '  undefine:dominfo) exit 0 ;;' \
    '  undefine:domstate) printf "%s\n" "shut off"; exit 0 ;;' \
    '  undefine:undefine) exit 1 ;;' \
    '  domain-unknown:dominfo) echo "error: permission denied" >&2; exit 1 ;;' \
    '  domain-unknown:list) exit 0 ;;' \
    '  domain-unknown:uri) exit 0 ;;' \
    '  domain-absent:dominfo) echo "error: Domain not found: fixture" >&2; exit 1 ;;' \
    '  domain-absent:vol-info) if [ -e "${FAKE_VIRSH_STATE}.volume-deleted" ]; then echo "error: Storage volume not found: fixture" >&2; exit 1; fi; exit 0 ;;' \
    '  domain-absent:vol-delete) touch "${FAKE_VIRSH_STATE}.volume-deleted"; exit 0 ;;' \
    '  volume-unknown:vol-info) echo "error: permission denied" >&2; exit 1 ;;' \
    '  volume-unknown:uri) exit 0 ;;' \
    '  keep:domuuid) printf "%s\n" "${4}"; exit 0 ;;' \
    '  agent:qemu-agent-command) printf "%s\n" "${FAKE_AGENT_REPLY:-}"; exit "${FAKE_AGENT_EXIT:-0}" ;;' \
    'esac' \
    'exit 0' > "${fake_bin}/virsh"
  chmod 0700 "${fake_bin}/virsh"
  : > "${fake_log}"
  : > "${fake_timeout_log}"
  find "${test_root}" -maxdepth 1 -type f -name 'virsh-state.*' -delete
}

reset_runtime() {
  libvirt_uri=qemu:///system
  test_rpc_timeout_seconds=7
  test_agent_timeout_seconds=1
  test_agent_probe_timeout_seconds=2
  test_transfer_timeout_seconds=19
  test_ssh_timeout_seconds=11
  test_pool=flanforge-test
  test_name=flanforge-test-unit
  keep_clone=false
  test_domain_uuid=""
  test_domain_definition_attempted=false
  test_domain_defined=false
  test_domain_identity_verified=false
  test_body_succeeded=false
  test_volume_names=()
  test_volume_keys=()
  untracked_volume_names=()
  pending_volume_name=""
  cleanup_had_failure=false
  test_work_dir="${fixture_work_dir}"
  test_identity="${test_work_dir}/key"
  guest_address=""
  known_hosts="${test_work_dir}/known hosts"
  test_host_key_alias=flanforge-libvirt-test-guest
  ssh_options=(
    -F /dev/null
    -o GlobalKnownHostsFile=/dev/null
    -o BatchMode=yes
    -o IdentitiesOnly=yes
    -o StrictHostKeyChecking=yes
    -o HostKeyAlias="${test_host_key_alias}"
    -o UserKnownHostsFile="${known_hosts}"
    -i "${test_identity}"
  )
  mkdir -p "${test_work_dir}"
}

write_fake_virsh identity
reset_runtime
if identity_output="$(
  (trap cleanup_test_resources EXIT; \
    create_volume flanforge-test-volume 1024B qcow2 fixture_key) 2>&1
)"; then
  die "identity lookup failure unexpectedly succeeded"
fi
[[ "${identity_output}" == *'no immutable key was captured'* ]] \
  || die "identity lookup failure did not report manual recovery"
if grep -q 'vol-delete' "${fake_log}"; then
  die "identity lookup failure deleted a volume by its reusable name"
fi

write_fake_virsh create-failed
reset_runtime
if create_output="$(
  (trap cleanup_test_resources EXIT; \
    create_volume flanforge-test-volume 1024B qcow2 fixture_key) 2>&1
)"; then
  die "failed volume creation unexpectedly succeeded"
fi
[[ "${create_output}" == *'unknown creation outcome'* ]] \
  || die "unknown volume creation outcome lost its recovery marker"
if grep -q 'vol-delete' "${fake_log}"; then
  die "failed volume creation deleted an unowned name collision"
fi

write_fake_virsh identity
reset_runtime
if overlay_output="$(
  (trap cleanup_test_resources EXIT; \
    create_overlay flanforge-test-overlay 1024B immutable-backing-key) 2>&1
)"; then
  die "overlay identity lookup failure unexpectedly succeeded"
fi
[[ "${overlay_output}" == *'no immutable key was captured'* ]] \
  || die "overlay identity failure lost its manual-recovery marker"

write_fake_virsh cleanup
reset_runtime
test_domain_uuid=11111111-1111-1111-1111-111111111111
test_domain_definition_attempted=true
test_domain_defined=true
test_domain_identity_verified=true
test_volume_names=(flanforge-test-volume)
test_volume_keys=(immutable-volume-key)
cleanup_test_resources || die "identity-bound cleanup unexpectedly failed"
grep -Fq 'domstate 11111111-1111-1111-1111-111111111111' "${fake_log}" \
  || die "domain state was not queried by UUID"
grep -Fq 'undefine 11111111-1111-1111-1111-111111111111' "${fake_log}" \
  || die "domain was not undefined by UUID"
grep -Fq 'vol-delete immutable-volume-key' "${fake_log}" \
  || die "volume was not deleted by immutable key"
if grep -Eq '(domstate|shutdown|destroy|undefine|vol-delete) .*flanforge-test-unit' \
  "${fake_log}"; then
  die "cleanup used a reusable resource name"
fi

write_fake_virsh domain-unknown
reset_runtime
test_domain_uuid=11111111-1111-1111-1111-111111111111
test_domain_definition_attempted=true
test_volume_names=(flanforge-test-volume)
test_volume_keys=(immutable-volume-key)
if cleanup_test_resources >/dev/null 2>&1; then
  die "ambiguous domain lookup reported successful cleanup"
fi
if grep -q 'vol-delete' "${fake_log}"; then
  die "ambiguous domain lookup deleted a dependent volume"
fi
if grep -q 'list --all --uuid' "${fake_log}"; then
  die "ambiguous domain lookup trusted ACL-filtered enumeration"
fi

write_fake_virsh domain-absent
reset_runtime
test_domain_uuid=11111111-1111-1111-1111-111111111111
test_domain_definition_attempted=true
test_volume_names=(flanforge-test-volume)
test_volume_keys=(immutable-volume-key)
cleanup_test_resources || die "canonical domain absence blocked volume cleanup"
grep -Fq 'vol-delete immutable-volume-key' "${fake_log}" \
  || die "canonical domain absence did not release dependent volume"

write_fake_virsh volume-unknown
reset_runtime
test_volume_names=(flanforge-test-volume)
test_volume_keys=(immutable-volume-key)
if cleanup_test_resources >/dev/null 2>&1; then
  die "ambiguous volume lookup reported successful cleanup"
fi
if grep -q 'vol-delete' "${fake_log}"; then
  die "ambiguous volume lookup attempted deletion"
fi

write_fake_virsh cleanup
reset_runtime
test_domain_uuid=11111111-1111-1111-1111-111111111111
test_domain_definition_attempted=true
test_volume_names=(flanforge-test-volume)
test_volume_keys=(immutable-volume-key)
FAKE_TIMEOUT_FAIL_MATCH=virsh
export FAKE_TIMEOUT_FAIL_MATCH
if cleanup_test_resources >/dev/null 2>&1; then
  die "timed-out libvirt lookup reported successful cleanup"
fi
unset FAKE_TIMEOUT_FAIL_MATCH
if grep -q 'vol-delete' "${fake_log}"; then
  die "timed-out domain lookup deleted a dependent volume"
fi
grep -Fq -- '--kill-after=5s 7s env LC_ALL=C virsh' "${fake_timeout_log}" \
  || die "libvirt calls did not use the configured GNU timeout bound"

FAKE_TIMEOUT_FAIL_MATCH=ssh
export FAKE_TIMEOUT_FAIL_MATCH
if run_test_ssh_with_timeout 11 -V >/dev/null 2>&1; then
  die "timed-out SSH command unexpectedly succeeded"
fi
unset FAKE_TIMEOUT_FAIL_MATCH
grep -Fq -- '--kill-after=5s 11s ssh -V' "${fake_timeout_log}" \
  || die "SSH calls did not use the configured overall timeout bound"

write_fake_virsh undefine
reset_runtime
test_domain_uuid=11111111-1111-1111-1111-111111111111
test_domain_definition_attempted=true
test_domain_defined=true
test_volume_names=(flanforge-test-volume)
test_volume_keys=(immutable-volume-key)
if cleanup_test_resources >/dev/null 2>&1; then
  die "failed domain undefine reported successful cleanup"
fi
if grep -q 'vol-delete' "${fake_log}"; then
  die "failed domain undefine allowed volume deletion"
fi
[ ! -e "${test_work_dir}" ] || die "failed cleanup retained local test keys"

write_fake_virsh undefine
reset_runtime
test_domain_uuid=11111111-1111-1111-1111-111111111111
test_domain_definition_attempted=true
test_domain_defined=true
test_body_succeeded=true
if (on_test_exit) >/dev/null 2>&1; then
  die "cleanup failure after a successful body returned zero"
fi

write_fake_virsh undefine
reset_runtime
test_domain_uuid=11111111-1111-1111-1111-111111111111
test_domain_definition_attempted=true
test_domain_defined=true
set +e
(set +e; (exit 42); on_test_exit) >/dev/null 2>&1
final_status=$?
set -e
[ "${final_status}" -eq 42 ] || die "cleanup replaced the original body failure"

write_fake_virsh keep
reset_runtime
keep_clone=true
test_domain_uuid=11111111-1111-1111-1111-111111111111
test_domain_defined=true
test_domain_identity_verified=true
test_body_succeeded=true
guest_address=192.0.2.10
test_identity="${fixture_work_dir}/key with ' quote"
known_hosts="${fixture_work_dir}/known hosts with ' quote"
ssh_options=(
  -F /dev/null
  -o GlobalKnownHostsFile=/dev/null
  -o BatchMode=yes
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=yes
  -o HostKeyAlias="${test_host_key_alias}"
  -o UserKnownHostsFile="${known_hosts}"
  -i "${test_identity}"
)
test_volume_names=(first-volume second-volume)
test_volume_keys=(first-key "second key with ' quote")
keep_output="$(cleanup_test_resources)" || die "verified keep unexpectedly failed"
[ -d "${test_work_dir}" ] || die "verified keep removed the local SSH identity"
if grep -Eq ' (destroy|undefine|vol-delete) ' "${fake_log}"; then
  die "verified keep destroyed test inventory"
fi
keep_commands="${test_root}/keep-commands.sh"
printf '%s\n' "${keep_output}" | sed -n 's/^    \(timeout .*\)$/\1/p' \
  > "${keep_commands}"
[ "$(wc -l < "${keep_commands}")" -eq 5 ] \
  || die "verified keep did not print SSH and complete cleanup commands"
bash -n "${keep_commands}" || die "verified keep printed invalid shell commands"
grep -Fq 'StrictHostKeyChecking=yes' "${keep_commands}" \
  || die "kept SSH command omitted strict host-key verification"
grep -Fq 'HostKeyAlias=flanforge-libvirt-test-guest' "${keep_commands}" \
  || die "kept SSH command omitted its authenticated host alias"
grep -Fq 'UserKnownHostsFile=' "${keep_commands}" \
  || die "kept SSH command omitted its private known_hosts file"
mapfile -t keep_command_lines < "${keep_commands}"
[[ "${keep_command_lines[3]}" == *'vol-delete --pool flanforge-test'*'# second-volume' ]] \
  || die "kept volume cleanup is not in reverse dependency order"
[[ "${keep_command_lines[4]}" == *'vol-delete --pool flanforge-test first-key'*'# first-volume' ]] \
  || die "kept volume cleanup omitted the first volume key"

if grep -v '^LC_ALL=C ' "${fake_log}" | grep -q .; then
  die "a parsed fake libvirt invocation did not force the C locale"
fi

grep -Fq -- '--uuid "${test_domain_uuid}"' "${template_dir}/test.sh" \
  || die "clone test does not assign its domain UUID before define"
grep -Fq 'StrictHostKeyChecking=yes' "${template_dir}/test.sh" \
  || die "clone test does not require a known SSH host key"
grep -Fq 'HostKeyAlias="${test_host_key_alias}"' "${template_dir}/test.sh" \
  || die "clone test does not use its fixed SSH host alias"
grep -Fq 'ssh_keys:' "${template_dir}/test.sh" \
  || die "clone test does not inject its generated SSH host key"
if grep -Fq 'accept-new' "${template_dir}/test.sh"; then
  die "clone test still trusts an SSH host key on first use"
fi

# The guest-agent helpers: a timed-out, malformed, in-band-error, or oversized
# reply must never be read as a success, because the daemon's whole guest
# contract is carried over exactly these calls.
write_fake_virsh agent
reset_runtime
test_domain_uuid=00000000-0000-4000-8000-000000000001

# Exported explicitly: the fake reads them from its own environment, and an
# assignment that never reached it would make every case below pass vacuously.
agent_reply() {
  export FAKE_AGENT_REPLY="$1" FAKE_AGENT_EXIT="${2:-0}"
}

agent_reply '{"return":{}}'
wait_for_guest_agent || die "a live guest agent was not detected"
grep -Fq 'qemu-agent-command --timeout 2' "${fake_log}" \
  || die "the agent probe did not carry its own bounded timeout"

agent_reply '' 1
wait_for_guest_agent && die "an unreachable guest agent was reported as ready"
agent_reply 'not json'
wait_for_guest_agent && die "a malformed reply was reported as ready"
agent_reply '{"error":{"class":"CommandNotFound","desc":"blocked"}}'
wait_for_guest_agent && die "an in-band error was reported as ready"

agent_reply "$(printf '{"return":{"pad":"%s"}}' "$(head -c 70000 /dev/zero | tr '\0' a)")"
agent_oversized="$(run_test_agent_command '{"execute":"guest-ping"}')"
[ "${#agent_oversized}" -le 65536 ] || die "an oversized agent reply was not capped"
[ "${#agent_oversized}" -gt 1024 ] || die "the oversized fixture never reached the helper"

# An unusable pid must not be handed back to the agent at all: polling one is
# how a status probe ends up interrogating whatever process now holds it.
for unusable in '{"return":{"pid":0}}' '{"return":{"pid":-1}}' '{"return":{}}' '{"return":{"pid":"x"}}'; do
  agent_reply "${unusable}"
  : > "${fake_log}"
  run_test_agent_exec '{"execute":"guest-exec"}' \
    && die "an unusable pid was accepted as a started command: ${unusable}"
  if grep -Fq 'guest-exec-status' "${fake_log}"; then
    die "an unusable pid was still polled: ${unusable}"
  fi
done
agent_reply '{"return":{"pid":42,"exited":false}}'
run_test_agent_exec '{"execute":"guest-exec"}' \
  && die "a command that never exited was reported as finished"
# A failed virsh has to return from the helper, not abort the test around it.
agent_reply '{"return":{"pid":42,"exited":true,"exitcode":0}}' 1
run_test_agent_exec '{"execute":"guest-exec"}' \
  && die "an unreachable agent was reported as a finished command"
agent_reply '{"return":{"pid":42,"exited":true,"exitcode":7}}'
agent_finished="$(run_test_agent_exec '{"execute":"guest-exec"}')" \
  || die "a finished command was not reported"
printf '%s' "${agent_finished}" | jq -e '.return.exitcode == 7' >/dev/null \
  || die "the finished command's exit code was lost"
unset FAKE_AGENT_REPLY FAKE_AGENT_EXIT

# The clone test must keep the virtio channel the agent needs, and must keep
# asserting the contract the daemon refuses to run without.
grep -Fq 'target.name=org.qemu.guest_agent.0' "${template_dir}/test.sh" \
  || die "clone test no longer attaches the guest agent channel"
grep -Fq 'guest-file-open' "${template_dir}/test.sh" \
  || die "clone test no longer proves the file RPCs stay blocked"
grep -Fq 'RuntimeMaxSec=5' "${template_dir}/test.sh" \
  || die "clone test no longer proves the guest-side deadline"

echo "libvirt destructive-helper tests passed"
