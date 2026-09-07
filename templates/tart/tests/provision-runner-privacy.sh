#!/usr/bin/env bash
# Drive the runner privacy provisioner against fake macOS tools. This proves
# the toggle, the validation, and which statements are issued; whether macOS
# honours the resulting database can only be answered on real hardware.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-tart-privacy.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-tart-privacy."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

fake_bin="${test_root}/bin"
mkdir -p "${fake_bin}"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'echo "NFSHomeDirectory: /Users/runner"' > "${fake_bin}/dscl"

cat > "${fake_bin}/sqlite3" <<'FAKE_SQLITE'
#!/usr/bin/env bash
set -Eeuo pipefail
database="$1"
shift
if [ "$#" -gt 0 ]; then statement="$1"; else statement="$(cat)"; fi
printf '%s\n%s\n' "${database}" "${statement}" >> "${FAKE_SQLITE_LOG}"
case "${statement}" in
  .backup*)
    destination="${statement#.backup }"
    destination="${destination#\'}"
    destination="${destination%\'}"
    : > "${destination}"
    ;;
  SELECT*)
    [ -z "${FAKE_SQLITE_NO_GRANTS:-}" ] || exit 0
    printf '%s\n' kTCCServiceAccessibility kTCCServiceScreenCapture \
      kTCCServicePostEvent kTCCServiceAppleEvents
    ;;
esac
FAKE_SQLITE

# Every argument is rewritten into the fake guest root, including the path
# quoted inside sqlite3's `.backup` argument.
cat > "${fake_bin}/sudo" <<'FAKE_SUDO'
#!/usr/bin/env bash
set -Eeuo pipefail
while [ "$#" -gt 0 ]; do
  case "$1" in -H) shift ;; -u) shift 2 ;; *) break ;; esac
done
command_name="$1"
shift
arguments=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o|-g) shift 2 ;;
    *) arguments+=("${1//\/Users\//${FAKE_GUEST_ROOT}/Users/}"); shift ;;
  esac
done
case "${command_name}" in
  chown) exit 0 ;;
  test) test "${arguments[@]}" ;;
  chmod|install|sqlite3) "${command_name}" "${arguments[@]}" ;;
  *) echo "unexpected sudo command: ${command_name}" >&2; exit 2 ;;
esac
FAKE_SUDO

chmod 0700 "${fake_bin}/dscl" "${fake_bin}/sqlite3" "${fake_bin}/sudo"

# The bootstrap account's database is the schema source; a fixture stands in.
make_bootstrap_home() {
  local home="$1"
  mkdir -p "${home}/Library/Application Support/com.apple.TCC"
  printf 'fixture\n' > "${home}/Library/Application Support/com.apple.TCC/TCC.db"
}

run_fixture() {
  local fixture="$1"
  local enabled="$2"
  local guest_root="${test_root}/${fixture}"
  local bootstrap_home="${test_root}/${fixture}-admin"
  local status=0
  mkdir -p "${guest_root}/Users/runner"
  env PATH="${fake_bin}:${PATH}" \
    FAKE_GUEST_ROOT="${guest_root}" \
    FAKE_SQLITE_LOG="${test_root}/${fixture}.sql" \
    FAKE_SQLITE_NO_GRANTS="${3:-}" \
    HOME="${bootstrap_home}" \
    PRIVACY_GRANTS_ENABLED="${enabled}" \
    bash "${template_dir}/scripts/provision-runner-privacy.sh" >/dev/null 2>&1 \
    || status=$?
  return "${status}"
}

make_bootstrap_home "${test_root}/seeded-admin"
run_fixture seeded true
seeded_sql="${test_root}/seeded.sql"
grep -Fq '.backup' "${seeded_sql}"
grep -Fq 'DELETE FROM access;' "${seeded_sql}"
for service in kTCCServiceAccessibility kTCCServiceScreenCapture \
  kTCCServicePostEvent kTCCServiceAppleEvents; do
  grep -Fq "${service}" "${seeded_sql}" \
    || { echo "${service} was never granted to runner" >&2; exit 1; }
done
[ -f "${test_root}/seeded/Users/runner/Library/Application Support/com.apple.TCC/TCC.db" ]

# An account that already has a database keeps it; only the grants are written.
make_bootstrap_home "${test_root}/existing-admin"
mkdir -p "${test_root}/existing/Users/runner/Library/Application Support/com.apple.TCC"
printf 'existing\n' \
  > "${test_root}/existing/Users/runner/Library/Application Support/com.apple.TCC/TCC.db"
run_fixture existing true
if grep -Fq '.backup' "${test_root}/existing.sql"; then
  echo "an existing runner privacy database was overwritten" >&2
  exit 1
fi
grep -Fq 'kTCCServiceAppleEvents' "${test_root}/existing.sql"

make_bootstrap_home "${test_root}/disabled-admin"
run_fixture disabled false
[ ! -e "${test_root}/disabled.sql" ] \
  || { echo "disabled privacy grants still wrote to a database" >&2; exit 1; }

make_bootstrap_home "${test_root}/malformed-admin"
if run_fixture malformed maybe; then
  echo "a malformed privacy toggle was accepted" >&2
  exit 1
fi

# Without the bootstrap database there is no schema to copy, and inventing one
# would produce grants macOS ignores. That must fail loudly.
if run_fixture unseeded true; then
  echo "privacy grants were claimed without a schema source" >&2
  exit 1
fi

# The failure that matters most is the quiet one: writes that store nothing.
make_bootstrap_home "${test_root}/ungranted-admin"
if run_fixture ungranted true no-grants; then
  echo "grants that never landed were reported as stored" >&2
  exit 1
fi

echo "Tart runner privacy provisioning fake tests passed"
