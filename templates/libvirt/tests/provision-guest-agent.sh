#!/usr/bin/env bash
# Exercise the qemu-ga block-list rewrite against hostile sysconfig fixtures.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-guest-agent-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-guest-agent-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

provisioner="${template_dir}/scripts/provision-guest-agent.sh"
helper_source="${template_dir}/guest/flanforge-guest-ready"
recycle_source="${template_dir}/guest/flanforge-guest-recycle"
fake_bin="${test_root}/bin"
command_log="${test_root}/commands"
mkdir -p "${fake_bin}"

readonly FEDORA_BLOCK_LIST='guest-file-open,guest-file-close,guest-file-read,guest-file-write,guest-file-seek,guest-file-flush,guest-exec,guest-exec-status'
readonly RETAINED_BLOCK_LIST='guest-file-open,guest-file-close,guest-file-read,guest-file-write,guest-file-seek,guest-file-flush'

# sudo is the only privileged verb these scripts use, so it is recorded and
# executed unprivileged against the fake guest root.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'printf "sudo %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'while [ "${1:-}" = -H ] || [ "${1:-}" = -u ]; do' \
  '  if [ "$1" = -u ]; then shift 2; else shift; fi' \
  'done' \
  'case "$1" in' \
  '  install|tee|chmod|chown|rm|test) exec "$@" ;;' \
  '  *) echo "unexpected sudo command: $1" >&2; exit 2 ;;' \
  'esac' > "${fake_bin}/sudo"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'printf "systemctl %s\n" "$*" >> "${FAKE_COMMAND_LOG}"' \
  'if [ "${1:-}" = cat ]; then printf "%s\n" "${FAKE_UNIT_TEXT}"; fi' \
  'exit 0' > "${fake_bin}/systemctl"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'exit 0' > "${fake_bin}/chown"

# An unprivileged test cannot own a fixture as root, so ownership is answered
# from FAKE_STAT_OWNER and every other format comes from the real stat.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'if [ "${1:-}" = -c ] && [ "${2:-}" = "%U:%G" ]; then' \
  '  printf "%s\n" "${FAKE_STAT_OWNER:-root:root}"' \
  '  exit 0' \
  'fi' \
  'exec /usr/bin/stat "$@"' > "${fake_bin}/stat"

# Likewise -o/-g: the modes and the destination still come from real install.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -Eeuo pipefail' \
  'arguments=()' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in' \
  '    -o|-g) shift 2 ;;' \
  '    *) arguments+=("$1"); shift ;;' \
  '  esac' \
  'done' \
  'exec /usr/bin/install "${arguments[@]}"' > "${fake_bin}/install"
