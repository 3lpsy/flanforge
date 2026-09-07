#!/usr/bin/env bash
# Cover the smoke check's guards without a Mac: argument handling, and the
# guest script's refusal to run anywhere but an SSH session as runner. Whether
# the simulator and xcodebuild work can only be answered on real hardware.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-tart-smoke.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-tart-smoke."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

fake_bin="${test_root}/bin"
mkdir -p "${fake_bin}"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'echo runner' > "${fake_bin}/id"
chmod 0700 "${fake_bin}/id"

guest_script="${template_dir}/scripts/smoke-job-path.sh"

bash "${template_dir}/smoke.sh" --help >/dev/null 2>&1 \
  || { echo "smoke.sh rejected --help" >&2; exit 1; }
if bash "${template_dir}/smoke.sh" --bogus >/dev/null 2>&1; then
  echo "smoke.sh accepted an unknown option" >&2
  exit 1
fi
if bash "${template_dir}/smoke.sh" one two >/dev/null 2>&1; then
  echo "smoke.sh accepted two template names" >&2
  exit 1
fi

# The domain difference is the entire point: a run outside an SSH session is
# testing the provisioning session again, so it must refuse rather than pass.
if env -u SSH_CONNECTION PATH="${fake_bin}:${PATH}" \
  bash "${guest_script}" >/dev/null 2>&1; then
  echo "the guest smoke check ran outside an SSH session" >&2
  exit 1
fi

if env SSH_CONNECTION="203.0.113.2 1 203.0.113.3 22" \
  SMOKE_PREWARM_TARGET='iPhone; rm -rf /' \
  PATH="${fake_bin}:${PATH}" \
  bash "${guest_script}" >/dev/null 2>&1; then
  echo "the guest smoke check accepted a malformed simulator name" >&2
  exit 1
fi

if env SSH_CONNECTION="203.0.113.2 1 203.0.113.3 22" \
  SMOKE_BOOT_TIMEOUT_SECONDS=forever \
  PATH="${fake_bin}:${PATH}" \
  bash "${guest_script}" >/dev/null 2>&1; then
  echo "the guest smoke check accepted a malformed timeout" >&2
  exit 1
fi

echo "Tart job-path smoke check guard tests passed"
