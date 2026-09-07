#!/usr/bin/env bash
# Select and enforce one shared, non-overlapping subordinate-ID range.

subordinate_id_range_size=65536
subordinate_id_first_candidate=100000
subordinate_id_last_candidate=4294901760

subordinate_id_error() {
  echo "subordinate ID error: $1" >&2
  return 1
}

validate_subordinate_id_file() {
  local file="$1"
  [ -f "${file}" ] && [ ! -L "${file}" ] \
    || { subordinate_id_error "${file} must be a regular file"; return 1; }
  if [ -s "${file}" ] && [ -n "$(tail -c 1 "${file}")" ]; then
    subordinate_id_error "${file} must end with a newline"
    return 1
  fi
  awk -F: '
    /^[[:space:]]*($|#)/ { next }
    NF != 3 { status = 1 }
    $1 !~ /^[A-Za-z0-9_.][A-Za-z0-9_.-]*[$]?$/ { status = 1 }
    $2 !~ /^[0-9]+$/ || $3 !~ /^[0-9]+$/ { status = 1 }
    $3 == 0 || $2 + $3 > 4294967296 { status = 1 }
    END { exit status }
  ' "${file}" || { subordinate_id_error "${file} has an invalid entry"; return 1; }
}

is_subordinate_id_range_free() {
  local start="$1"
  local count="$2"
  local excluded_user="$3"
  local subuid_file="$4"
  local subgid_file="$5"
  awk -F: -v candidate_start="${start}" -v candidate_count="${count}" \
    -v excluded_user="${excluded_user}" '
    /^[[:space:]]*($|#)/ || $1 == excluded_user { next }
    {
      candidate_end = candidate_start + candidate_count
      entry_end = $2 + $3
      if (candidate_start < entry_end && $2 < candidate_end) {
        exit 1
      }
    }
  ' "${subuid_file}" "${subgid_file}"
}

read_subordinate_id_ranges() {
  local user="$1"
  local file="$2"
  awk -F: -v user="${user}" '$1 == user { print $2 ":" $3 }' "${file}"
}

append_subordinate_id_line() {
  local file="$1"
  local entry="$2"
  printf '%s\n' "${entry}" >> "${file}"
}

rollback_subordinate_id_append() {
  local file="$1"
  local original_size="$2"
  local entry="$3"
  local current_size
  local expected_size=$((original_size + ${#entry} + 1))

  current_size="$(stat -c '%s' "${file}")" || return 1
  [ "${current_size}" -eq "${original_size}" ] && return 0
  [ "${current_size}" -eq "${expected_size}" ] || return 1
  [ "$(tail -n 1 "${file}")" = "${entry}" ] || return 1
  truncate -s "${original_size}" "${file}"
}

select_subordinate_id_range() {
  local user="$1"
  local subuid_file="$2"
  local subgid_file="$3"
  local candidate
  local uid_count
  local gid_count
  local uid_range
  local gid_range
  local start
  local count

  [[ "${user}" =~ ^[A-Za-z_][A-Za-z0-9_.-]*[$]?$ ]] \
    || { subordinate_id_error "invalid user name"; return 1; }
  validate_subordinate_id_file "${subuid_file}" || return 1
  validate_subordinate_id_file "${subgid_file}" || return 1

  uid_count="$(read_subordinate_id_ranges "${user}" "${subuid_file}" | wc -l)"
  gid_count="$(read_subordinate_id_ranges "${user}" "${subgid_file}" | wc -l)"
  if [ "${uid_count}" -eq 1 ] && [ "${gid_count}" -eq 1 ]; then
    uid_range="$(read_subordinate_id_ranges "${user}" "${subuid_file}")"
    gid_range="$(read_subordinate_id_ranges "${user}" "${subgid_file}")"
    [ "${uid_range}" = "${gid_range}" ] \
      || { subordinate_id_error "${user} has mismatched UID and GID ranges"; return 1; }
    IFS=: read -r start count <<< "${uid_range}"
    [ "${count}" -eq "${subordinate_id_range_size}" ] \
      || { subordinate_id_error "${user} range must contain 65536 IDs"; return 1; }
    [ "${start}" -ge "${subordinate_id_first_candidate}" ] \
      && [ "${start}" -le "${subordinate_id_last_candidate}" ] \
      || { subordinate_id_error "${user} range is outside the safe host-ID window"; return 1; }
    is_subordinate_id_range_free "${start}" "${count}" "${user}" \
      "${subuid_file}" "${subgid_file}" \
      || { subordinate_id_error "${user} range overlaps another allocation"; return 1; }
    printf 'existing:%s:%s\n' "${start}" "${count}"
    return 0
  fi

  if [ "${uid_count}" -ne 0 ] || [ "${gid_count}" -ne 0 ]; then
    subordinate_id_error "${user} must have exactly one matching range in both files"
    return 1
  fi

  candidate="${subordinate_id_first_candidate}"
  while [ "${candidate}" -le "${subordinate_id_last_candidate}" ]; do
    if is_subordinate_id_range_free "${candidate}" \
      "${subordinate_id_range_size}" "${user}" "${subuid_file}" "${subgid_file}"; then
      printf 'new:%s:%s\n' "${candidate}" "${subordinate_id_range_size}"
      return 0
    fi
    candidate=$((candidate + subordinate_id_range_size))
  done
  subordinate_id_error "no shared 65536-ID range is available"
}

ensure_subordinate_id_range() {
  local user="$1"
  local subuid_file="$2"
  local subgid_file="$3"
  local lock_file="${FLANFORGE_SUBORDINATE_ID_LOCK_FILE:-/run/lock/flanforge-subordinate-ids.lock}"
  local mode
  local entry
  local original_subgid_size
  local original_subuid_size
  local rollback_failed=false
  local start
  local count
  local selection
  local verified

  [[ "${lock_file}" = /* && "${lock_file}" != *$'\n'* && "${lock_file}" != *$'\r'* ]] \
    || { subordinate_id_error "lock path must be an absolute single-line path"; return 1; }
  exec 9> "${lock_file}" \
    || { subordinate_id_error "cannot open allocation lock"; return 1; }
  flock -x 9 || { subordinate_id_error "cannot acquire allocation lock"; return 1; }

  selection="$(select_subordinate_id_range "${user}" \
    "${subuid_file}" "${subgid_file}")" || return 1
  IFS=: read -r mode start count <<< "${selection}"
  if [ "${mode}" = new ]; then
    entry="${user}:${start}:${count}"
    original_subuid_size="$(stat -c '%s' "${subuid_file}")" || return 1
    original_subgid_size="$(stat -c '%s' "${subgid_file}")" || return 1
    if ! append_subordinate_id_line "${subuid_file}" "${entry}"; then
      if ! rollback_subordinate_id_append \
        "${subuid_file}" "${original_subuid_size}" "${entry}"; then
        subordinate_id_error "failed subuid append could not be rolled back safely"
        return 1
      fi
      subordinate_id_error "cannot update ${subuid_file}"
      return 1
    fi
    if ! append_subordinate_id_line "${subgid_file}" "${entry}"; then
      if ! rollback_subordinate_id_append \
        "${subgid_file}" "${original_subgid_size}" "${entry}"; then
        rollback_failed=true
      fi
      if ! rollback_subordinate_id_append \
        "${subuid_file}" "${original_subuid_size}" "${entry}"; then
        rollback_failed=true
      fi
      if [ "${rollback_failed}" = true ]; then
        subordinate_id_error "paired allocation could not be rolled back safely"
        return 1
      fi
      subordinate_id_error "cannot update ${subgid_file}; paired update was rolled back"
      return 1
    fi
  fi

  if ! verified="$(select_subordinate_id_range "${user}" \
    "${subuid_file}" "${subgid_file}")" \
    || [ "${verified}" != "existing:${start}:${count}" ]; then
    if [ "${mode}" = new ]; then
      rollback_failed=false
      if ! rollback_subordinate_id_append \
        "${subgid_file}" "${original_subgid_size}" "${entry}"; then
        rollback_failed=true
      fi
      if ! rollback_subordinate_id_append \
        "${subuid_file}" "${original_subuid_size}" "${entry}"; then
        rollback_failed=true
      fi
      if [ "${rollback_failed}" = true ]; then
        subordinate_id_error "failed verification could not be rolled back safely"
        return 1
      fi
    fi
    subordinate_id_error "allocation verification failed"
    return 1
  fi
  printf '%s\n' "${verified}"
}