chmod 0755 "${fake_bin}"/*

die() { echo "guest-agent provisioner test: $1" >&2; exit 1; }

# Each case gets its own guest root, so no test can observe another's writes.
prepare_case() {
  local name="$1" block_line="$2" extra_line="${3:-}"
  case_root="${test_root}/${name}"
  mkdir -p "${case_root}/etc/sysconfig" "${case_root}/share" \
    "${case_root}/libexec" "${case_root}/tmp"
  sysconfig="${case_root}/etc/sysconfig/qemu-ga"
  {
    printf '%s\n' '# This is a systemd environment file, not a shell script.'
    printf '%s\n' 'DAEMON=/usr/bin/qemu-ga'
    [ -z "${block_line}" ] || printf '%s\n' "${block_line}"
    [ -z "${extra_line}" ] || printf '%s\n' "${extra_line}"
    printf '%s\n' 'FSFREEZE_HOOK_PATHNAME=/etc/qemu-ga/fsfreeze-hook'
  } > "${sysconfig}"
  chmod 0644 "${sysconfig}"
  cp "${helper_source}" "${case_root}/tmp/flanforge-guest-ready"
  cp "${recycle_source}" "${case_root}/tmp/flanforge-guest-recycle"
  : > "${command_log}"
}

run_case() {
  env PATH="${fake_bin}:${PATH}" \
    FAKE_COMMAND_LOG="${command_log}" \
    FAKE_STAT_OWNER="${FAKE_STAT_OWNER:-root:root}" \
    FAKE_UNIT_TEXT="${FAKE_UNIT_TEXT:-ExecStart=/usr/bin/qemu-ga --block-rpcs=\${BLOCK_RPCS}}" \
    FLANFORGE_QEMU_GA_SYSCONFIG="${sysconfig}" \
    FLANFORGE_SHARE_DIR="${case_root}/share" \
    FLANFORGE_LIBEXEC_DIR="${case_root}/libexec" \
    FLANFORGE_STAGED_HELPER="${case_root}/tmp/flanforge-guest-ready" \
    FLANFORGE_STAGED_RECYCLE="${case_root}/tmp/flanforge-guest-recycle" \
    bash "${provisioner}"
}

# 1. The Fedora default: exactly two entries removed, six retained in order.
FAKE_UNIT_TEXT='ExecStart=/usr/bin/qemu-ga --block-rpcs=${BLOCK_RPCS}'
prepare_case default "BLOCK_RPCS=${FEDORA_BLOCK_LIST}"
run_case >/dev/null || die "the default fixture was refused"
grep -Fqx "BLOCK_RPCS=${RETAINED_BLOCK_LIST}" "${sysconfig}" \
  || die "the retained block list is wrong: $(grep BLOCK_RPCS "${sysconfig}")"
grep -Fqx '# This is a systemd environment file, not a shell script.' "${sysconfig}" \
  || die "an unrelated line was rewritten"
grep -Fqx 'FSFREEZE_HOOK_PATHNAME=/etc/qemu-ga/fsfreeze-hook' "${sysconfig}" \
  || die "an unrelated line was dropped"
[ "$(stat -c '%a' "${sysconfig}")" = 644 ] || die "the file mode changed"
grep -Fqx "${RETAINED_BLOCK_LIST}" "${case_root}/share/qemu-ga-block-rpcs" \
  || die "the effective block list was not recorded"
grep -Fqx 'runner:2000' "${case_root}/share/job-account" \
  || die "the job account was not recorded"
[ -x "${case_root}/libexec/flanforge-guest-ready" ] \
  || die "the readiness helper was not installed"
[ ! -e "${case_root}/tmp/flanforge-guest-ready" ] \
  || die "the staged helper was retained"
# The recycle gate is baked the same way the readiness helper is, so the daemon
# runs one named command per platform and knows nothing about either.
[ -x "${case_root}/libexec/flanforge-guest-recycle" ] \
  || die "the recycle helper was not installed"
[ ! -e "${case_root}/tmp/flanforge-guest-recycle" ] \
  || die "the staged recycle helper was retained"

# 2. Idempotent: a second run over its own output changes nothing.
before="$(cat "${sysconfig}")"
cp "${helper_source}" "${case_root}/tmp/flanforge-guest-ready"
cp "${recycle_source}" "${case_root}/tmp/flanforge-guest-recycle"
run_case >/dev/null || die "the second run was refused"
[ "$(cat "${sysconfig}")" = "${before}" ] || die "the rewrite is not idempotent"

# 3. The legacy variable name is discovered rather than hardcoded.
FAKE_UNIT_TEXT='ExecStart=/usr/bin/qemu-ga --blacklist=${BLACKLIST_RPC}'
prepare_case legacy "BLACKLIST_RPC=${FEDORA_BLOCK_LIST}"
run_case >/dev/null || die "the legacy fixture was refused"
grep -Fqx "BLACKLIST_RPC=${RETAINED_BLOCK_LIST}" "${sysconfig}" \
  || die "the legacy rewrite is wrong"

# 3b. QEMU >= 9.1 packaging: $QEMU_GA_ARGS with no active assignment (the
# shipped file only carries a commented example), and an ExecStartPre that
# names BLACKLIST_RPC literally without consuming it. The list is imposed.
QEMU_GA_UNIT_TEXT='ExecStartPre=/bin/sh -c "if grep '"'"'^BLACKLIST_RPC'"'"' /etc/sysconfig/qemu-ga >/dev/null ; then sed -i s/x/y/ /etc/sysconfig/qemu-ga ; fi"
ExecStart=/usr/bin/qemu-ga --method=virtio-serial -F${FSFREEZE_HOOK_PATHNAME} $QEMU_GA_ARGS'
FAKE_UNIT_TEXT="${QEMU_GA_UNIT_TEXT}"
prepare_case args-default "" "#QEMU_GA_ARGS=--block-rpcs=${FEDORA_BLOCK_LIST}"
run_case >/dev/null || die "the QEMU_GA_ARGS default fixture was refused"
grep -Fqx "QEMU_GA_ARGS=--block-rpcs=${RETAINED_BLOCK_LIST}" "${sysconfig}" \
  || die "the imposed QEMU_GA_ARGS assignment is wrong"
grep -Fqx "#QEMU_GA_ARGS=--block-rpcs=${FEDORA_BLOCK_LIST}" "${sysconfig}" \
  || die "the commented example was rewritten"
grep -Fqx "${RETAINED_BLOCK_LIST}" "${case_root}/share/qemu-ga-block-rpcs" \
  || die "the imposed block list was not recorded"
before="$(cat "${sysconfig}")"
cp "${helper_source}" "${case_root}/tmp/flanforge-guest-ready"
cp "${recycle_source}" "${case_root}/tmp/flanforge-guest-recycle"
run_case >/dev/null || die "the second QEMU_GA_ARGS run was refused"
[ "$(cat "${sysconfig}")" = "${before}" ] \
  || die "the QEMU_GA_ARGS rewrite is not idempotent"

# 3c. An active flag assignment is edited in place, exec entries removed.
prepare_case args-active "QEMU_GA_ARGS=--block-rpcs=${FEDORA_BLOCK_LIST}"
run_case >/dev/null || die "the active QEMU_GA_ARGS fixture was refused"
grep -Fqx "QEMU_GA_ARGS=--block-rpcs=${RETAINED_BLOCK_LIST}" "${sysconfig}" \
  || die "the active QEMU_GA_ARGS rewrite is wrong"

# 3d. Arguments beyond --block-rpcs, or leftover legacy assignments the unit
# no longer reads, are refused rather than guessed at.
prepare_case args-foreign "QEMU_GA_ARGS=--verbose --block-rpcs=${FEDORA_BLOCK_LIST}"
if run_case >/dev/null 2>&1; then
  die "QEMU_GA_ARGS with foreign arguments was accepted"
fi
prepare_case args-legacy-cruft "QEMU_GA_ARGS=--block-rpcs=${FEDORA_BLOCK_LIST}" \
  "BLOCK_RPCS=${FEDORA_BLOCK_LIST}"
if run_case >/dev/null 2>&1; then
  die "an unconsumed legacy assignment beside QEMU_GA_ARGS was accepted"
fi

# 4. A unit that does not consume the variable fails before writing.
FAKE_UNIT_TEXT='ExecStart=/usr/bin/qemu-ga --method=virtio-serial'
prepare_case unconsumed "BLOCK_RPCS=${FEDORA_BLOCK_LIST}"
before="$(cat "${sysconfig}")"
if run_case >/dev/null 2>&1; then
  die "a unit that ignores the variable was accepted"
fi
[ "$(cat "${sysconfig}")" = "${before}" ] || die "the fixture was written before the check"
[ ! -e "${case_root}/share/qemu-ga-block-rpcs" ] || die "a record was written anyway"

# 5. Absent, ambiguous, and unsafe values are refused.
FAKE_UNIT_TEXT='ExecStart=/usr/bin/qemu-ga --block-rpcs=${BLOCK_RPCS}'
refuses() {
  if run_case >/dev/null 2>&1; then
    die "$1"
  fi
}

prepare_case absent ""
refuses "an absent block-list variable was accepted"
prepare_case ambiguous "BLOCK_RPCS=${FEDORA_BLOCK_LIST}" "BLACKLIST_RPC=${FEDORA_BLOCK_LIST}"
refuses "two block-list variables were accepted"
prepare_case repeated "BLOCK_RPCS=${FEDORA_BLOCK_LIST}" "BLOCK_RPCS=guest-exec"
refuses "a repeated assignment was accepted"
for unsafe in 'BLOCK_RPCS=guest-exec; touch /tmp/pwn' 'BLOCK_RPCS=Guest-Exec' \
  'BLOCK_RPCS=guest exec'; do
  prepare_case unsafe "${unsafe}"
  refuses "an unsafe value was accepted: ${unsafe}"
done

# 6. A non-empty allow-list would override the block-list entirely.
prepare_case allowlist "BLOCK_RPCS=${FEDORA_BLOCK_LIST}" 'ALLOW_RPCS=guest-ping'
refuses "a non-empty ALLOW_RPCS was accepted"
prepare_case allowlist-empty "BLOCK_RPCS=${FEDORA_BLOCK_LIST}" 'ALLOW_RPCS='
run_case >/dev/null || die "an empty ALLOW_RPCS was refused"

# 7. A symlinked or non-root sysconfig is refused before anything reads it.
prepare_case symlink "BLOCK_RPCS=${FEDORA_BLOCK_LIST}"
mv "${sysconfig}" "${sysconfig}.real"
ln -s "${sysconfig}.real" "${sysconfig}"
refuses "a symlinked sysconfig was accepted"
prepare_case foreign-owner "BLOCK_RPCS=${FEDORA_BLOCK_LIST}"
FAKE_STAT_OWNER=packer:packer
refuses "a sysconfig another account owns was accepted"
FAKE_STAT_OWNER=root:root

# 8. The service is never restarted: Packer attaches no guest-agent port.
FAKE_UNIT_TEXT='ExecStart=/usr/bin/qemu-ga --block-rpcs=${BLOCK_RPCS}'
prepare_case norestart "BLOCK_RPCS=${FEDORA_BLOCK_LIST}"
run_case >/dev/null || die "the no-restart fixture was refused"
if grep -Eq 'systemctl (restart|start|reload)' "${command_log}"; then
  die "the guest agent was restarted during the build"
fi

# 9. Static cross-checks: the build runs the provisioner before verification,
# and nothing later deletes what it baked.
packer_file="${template_dir}/flanforge-base.pkr.hcl"
grep -Fq 'guest/flanforge-guest-ready' "${packer_file}" \
  || die "the packer template does not upload the readiness helper"
grep -Fq 'guest/flanforge-guest-recycle' "${packer_file}" \
  || die "the packer template does not upload the recycle helper"
guest_agent_line="$(grep -n 'provision-guest-agent.sh' "${packer_file}" | head -n 1 | cut -d: -f1)"
verify_line="$(grep -n 'provision-verify.sh' "${packer_file}" | head -n 1 | cut -d: -f1)"
[ -n "${guest_agent_line}" ] && [ -n "${verify_line}" ] \
  && [ "${guest_agent_line}" -lt "${verify_line}" ] \
  || die "provision-guest-agent.sh must run before provision-verify.sh"
if grep -Eq 'flanforge-guest-ready|flanforge-guest-recycle|qemu-ga-block-rpcs|job-account' \
  "${template_dir}/scripts/finalize.sh"; then
  die "the finalizer deletes what the guest channel depends on"
fi
generalize_source="${template_dir}/../../crates/flanforge-runtime-libvirt/src/warm/generalize.rs"
[ -f "${generalize_source}" ] || die "the warm generalization source is missing"
if ! generalize_text="$(cat -- "${generalize_source}")"; then
  die "the warm generalization source is unreadable"
fi
if grep -Eq 'flanforge-guest-ready|/etc/sysconfig|/usr/local/share/flanforge' \
  <<< "${generalize_text}"; then
  die "warm identity generalization targets guest-channel state"
fi

echo "guest-agent provisioner fake tests passed"
