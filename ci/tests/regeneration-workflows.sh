#!/usr/bin/env bash
# Validate every tracked warm-image producer's marker handoff.
set -Eeuo pipefail

die() {
  echo "error: $*" >&2
  exit 1
}

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${project_root}"

specs=(
  'examples/regeneration.yml|apple-build'
  'examples/regeneration-libvirt.yml|linux-build'
  '.forgejo/workflows/libvirt-warm.yml|linux-analysis'
)
sentinel='.flanforge-regeneration-complete'

job_body() {
  local workflow="$1" job="$2"
  awk -v header="  ${job}:" '
    $0 == header { emit = 1; next }
    emit && $0 ~ /^  [A-Za-z0-9_-]+:$/ { exit }
    emit { print }
  ' "${workflow}"
}

step_block() {
  local ordinal="$1"
  awk -v wanted="${ordinal}" '
    /^      - / {
      count += 1
      if (count > wanted) exit
    }
    count == wanted { print }
  '
}

step_script() {
  awk '
    $0 == "        run: |" { emit = 1; next }
    emit && $0 == "" { blanks += 1; next }
    emit && $0 ~ /^          / {
      while (blanks > 0) {
        print ""
        blanks -= 1
      }
      sub(/^          /, "")
      print
      next
    }
    emit { exit }
  '
}

expected="$({
  for spec in "${specs[@]}"; do
    printf '%s\n' "${spec%%|*}"
  done
} | sort)"
actual="$({
  while IFS= read -r workflow; do
    case "${workflow}" in
      *.yml|*.yaml) ;;
      *) continue ;;
    esac
    if grep -Fq "${sentinel}" "${workflow}" \
      || grep -Fq 'regeneration_workflow' "${workflow}"; then
      printf '%s\n' "${workflow}"
    fi
  done < <(git ls-files -- .forgejo/workflows examples)
} | sort)"
[ "${actual}" = "${expected}" ] \
  || die "tracked regeneration producers changed; expected [${expected}], found [${actual}]"

test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-regeneration-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-regeneration-tests."* ]]; then
    chmod -R u+w "${test_root}" 2>/dev/null || true
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

reset_script="${test_root}/reset.sh"
completion_script="${test_root}/complete.sh"
credential_scripts=()
credential_workflows=()
producer_index=0
for spec in "${specs[@]}"; do
  workflow="${spec%%|*}"
  job="${spec##*|}"
  body="$(job_body "${workflow}" "${job}")"
  [ -n "${body}" ] || die "${workflow}: producer job ${job} was not found"
  count="$(grep -Ec '^      - ' <<< "${body}")"
  [ "${count}" -ge 2 ] || die "${workflow}: producer has fewer than two steps"

  first="$(step_block 1 <<< "${body}")"
  last="$(step_block "${count}" <<< "${body}")"
  grep -Fqx '      - name: Reset inherited completion marker' <<< "${first}" \
    || die "${workflow}: marker reset is not the literal first producer step"
  grep -Fqx '      - name: Mark the regeneration complete' <<< "${last}" \
    || die "${workflow}: completion handoff is not the literal last producer step"
  ! grep -Eq '^        (if|continue-on-error):' <<< "${first}" \
    || die "${workflow}: marker reset may not override normal step failure"
  ! grep -Eq '^        (if|continue-on-error):' <<< "${last}" \
    || die "${workflow}: completion handoff must run only after success"

  candidate_reset="${test_root}/reset-${producer_index}.sh"
  candidate_completion="${test_root}/complete-${producer_index}.sh"
  step_script <<< "${first}" > "${candidate_reset}"
  step_script <<< "${last}" > "${candidate_completion}"
  [ -s "${candidate_reset}" ] || die "${workflow}: reset is not a run block"
  [ -s "${candidate_completion}" ] || die "${workflow}: completion is not a run block"
  if [ "${producer_index}" -eq 0 ]; then
    cp "${candidate_reset}" "${reset_script}"
    cp "${candidate_completion}" "${completion_script}"
  else
    cmp -s "${candidate_reset}" "${reset_script}" \
      || die "${workflow}: marker reset drifted from the shared contract"
    cmp -s "${candidate_completion}" "${completion_script}" \
      || die "${workflow}: completion handoff drifted from the shared contract"
  fi

  for ((step = 2; step < count; step += 1)); do
    block="$(step_block "${step}" <<< "${body}")"
    intermediate="$(step_script <<< "${block}")"
    ! grep -Fq "${sentinel}" <<< "${intermediate}" \
      || die "${workflow}: intermediate step ${step} mutates the completion marker"
    if grep -Eq '^[[:space:]]+uses: .*checkout@' <<< "${block}"; then
      grep -Fqx '          persist-credentials: false' <<< "${block}" \
        || die "${workflow}: guest checkout persists credentials into the retained image"
    fi
    if grep -Fqx '      - name: Refuse to bake credentials into the image' <<< "${block}"; then
      credential_script="${test_root}/credentials-${producer_index}.sh"
      step_script <<< "${block}" > "${credential_script}"
      # Match literal extracted workflow source.
      # shellcheck disable=SC2016
      grep -Fq 'if ! hit="$(find ' "${credential_script}" \
        || die "${workflow}: credential traversal does not fail closed"
      grep -Fq -- '-print -quit)' "${credential_script}" \
        || die "${workflow}: credential traversal does not stop without a pipeline"
      ! grep -Fq '2>/dev/null' "${credential_script}" \
        || die "${workflow}: credential traversal hides diagnostics"
      credential_scripts+=("${credential_script}")
      credential_workflows+=("${workflow}")
    fi
  done
  producer_index=$((producer_index + 1))
