#!/usr/bin/env bash
# Identity-bound lifecycle helpers for the destructive clone test.
# shellcheck disable=SC2154 # The entrypoint supplies validated lifecycle state.

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
domain_presence=unknown
volume_presence=unknown

run_test_virsh() {
  run_test_virsh_with_timeout "${test_rpc_timeout_seconds}" "$@"
}

run_test_virsh_with_timeout() {
  local maximum_seconds="$1"
  shift
  run_bounded "${maximum_seconds}" \
    env LC_ALL=C virsh --connect "${libvirt_uri}" "$@"
}

run_test_ssh_with_timeout() {
  local maximum_seconds="$1"
  shift
  run_bounded "${maximum_seconds}" ssh "$@"
}

remember_volume() {
  local volume_name="$1"
  local output_variable="$2"
  local volume_key
  if ! volume_key="$(run_test_virsh vol-key --pool "${test_pool}" \
    "${volume_name}")" || [ -z "${volume_key}" ]; then
    return 1
  fi
  [[ "${volume_key}" != *$'\n'* && "${volume_key}" != *$'\r'* ]] || return 1
  test_volume_names+=("${volume_name}")
  test_volume_keys+=("${volume_key}")
  printf -v "${output_variable}" '%s' "${volume_key}"
}

retain_untracked_volume() {
  local volume_name="$1"
  untracked_volume_names+=("${volume_name}")
  pending_volume_name=""
  echo "error: retaining ${volume_name}; its immutable volume key was not captured" >&2
  echo "error: inspect the dedicated test pool and recover it manually" >&2
}

create_volume() {
  local volume_name="$1"
  local capacity="$2"
  local format="$3"
  local output_variable="$4"
  local recorded_key=""
  [[ "${output_variable}" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] \
    || die "invalid volume-key output variable"
  pending_volume_name="${volume_name}"
  if ! run_test_virsh vol-create-as --pool "${test_pool}" --name "${volume_name}" \
    --capacity "${capacity}" --format "${format}" >/dev/null; then
    if determine_volume_name_presence "${volume_name}" \
      && [ "${volume_presence}" = absent ]; then
      pending_volume_name=""
    fi
    die "could not create test volume ${volume_name}"
  fi
  if ! remember_volume "${volume_name}" recorded_key; then
    retain_untracked_volume "${volume_name}"
    die "could not record the identity of test volume ${volume_name}"
  fi
  pending_volume_name=""
  printf -v "${output_variable}" '%s' "${recorded_key}"
}

create_overlay() {
  local volume_name="$1"
  local capacity="$2"
  local backing_volume_key="$3"
  local recorded_key=""
  pending_volume_name="${volume_name}"
  if ! run_test_virsh vol-create-as --pool "${test_pool}" --name "${volume_name}" \
    --capacity "${capacity}" --format qcow2 \
    --backing-vol "${backing_volume_key}" --backing-vol-format qcow2 >/dev/null; then
    if determine_volume_name_presence "${volume_name}" \
      && [ "${volume_presence}" = absent ]; then
      pending_volume_name=""
    fi
    die "could not create test overlay ${volume_name}"
  fi
  if ! remember_volume "${volume_name}" recorded_key; then
    retain_untracked_volume "${volume_name}"
    die "could not record the identity of test overlay ${volume_name}"
  fi
  pending_volume_name=""
}

wait_for_guest_address() {
  local mac_address="$1"
  local deadline=$((SECONDS + 180))
  local address=""
  while [ "${SECONDS}" -lt "${deadline}" ]; do
    address="$(run_test_virsh domifaddr "${test_domain_uuid}" \
      --source lease 2>/dev/null \
      | awk -v mac="${mac_address}" '$2 == mac && $3 == "ipv4" { sub(/\/.*/, "", $4); print $4; exit }')"
    if [ -n "${address}" ]; then
      printf '%s\n' "${address}"
      return 0
    fi
    sleep 2
  done
  return 1
}

# One bounded guest-agent command. The reply is capped here as well as by the
# agent, so a broken or hostile guest cannot fill the runner through virsh.
run_test_agent_command() {
  local command="$1"
  run_test_virsh_with_timeout "${test_rpc_timeout_seconds}" \
    qemu-agent-command --timeout "${test_agent_probe_timeout_seconds}" \
    "${test_domain_uuid}" "${command}" 2>/dev/null | head -c 65536
}

wait_for_guest_agent() {
  local deadline=$((SECONDS + test_agent_timeout_seconds))
  while [ "${SECONDS}" -lt "${deadline}" ]; do
    if run_test_agent_command '{"execute":"guest-ping"}' | grep -Fq '"return"'; then
      return 0
    fi
    sleep 2
  done
  return 1
}

# Runs one command through guest-exec and prints its guest-exec-status reply.
# The pid is the agent's, so only this same domain can interpret it. A failed
# virsh returns from here rather than aborting the test around it, and never
# becomes a pid: `if ! x="$(...)"` is a condition, so `set -e` does not fire.
run_test_agent_exec() {
  local request="$1"
  local started="" reply="" pid=""
  if ! started="$(run_test_agent_command "${request}")"; then
    return 1
  fi
  pid="$(printf '%s' "${started}" | jq -r '.return.pid // empty' 2>/dev/null)" || return 1
  [[ "${pid}" =~ ^[1-9][0-9]*$ ]] || return 1
  local deadline=$((SECONDS + test_agent_timeout_seconds))
  while [ "${SECONDS}" -lt "${deadline}" ]; do
    if ! reply="$(run_test_agent_command \
      "$(jq -cn --argjson pid "${pid}" \
        '{execute: "guest-exec-status", arguments: {pid: $pid}}')")"; then
      return 1
    fi
    if printf '%s' "${reply}" | jq -e '.return.exited == true' >/dev/null 2>&1; then
      printf '%s\n' "${reply}"
      return 0
    fi
    sleep 1
  done
  return 1
}

mark_cleanup_failure() {
  cleanup_had_failure=true
  echo "warning: $1" >&2
}

is_domain_state_active() {
  local state="${1,,}"
  case "${state}" in
    "shut off"|crashed) return 1 ;;
    *) return 0 ;;
  esac
}

