set shell := ["bash", "-euo", "pipefail", "-c"]

default:
	@just --list

# Shared implementation behind version-bump-*. part is major, minor, or patch.
_version-bump part:
	#!/usr/bin/env bash
	set -euo pipefail
	part='{{part}}'
	cur="$(sed -n '/^\[workspace.package\]/,/^\[/p' Cargo.toml | grep -m1 '^version' | sed -E 's/.*"([^"]+)".*/\1/')"
	if [[ ! "${cur}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
	  echo "error: [workspace.package] version '${cur}' is not a plain X.Y.Z" >&2
	  exit 1
	fi
	major="${BASH_REMATCH[1]}"; minor="${BASH_REMATCH[2]}"; patch="${BASH_REMATCH[3]}"
	case "${part}" in
	  major) major=$((major + 1)); minor=0; patch=0 ;;
	  minor) minor=$((minor + 1)); patch=0 ;;
	  patch) patch=$((patch + 1)) ;;
	  *) echo "error: unknown bump part '${part}'" >&2; exit 1 ;;
	esac
	new="${major}.${minor}.${patch}"
	awk -v new="${new}" '
	  /^\[/ { in_workspace_package = ($0 == "[workspace.package]") }
	  in_workspace_package && /^version[[:space:]]*=/ && !done {
	    sub(/"[^"]*"/, "\"" new "\""); done = 1
	  }
	  { print }
	' Cargo.toml > Cargo.toml.tmp
	mv Cargo.toml.tmp Cargo.toml
	# Rewrite the members' locked versions directly. CI builds with --locked, and
	# resolving the graph here would need a registry that has every locked crate.
	# Names come from each member manifest: a member's directory (webui/app)
	# need not match its package name (flanforge-webui).
	member_paths="$(sed -n '/^\[workspace\]/,/^\]/p' Cargo.toml | sed -n 's|^ *"\([^"]*\)".*|\1|p')"
	[ -n "${member_paths}" ] || { echo "error: no workspace members found in Cargo.toml" >&2; exit 1; }
	members="$(for path in ${member_paths}; do sed -n 's/^name = "\(.*\)"$/\1/p' "${path}/Cargo.toml" | head -n1; done)"
	[ -n "${members}" ] || { echo "error: no member package names resolved" >&2; exit 1; }
	awk -v members="${members}" -v new="${new}" '
	  BEGIN { count = split(members, list, "\n"); for (i = 1; i <= count; i++) wanted[list[i]] = 1 }
	  /^name = "/ {
	    name = $0; sub(/^name = "/, "", name); sub(/"$/, "", name); member = (name in wanted)
	  }
	  member && /^version = "/ { sub(/"[^"]*"/, "\"" new "\""); member = 0 }
	  { print }
	' Cargo.lock > Cargo.lock.tmp
	mv Cargo.lock.tmp Cargo.lock
	for name in ${members}; do
	  locked="$(awk -v name="${name}" '$0 == "name = \"" name "\"" { getline; print; exit }' Cargo.lock)"
	  if [ "${locked}" != "version = \"${new}\"" ]; then
	    echo "error: Cargo.lock records ${locked:-nothing} for ${name} after the bump" >&2
	    exit 1
	  fi
	done
	echo "[workspace.package] version: ${cur} -> ${new} (Cargo.lock refreshed)"
	echo "Next: commit both, then 'just ci-tagged-release'."

check: fmt-check clippy test test-doc

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --locked --workspace --all-targets -- -D warnings

test:
	cargo nextest run --locked --workspace --all-targets

# nextest deliberately excludes doctests, so keep this as an explicit lane.
test-doc:
	cargo test --locked --workspace --doc

# The single full suite used by test.yml and tagged releases.
ci-tests: fmt-check clippy test test-doc

# ── Web UI (ui-*) ─────────────────────────────────────────────────────────────
# One-time setup: rustup target add wasm32-unknown-unknown
#                 cargo install wasm-bindgen-cli --version <Cargo.lock version>

# Every wasm crate; hand-kept because default-members excludes them.
ui_packages := "-p flanforge-webui -p flanforge-webui-api-client -p flanforge-webui-components"

