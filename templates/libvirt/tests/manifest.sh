#!/usr/bin/env bash
# Compare the emitted manifest contract with its checked-in golden fixture.
set -Eeuo pipefail

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../host/common.sh
source "${template_dir}/host/common.sh"
# shellcheck source=../host/manifest.sh
source "${template_dir}/host/manifest.sh"
# shellcheck source=../host/dependencies.sh
source "${template_dir}/host/dependencies.sh"
# shellcheck source=../host/provenance.sh
source "${template_dir}/host/provenance.sh"

test_root="$(mktemp -d "${TMPDIR:-/tmp}/flanforge-manifest-tests.XXXXXX")"
cleanup() {
  if [[ "${test_root}" == "${TMPDIR:-/tmp}/flanforge-manifest-tests."* ]]; then
    find "${test_root}" -depth -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

fake_bin="${test_root}/bin"
output_directory="${test_root}/output"
mkdir -p "${fake_bin}" "${output_directory}" "${test_root}/not-a-repository"
printf '%s\n' '#!/usr/bin/env bash' 'printf "%s\n" "Packer v1.15.4"' \
  > "${fake_bin}/packer"
chmod 0700 "${fake_bin}/packer"
PATH="${fake_bin}:${PATH}"
export PATH

fixture="${template_dir}/tests/fixtures/image-manifest.json"
image_path="${test_root}/flanforge-base.qcow2"
guest_manifest="${test_root}/guest.json"
provenance_manifest="${test_root}/provenance.json"
printf 'fixture-image\n' > "${image_path}"
jq '.guest' "${fixture}" > "${guest_manifest}"
jq '.provenance' "${fixture}" > "${provenance_manifest}"

image_info='{"virtual-size":42949672960}'
qemu_plugin_version="$(read_qemu_plugin_version \
  "${template_dir}/flanforge-base.pkr.hcl")"
manifest_result="$(write_image_manifest "${image_path}" "${image_info}" \
  "${output_directory}" flanforge-base \
  https://example.invalid/fedora.qcow2 \
  aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  "${guest_manifest}" "${test_root}/not-a-repository" \
  "${provenance_manifest}" "${qemu_plugin_version}")"
IFS=$'\t' read -r manifest_path image_sha256 <<< "${manifest_result}"
[ "${image_sha256}" = 4bd5099b11c4c1aafbbd85c9e5b18b4a891516a7386b4a7997f37983fa181b13 ] \
  || die "manifest writer returned an unexpected image digest"

jq --arg created_at '2026-01-02T03:04:05Z' \
  '.created_at = $created_at' "${manifest_path}" | jq -S . \
  > "${test_root}/actual.json"
jq -S . "${fixture}" > "${test_root}/expected.json"
cmp "${test_root}/expected.json" "${test_root}/actual.json" \
  || die "emitted image manifest differs from the golden contract"

[ "$(jq -r '.builder.qemu_plugin_version' "${fixture}")" = \
  "${qemu_plugin_version}" ] \
  || die "golden manifest QEMU version differs from the Packer constraint"

missing_constraint="${test_root}/missing-qemu-version.pkr.hcl"
duplicate_constraint="${test_root}/duplicate-qemu-version.pkr.hcl"
inexact_constraint="${test_root}/inexact-qemu-version.pkr.hcl"
printf '%s\n' 'qemu = {' '  source = "github.com/hashicorp/qemu"' '}' \
  > "${missing_constraint}"
printf '%s\n' 'qemu = {' '  version = "= 1.1.5"' \
  '  version = "= 1.1.6"' '}' > "${duplicate_constraint}"
printf '%s\n' 'qemu = {' '  version = ">= 1.1.5"' '}' \
  > "${inexact_constraint}"
for invalid_template in "${missing_constraint}" "${duplicate_constraint}" \
  "${inexact_constraint}"; do
  if (read_qemu_plugin_version "${invalid_template}") >/dev/null 2>&1; then
    die "invalid QEMU plugin constraint was accepted"
  fi
done

derived_provenance="${test_root}/derived-provenance.json"
empty_ca="${test_root}/empty-ca.pem"
derive_dependency_routes proxy.example.invalid registry.example.invalid/cache
dependency_ca_sha256="$(prepare_dependency_ca "" "${empty_ca}")"
write_build_provenance "${derived_provenance}" \
  https://example.invalid/api/releases/latest \
  https://example.invalid/releases/download \
  0123456789ABCDEF0123456789ABCDEF01234567 \
  "${dependency_ca_sha256}"
jq -e '
  .dependency_proxy.ca_sha256 == null and
  .dependency_proxy.configured == true and
  (.dependency_proxy | tostring | test("https?://") | not)
' "${derived_provenance}" >/dev/null \
  || die "build provenance leaked concrete dependency routes"

unset_provenance="${test_root}/unset-provenance.json"
derive_dependency_routes "" ""
write_build_provenance "${unset_provenance}" \
  https://example.invalid/api/releases/latest \
  https://example.invalid/releases/download \
  0123456789ABCDEF0123456789ABCDEF01234567 ""
jq -e '.dependency_proxy == {configured: false, ca_sha256: null}' \
  "${unset_provenance}" >/dev/null \
  || die "unset build provenance emitted dependency configuration"

jq -e '
  keys == ["builder", "created_at", "guest", "image", "provenance",
    "schema_version", "source"] and
  (.provenance.runner_release | keys ==
    ["api_url", "download_base_url", "signing_primary_fingerprint"]) and
  (.provenance.dependency_proxy | keys ==
    ["ca_sha256", "configured"]) and
  (.guest.dependency_proxy_configured ==
    .provenance.dependency_proxy.configured) and
  (.provenance.dependency_proxy | tostring | test("https?://") | not)
' "${fixture}" >/dev/null || die "golden manifest keys are incomplete"

if grep -Fq 'services_root_domain' "${fixture}"; then
  die "golden manifest leaked the template root-domain convention"
fi

echo "manifest golden test passed"