remove_test_work_dir() {
  [ -n "${test_work_dir:-}" ] || return 0
  if [[ "${test_work_dir}" != "${TMPDIR:-/tmp}/flanforge-libvirt-test."* ]] \
    || [ ! -d "${test_work_dir}" ] || [ -L "${test_work_dir}" ]; then
    mark_cleanup_failure "refusing to remove unexpected test directory ${test_work_dir}"
    return 1
  fi
  if ! find "${test_work_dir}" -depth -delete 2>/dev/null; then
    mark_cleanup_failure "could not remove private test directory ${test_work_dir}"
    return 1
  fi
}

determine_domain_presence_by_uuid() {
  local diagnostics
  domain_presence=unknown
  if diagnostics="$(run_test_virsh dominfo "${test_domain_uuid}" \
    2>&1 >/dev/null)"; then
    domain_presence=present
    return 0
  fi
  diagnostics="${diagnostics:0:4096}"
  if printf '%s\n' "${diagnostics}" | grep -Eq '^error: Domain not found:'; then
    domain_presence=absent
    return 0
  fi
  return 1
}

is_storage_volume_not_found() {
  local diagnostics="$1"
  printf '%s\n' "${diagnostics}" \
    | grep -Eq '^error: Storage volume not found:'
}

determine_volume_name_presence() {
  local volume_name="$1"
  local diagnostics
  volume_presence=unknown
  if diagnostics="$(run_test_virsh vol-info --pool "${test_pool}" \
    "${volume_name}" 2>&1 >/dev/null)"; then
    volume_presence=present
    return 0
  fi
  diagnostics="${diagnostics:0:4096}"
  if is_storage_volume_not_found "${diagnostics}"; then
    volume_presence=absent
    return 0
  fi
  return 1
}

determine_volume_key_presence() {
  local expected_key="$1"
  local diagnostics
  volume_presence=unknown
  if diagnostics="$(run_test_virsh vol-info "${expected_key}" \
    2>&1 >/dev/null)"; then
    volume_presence=present
    return 0
  fi
  diagnostics="${diagnostics:0:4096}"
  if is_storage_volume_not_found "${diagnostics}"; then
    volume_presence=absent
    return 0
  fi
  return 1
}

cleanup_domain_by_uuid() {
  local state=""
  local still_active=false

  if ! determine_domain_presence_by_uuid; then
    mark_cleanup_failure "cannot determine whether domain UUID ${test_domain_uuid} exists"
    return 1
  fi
  if [ "${domain_presence}" = absent ]; then
    return 0
  fi

  if ! state="$(run_test_virsh domstate \
    "${test_domain_uuid}" 2>/dev/null)"; then
    mark_cleanup_failure "could not read state for domain UUID ${test_domain_uuid}"
    return 1
  fi

  if is_domain_state_active "${state}"; then
    if ! run_test_virsh shutdown \
      "${test_domain_uuid}" >/dev/null 2>&1; then
      mark_cleanup_failure "could not request shutdown for domain UUID ${test_domain_uuid}"
    fi
    still_active=true
    for _ in $(seq 1 15); do
      if ! state="$(run_test_virsh domstate \
        "${test_domain_uuid}" 2>/dev/null)"; then
        mark_cleanup_failure "lost state for domain UUID ${test_domain_uuid} during shutdown"
        return 1
      fi
      if ! is_domain_state_active "${state}"; then
        still_active=false
        break
      fi
      sleep 2
    done
    if [ "${still_active}" = true ] && ! run_test_virsh \
      destroy "${test_domain_uuid}" >/dev/null 2>&1; then
      mark_cleanup_failure "could not destroy domain UUID ${test_domain_uuid}"
      return 1
    fi
  fi

  if ! run_test_virsh undefine \
    "${test_domain_uuid}" >/dev/null 2>&1; then
    mark_cleanup_failure "could not undefine domain UUID ${test_domain_uuid}"
    return 1
  fi
  if ! determine_domain_presence_by_uuid \
    || [ "${domain_presence}" != absent ]; then
    mark_cleanup_failure "could not prove domain UUID ${test_domain_uuid} was undefined"
    return 1
  fi
}