# Fails fast when the installed wasm-bindgen CLI mismatches Cargo.lock.
_ui-check-wbg:
	#!/usr/bin/env bash
	set -euo pipefail
	want="$(awk '/^name = "wasm-bindgen"$/{f=1} f && /^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.lock)"
	have="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')" || true
	if [ "${have:-}" != "$want" ]; then
	  echo "error: wasm-bindgen CLI ${have:-missing} != Cargo.lock $want" >&2
	  echo "fix: cargo install wasm-bindgen-cli --version $want" >&2
	  exit 1
	fi

# Build the Dioxus UI into webui/dist/ (wasm + bindgen + static files).
ui-build: _ui-check-wbg
	cargo build --locked -p flanforge-webui --profile wasm-release --target wasm32-unknown-unknown
	mkdir -p webui/dist
	wasm-bindgen --target web --no-typescript --out-dir webui/dist --out-name flanforge-webui \
		"${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/wasm-release/flanforge-webui.wasm"
	cp webui/app/index.html webui/app/assets/style.css webui/app/assets/favicon.svg webui/dist/

# Type-check the wasm crates (web-sys and dioxus move fast).
ui-check:
	cargo check --locked {{ui_packages}} --target wasm32-unknown-unknown

ui-clippy:
	#!/usr/bin/env bash
	set -euo pipefail
	# Guard the hand-kept package list against a crate added to webui/ later.
	for member in webui/*/Cargo.toml; do
	  name="$(grep -m1 '^name = ' "$member" | sed -E 's/.*"([^"]+)".*/\1/')"
	  if ! grep -q -- "-p $name" <<< "{{ui_packages}}"; then
	    echo "error: $name is missing from ui_packages in the justfile" >&2
	    exit 1
	  fi
	done
	cargo clippy --locked {{ui_packages}} --target wasm32-unknown-unknown -- -D warnings

# UI iteration never re-embeds or relinks the daemon: webui/dist is read from
# disk via webui.dev_dist_dir.
ui-dev config="config.toml": ui-build
	cargo run -p flanforge -- --config {{config}} --set webui.dev_dist_dir=$(pwd)/webui/dist daemon run

# Build the retained Tart template. Apple Silicon only; see templates/tart/.env.example.
[positional-arguments]
tart-template-build *args:
	#!/usr/bin/env bash
	set -euo pipefail
	exec ./templates/tart/build.sh "$@"

# Clone the Tart template, check it as the runner account, then delete it.
[positional-arguments]
tart-template-test *args:
	#!/usr/bin/env bash
	set -euo pipefail
	exec ./templates/tart/test.sh "$@"

# Run the job-time path in a clone: boot the simulator and one xcodebuild test.
[positional-arguments]
tart-template-smoke *args:
	#!/usr/bin/env bash
	set -euo pipefail
	exec ./templates/tart/smoke.sh "$@"

# Build or validate the Linux libvirt base; see templates/libvirt/README.md.
[positional-arguments]
libvirt-template-build *args:
	#!/usr/bin/env bash
	set -euo pipefail
	exec ./templates/libvirt/build.sh "$@"

# Validate the libvirt Packer template without requiring KVM or starting QEMU.
libvirt-template-validate:
	./templates/libvirt/build.sh --validate-only

# Preflight, or explicitly execute, the disposable libvirt clone test.
[positional-arguments]
libvirt-template-test *args:
	#!/usr/bin/env bash
	set -euo pipefail
	exec ./templates/libvirt/test.sh "$@"

# Execute and retain a confirmed disposable clone for interactive inspection.
[positional-arguments]
libvirt-template-shell *args:
	#!/usr/bin/env bash
	set -euo pipefail
	exec ./templates/libvirt/test.sh --execute --keep "$@"

# Exercise libvirt test cleanup against the fake virsh implementation.
libvirt-template-unit-test:
	./templates/libvirt/tests/test-runtime.sh