done
[ "${#credential_scripts[@]}" -eq 2 ] \
  || die "expected credential scans in both public regeneration examples"

trap_line="$(grep -nFx 'trap cleanup EXIT' "${completion_script}" | cut -d: -f1)"
# Match literal extracted workflow source.
# shellcheck disable=SC2016
first_check_line="$(grep -nF 'if [ -e "$marker" ] || [ -L "$marker" ]; then' \
  "${completion_script}" | cut -d: -f1)"
[ -n "${trap_line}" ] && [ -n "${first_check_line}" ] \
  && [ "${trap_line}" -lt "${first_check_line}" ] \
  || die 'completion cleanup trap is not installed before the first handoff check'
# shellcheck disable=SC2016
grep -Fq 'if [ "$committed" -ne 1 ]; then' "${completion_script}" \
  || die 'completion cleanup is not guarded by an explicit commit flag'
[ "$(awk 'NF { last = $0 } END { print last }' "${completion_script}")" = 'committed=1' ] \
  || die 'completion handoff does not commit immediately before shell exit'
! grep -Fq 'trap - EXIT' "${completion_script}" \
  || die 'completion handoff disables cleanup before shell exit'

run_guest_script() {
  local script="$1" home="$2" path="${3:-${PATH}}"
  BASH_COMPAT=32 HOME="${home}" PATH="${path}" /bin/bash "${script}"
}

prepare_handoff_failure() {
  local home="$1"
  mkdir -p "${home}"
  printf 'preserved\n' > "${home}/unrelated"
}

assert_failed_handoff_is_safe() {
  local label="$1" home="$2" marker="${2}/${sentinel}"
  # This is the daemon's acceptance predicate. Every failed final step must
  # leave it false, regardless of which filesystem entry remains.
  if [ -f "${marker}" ] && [ ! -L "${marker}" ]; then
    die "${label}: daemon would accept a marker after final-step failure"
  fi
  grep -Fqx preserved "${home}/unrelated" \
    || die "${label}: cleanup changed an unrelated file"
  [ -z "$(find "${home}" -maxdepth 1 -name "${sentinel}.tmp.*" -print -quit)" ] \
    || die "${label}: cleanup left its private temporary file behind"
}