is_keep_identity_verified() {
  local current_uuid
  [ "${keep_clone}" = true ] \
    && [ "${test_body_succeeded}" = true ] \
    && [ "${test_domain_defined}" = true ] \
    && [ "${test_domain_identity_verified}" = true ] || return 1
  current_uuid="$(run_test_virsh domuuid \
    "${test_domain_uuid}" 2>/dev/null)" || return 1
  [ "${current_uuid}" = "${test_domain_uuid}" ]
}

print_kept_inventory() {
  local index
  echo "==> keeping identity-verified test domain and volumes"
  echo "    shell key: ${test_identity}"
  printf '    '
  printf '%q ' timeout --foreground --kill-after=5s \
    "${test_ssh_timeout_seconds}s" ssh "${ssh_options[@]}" \
    "runner@${guest_address:-<address>}"
  printf '\n'
  printf '    '
  printf '%q ' timeout --foreground --kill-after=5s \
    "${test_rpc_timeout_seconds}s" env LC_ALL=C virsh \
    --connect "${libvirt_uri}" destroy "${test_domain_uuid}"
  printf '\n'
  printf '    '
  printf '%q ' timeout --foreground --kill-after=5s \
    "${test_rpc_timeout_seconds}s" env LC_ALL=C virsh \
    --connect "${libvirt_uri}" undefine "${test_domain_uuid}"
  printf '\n'
  echo "    volume keys in pool '${test_pool}':"
  for ((index=${#test_volume_keys[@]} - 1; index >= 0; index--)); do
    printf '    '
    printf '%q ' timeout --foreground --kill-after=5s \
      "${test_rpc_timeout_seconds}s" env LC_ALL=C virsh \
      --connect "${libvirt_uri}" vol-delete --pool "${test_pool}" \
      "${test_volume_keys[index]}"
    printf '# %s\n' "${test_volume_names[index]}"
  done
}

cleanup_volumes_by_key() {
  local index
  local volume_cleanup_failed=false
  local volume_key
  for ((index=${#test_volume_keys[@]} - 1; index >= 0; index--)); do
    volume_key="${test_volume_keys[index]}"
    if ! determine_volume_key_presence "${volume_key}"; then
      mark_cleanup_failure "cannot determine whether volume key ${volume_key} exists"
      volume_cleanup_failed=true
      break
    fi
    [ "${volume_presence}" = present ] || continue
    if ! run_test_virsh vol-delete "${volume_key}" >/dev/null 2>&1; then
      mark_cleanup_failure "could not delete volume key ${volume_key}"
      volume_cleanup_failed=true
      break
    fi
    if ! determine_volume_key_presence "${volume_key}" \
      || [ "${volume_presence}" != absent ]; then
      mark_cleanup_failure "could not prove volume key ${volume_key} was deleted"
      volume_cleanup_failed=true
      break
    fi
  done
  [ "${volume_cleanup_failed}" = false ]
}

cleanup_test_resources() {
  local untracked_name
  cleanup_had_failure=false

  if is_keep_identity_verified; then
    print_kept_inventory
    return 0
  fi

  if [ "${test_domain_definition_attempted}" = true ] \
    && ! cleanup_domain_by_uuid; then
    echo "warning: retaining test volumes because domain cleanup failed" >&2
  else
    cleanup_volumes_by_key || true
  fi

  for untracked_name in "${untracked_volume_names[@]}"; do
    mark_cleanup_failure \
      "volume ${untracked_name} needs manual recovery; no immutable key was captured"
  done
  if [ -n "${pending_volume_name}" ]; then
    mark_cleanup_failure \
      "volume ${pending_volume_name} has an unknown creation outcome and needs manual inspection"
  fi
  remove_test_work_dir || true
  [ "${cleanup_had_failure}" = false ]
}

on_test_exit() {
  local body_status=$?
  local cleanup_status=0
  trap - EXIT
  trap '' HUP INT TERM
  cleanup_test_resources || cleanup_status=$?
  if [ "${body_status}" -ne 0 ]; then
    exit "${body_status}"
  fi
  exit "${cleanup_status}"
}