# Run every host-independent template test without Tart, KVM, or libvirt.
template-unit-tests:
	#!/usr/bin/env bash
	set -euo pipefail
	for script in templates/shared/tests/*.sh templates/tart/tests/*.sh templates/libvirt/tests/*.sh; do
	  "${script}"
	done

# Compatibility aliases retained for the first release after the Tart move.
[positional-arguments]
template-build *args:
	#!/usr/bin/env bash
	set -euo pipefail
	echo "warning: template-build is deprecated; use tart-template-build" >&2
	exec just tart-template-build "$@"

[positional-arguments]
template-test *args:
	#!/usr/bin/env bash
	set -euo pipefail
	echo "warning: template-test is deprecated; use tart-template-test" >&2
	exec just tart-template-test "$@"

# Bump [workspace.package], then refresh Cargo.lock before a tagged release.
version-bump-bugfix: (_version-bump "patch")
version-bump-minor: (_version-bump "minor")
version-bump-major: (_version-bump "major")

# Resolve the release remote from an override, branch upstream, or sole remote.
_release-remote:
	#!/usr/bin/env bash
	set -euo pipefail
	release_remote="${FLANFORGE_RELEASE_REMOTE:-}"
	if [ -z "${release_remote}" ]; then
	  branch="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
	  if [ -n "${branch}" ]; then
	    release_remote="$(git config --get "branch.${branch}.remote" || true)"
	  fi
	fi
	if [ -z "${release_remote}" ]; then
	  if git remote | grep -Fxq origin; then
	    release_remote=origin
	  elif [ "$(git remote | awk 'NF { count += 1 } END { print count + 0 }')" -eq 1 ]; then
	    release_remote="$(git remote)"
	  fi
	fi
	[[ "${release_remote}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] || {
	  echo "error: set FLANFORGE_RELEASE_REMOTE to one configured remote" >&2
	  exit 1
	}
	git remote get-url "${release_remote}" >/dev/null 2>&1 || {
	  echo "error: release remote '${release_remote}' is not configured" >&2
	  exit 1
	}
	printf '%s\n' "${release_remote}"

# Resolve Forgejo's owner/repository name without embedding a deployment owner.
_forgejo-repository:
	#!/usr/bin/env bash
	set -euo pipefail
	repository="${FORGEJO_REPO:-}"
	if [ -z "${repository}" ]; then
	  release_remote="$(just --quiet _release-remote)"
	  remote_url="$(git remote get-url "${release_remote}")"
	  case "${remote_url}" in
	    *://*)
	      without_scheme="${remote_url#*://}"
	      repository="${without_scheme#*/}"
	      ;;
	    *@*:*) repository="${remote_url#*:}" ;;
	    *)
	      echo "error: cannot derive Forgejo repository from '${release_remote}'" >&2
	      exit 1
	      ;;
	  esac
	  repository="${repository#/}"
	  repository="${repository%.git}"
	fi
	[[ "${repository}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}/[A-Za-z0-9][A-Za-z0-9._-]{0,99}$ ]] || {
	  echo "error: FORGEJO_REPO must be an owner/repository pair" >&2
	  exit 1
	}
	printf '%s\n' "${repository}"

# Push v<workspace-version> to the configured Forgejo remote and trigger CI.
ci-tagged-release:
	#!/usr/bin/env bash
	set -euo pipefail
	echo "Formatting..."
	cargo fmt --all
	version="$(sed -n '/^\[workspace.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\([^"]*\)"/\1/p' | head -n1)"
	[[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
	  echo "error: workspace version '${version:-missing}' is not a plain X.Y.Z" >&2
	  exit 1
	}
	tag="v${version}"
	release_remote="$(just --quiet _release-remote)"
	if [ -n "$(git status --porcelain)" ]; then
	  echo "error: working tree dirty; commit or stash before tagging ${tag}" >&2
	  exit 1
	fi
	if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
	  echo "error: tag ${tag} already exists locally (bump the workspace version first)" >&2
	  exit 1
	fi
	set +e
	git ls-remote --exit-code --tags "${release_remote}" "refs/tags/${tag}" >/dev/null 2>&1
	remote_status=$?
	set -e
	case "${remote_status}" in
	  0) echo "error: tag ${tag} already exists on ${release_remote} (bump the workspace version first)" >&2; exit 1 ;;
	  2) ;;
	  *) echo "error: cannot inspect tags on ${release_remote}" >&2; exit 1 ;;
	esac
	git tag -a "${tag}" -m "Release ${tag}"
	git push "${release_remote}" "${tag}"
	echo "Pushed ${tag} -> CI builds and publishes the release assets"

# Compatibility guard: published versions are immutable and cannot be rebuilt.
ci-tagged-release-force:
	#!/usr/bin/env bash
	set -euo pipefail
	echo "error: release tags and assets are immutable; bump the version" >&2
	exit 1