# Every possible pre-handoff failure starts after reset, so a stale marker
# cannot authorize retention even when checkout or later work fails.
producer_index=0
for spec in "${specs[@]}"; do
  workflow="${spec%%|*}"
  job="${spec##*|}"
  body="$(job_body "${workflow}" "${job}")"
  count="$(grep -Ec '^      - ' <<< "${body}")"
  for ((failed_step = 2; failed_step < count; failed_step += 1)); do
    home="${test_root}/failure-${producer_index}-${failed_step}"
    mkdir -p "${home}"
    printf 'inherited\n' > "${home}/${sentinel}"
    run_guest_script "${reset_script}" "${home}" >/dev/null 2>&1 \
      || die "${workflow}: reset rejected a stale regular marker"
    [ ! -e "${home}/${sentinel}" ] && [ ! -L "${home}/${sentinel}" ] \
      || die "${workflow}: stale marker survived failure before step ${failed_step}"
  done
  producer_index=$((producer_index + 1))
done

home="${test_root}/reset-symlink"
mkdir -p "${home}"
printf 'protected\n' > "${home}/target"
ln -s "${home}/target" "${home}/${sentinel}"
if run_guest_script "${reset_script}" "${home}" >/dev/null 2>&1; then
  die 'reset accepted a symlink marker'
fi
if [ ! -L "${home}/${sentinel}" ] || ! grep -Fqx protected "${home}/target"; then
  die 'reset changed a symlink marker or its target'
fi

home="${test_root}/reset-directory"
mkdir -p "${home}/${sentinel}"
if run_guest_script "${reset_script}" "${home}" >/dev/null 2>&1; then
  die 'reset accepted a non-regular marker'
fi
[ -d "${home}/${sentinel}" ] || die 'reset changed a non-regular marker'

home="${test_root}/complete-clean"
mkdir -p "${home}"
printf 'preserved\n' > "${home}/unrelated"
run_guest_script "${completion_script}" "${home}" >/dev/null 2>&1 \
  || die 'final handoff did not create a marker'
marker="${home}/${sentinel}"
[ -f "${marker}" ] && [ ! -L "${marker}" ] \
  || die 'final handoff did not create a regular non-symlink marker'
[ "$(stat -c '%a' "${marker}")" = 600 ] \
  || die 'final handoff did not create the marker with mode 0600'
grep -Fqx preserved "${home}/unrelated" \
  || die 'successful final handoff changed an unrelated file'
[ -z "$(find "${home}" -maxdepth 1 -name "${sentinel}.tmp.*" -print -quit)" ] \
  || die 'successful final handoff left its private temporary file behind'

home="${test_root}/complete-regular"
prepare_handoff_failure "${home}"
printf 'reappeared\n' > "${home}/${sentinel}"
if run_guest_script "${completion_script}" "${home}" >/dev/null 2>&1; then
  die 'final handoff accepted a reappeared regular marker'
fi
assert_failed_handoff_is_safe 'reappeared regular marker' "${home}"
[ ! -e "${home}/${sentinel}" ] \
  || die 'final handoff did not remove a reappeared regular marker'

home="${test_root}/complete-symlink"
prepare_handoff_failure "${home}"
printf 'protected\n' > "${home}/target"
ln -s "${home}/target" "${home}/${sentinel}"
if run_guest_script "${completion_script}" "${home}" >/dev/null 2>&1; then
  die 'final handoff accepted a reappeared symlink marker'
fi
assert_failed_handoff_is_safe 'reappeared symlink marker' "${home}"
[ ! -e "${home}/${sentinel}" ] && [ ! -L "${home}/${sentinel}" ] \
  || die 'final handoff did not remove a reappeared symlink marker'
grep -Fqx protected "${home}/target" \
  || die 'final handoff cleanup followed a reappeared marker symlink'

home="${test_root}/complete-directory"
prepare_handoff_failure "${home}"
mkdir -p "${home}/${sentinel}"
printf 'protected\n' > "${home}/${sentinel}/target"
if run_guest_script "${completion_script}" "${home}" >/dev/null 2>&1; then
  die 'final handoff accepted a reappeared non-regular marker'