# Shared implementation behind fj-download*. Downloads one release asset and
# its checksum into the working directory, then verifies it.
_fj-download asset tag="":
	#!/usr/bin/env bash
	set -euo pipefail
	asset='{{asset}}'
	tag='{{tag}}'
	if [ -z "${tag}" ]; then
	  version="$(sed -n '/^\[workspace.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\([^"]*\)"/\1/p' | head -n1)"
	  [[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
	    echo "error: workspace version '${version:-missing}' is not a plain X.Y.Z" >&2
	    exit 1
	  }
	  tag="v${version}"
	fi
	# fj matches a release by its title, which is "FlanForge <tag>".
	release="FlanForge ${tag}"
	repository="$(just --quiet _forgejo-repository)"
	echo "==> ${asset} from ${release}"
	fj release asset download -r "${repository}" "${release}" "${asset}" -o "${asset}"
	fj release asset download -r "${repository}" "${release}" "${asset}.sha256" -o "${asset}.sha256"
	if command -v shasum >/dev/null 2>&1; then
	  shasum -a 256 -c "${asset}.sha256"
	else
	  sha256sum -c "${asset}.sha256"
	fi
	chmod +x "${asset}"

# Download the target-native flanforged binary for the in-tree version or tag.
fj-download tag="":
	#!/usr/bin/env bash
	set -euo pipefail
	case "$(uname -s):$(uname -m)" in
	  Darwin:arm64) asset=flanforged-darwin-arm64 ;;
	  Linux:x86_64) asset=flanforged-linux-x86_64 ;;
	  *) echo "error: no release asset for $(uname -s)/$(uname -m)" >&2; exit 1 ;;
	esac
	just _fj-download "${asset}" '{{tag}}'

# Download the Darwin/ARM64 daemon explicitly.
fj-download-darwin tag="": (_fj-download "flanforged-darwin-arm64" tag)

# Download the Linux/x86-64 daemon explicitly.
fj-download-linux tag="": (_fj-download "flanforged-linux-x86_64" tag)

# Download the pinned Forgejo Runner binary for the in-tree version, or a tag.
fj-download-runner tag="":
	#!/usr/bin/env bash
	set -euo pipefail
	runner_version="$(sed -n 's/^FORGEJO_RUNNER_VERSION=//p' ci/forgejo-runner.env)"
	[[ "${runner_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
	  echo "error: Forgejo Runner version pin is invalid" >&2
	  exit 1
	}
	just _fj-download "forgejo-runner-${runner_version}-darwin-arm64" '{{tag}}'

# Push unsafe-v<workspace-version> to build and publish without the test suite.
# The separate tag prefix keeps untested builds out of the tested release lane.
ci-tagged-release-unsafe:
	#!/usr/bin/env bash
	set -euo pipefail
	version="$(sed -n '/^\[workspace.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\([^"]*\)"/\1/p' | head -n1)"
	[[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
	  echo "error: workspace version '${version:-missing}' is not a plain X.Y.Z" >&2
	  exit 1
	}
	tag="unsafe-v${version}"
	release_remote="$(just --quiet _release-remote)"
	if [ -n "$(git status --porcelain)" ]; then
	  echo "error: working tree dirty; commit or stash before tagging ${tag}" >&2
	  exit 1
	fi
	if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
	  echo "error: tag ${tag} already exists locally (bump the workspace version first)" >&2
	  exit 1
	fi
	set +e
	git ls-remote --exit-code --tags "${release_remote}" "refs/tags/${tag}" >/dev/null 2>&1
	remote_status=$?
	set -e
	case "${remote_status}" in
	  0) echo "error: tag ${tag} already exists on ${release_remote} (bump the workspace version first)" >&2; exit 1 ;;
	  2) ;;
	  *) echo "error: cannot inspect tags on ${release_remote}" >&2; exit 1 ;;
	esac
	git tag -a "${tag}" -m "Untested release ${tag}"
	git push "${release_remote}" "refs/tags/${tag}:refs/tags/${tag}"
	echo "Pushed ${tag} -> release-unsafe.yml builds and publishes it as a prerelease"

# Push a unique tag so test.yml exercises the committed tree in CI.
ci-test:
	#!/usr/bin/env bash
	set -euo pipefail
	tag="ci-test-$(date +%Y%m%d-%H%M%S)"
	release_remote="$(just --quiet _release-remote)"
	if [ -n "$(git status --porcelain)" ]; then
	  echo "warning: working tree dirty; CI runs the tagged commit, not local changes" >&2
	fi
	git tag "${tag}"
	git push "${release_remote}" "${tag}"
	echo "Pushed ${tag} -> test.yml"