fi
assert_failed_handoff_is_safe 'reappeared marker directory' "${home}"
[ -d "${home}/${sentinel}" ] || die 'final handoff changed a reappeared directory'
grep -Fqx protected "${home}/${sentinel}/target" \
  || die 'final handoff cleanup changed a reappeared marker directory'

# Recreate the marker after the early absence check. The hard-link publish
# must fail, and cleanup must remove both the uncommitted marker and temp file.
home="${test_root}/complete-race"
prepare_handoff_failure "${home}"
fake_bin="${test_root}/fake-ln-bin"
mkdir -p "${fake_bin}"
# Write a literal child script.
# shellcheck disable=SC2016
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "reappeared\\n" > "$2"' \
  'exec /usr/bin/ln "$@"' > "${fake_bin}/ln"
chmod 0700 "${fake_bin}/ln"
if run_guest_script "${completion_script}" "${home}" \
  "${fake_bin}:${PATH}" >/dev/null 2>&1; then
  die 'final handoff replaced a marker recreated during publication'
fi
assert_failed_handoff_is_safe 'publication race' "${home}"
[ ! -e "${home}/${sentinel}" ] \
  || die 'final handoff did not remove the marker recreated during publication'

# Fail after ln has published the hard link. The cleanup trap must revoke that
# otherwise valid marker, while leaving files outside its two targets alone.
home="${test_root}/complete-post-link"
prepare_handoff_failure "${home}"
fake_bin="${test_root}/fake-chmod-bin"
mkdir -p "${fake_bin}"
# Write a literal child script.
# shellcheck disable=SC2016
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'marker="${2%.tmp.*}"' \
  '[ -f "$marker" ] && [ ! -L "$marker" ] || exit 24' \
  'printf "after-link\n" > "$HOME/post-link-witness"' \
  'exit 23' > "${fake_bin}/chmod"
chmod 0700 "${fake_bin}/chmod"
if run_guest_script "${completion_script}" "${home}" \
  "${fake_bin}:${PATH}" >/dev/null 2>&1; then
  die 'final handoff accepted an injected failure after publication'
fi
grep -Fqx after-link "${home}/post-link-witness" \
  || die 'post-link failure injection ran before marker publication'
assert_failed_handoff_is_safe 'injected post-link failure' "${home}"
[ ! -e "${home}/${sentinel}" ] \
  || die 'final handoff did not revoke the marker after a post-link failure'

run_credential_script() {
  local script="$1" fixture="$2" path="${3:-${PATH}}"
  mkdir -p "${fixture}/home" "${fixture}/work" \
    "${fixture}/workspace" "${fixture}/tmp"
  BASH_COMPAT=32 HOME="${fixture}/home" WORK_DIR="${fixture}/work" \
    GITHUB_WORKSPACE="${fixture}/workspace" TMPDIR="${fixture}/tmp" \
    PATH="${path}" /bin/bash "${script}"
}

for ((index = 0; index < ${#credential_scripts[@]}; index += 1)); do
  credential_script="${credential_scripts[index]}"
  workflow="${credential_workflows[index]}"

  fixture="${test_root}/credential-clean-${index}"
  run_credential_script "${credential_script}" "${fixture}" >/dev/null 2>&1 \
    || die "${workflow}: credential scan rejected clean readable trees"

  fixture="${test_root}/credential-hit-${index}"
  mkdir -p "${fixture}/work"
  printf 'secret\n' > "${fixture}/work/.env"
  if run_credential_script "${credential_script}" "${fixture}" >/dev/null 2>&1; then
    die "${workflow}: credential scan accepted a matching file"
  fi

  fixture="${test_root}/credential-error-${index}"
  fake_find="${fixture}/bin"
  mkdir -p "${fake_find}"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'echo "simulated find failure" >&2' \
    'exit 23' > "${fake_find}/find"
  chmod 0700 "${fake_find}/find"
  if run_credential_script "${credential_script}" "${fixture}" \
    "${fake_find}:${PATH}" >/dev/null 2>&1; then
    die "${workflow}: credential scan masked a find failure"
  fi
done

echo 'regeneration workflow marker contract tests passed'
