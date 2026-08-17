set shell := ["bash", "-euo", "pipefail", "-c"]
set dotenv-load := true

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
	members="$(sed -n '/^\[workspace\]/,/^\]/p' Cargo.toml | sed -n 's|^ *"crates/\([^"]*\)".*|\1|p')"
	[ -n "${members}" ] || { echo "error: no workspace members found in Cargo.toml" >&2; exit 1; }
	awk -v members="${members}" -v new="${new}" '
	  BEGIN { count = split(members, list, "\n"); for (i = 1; i <= count; i++) wanted[list[i]] = 1 }
	  /^name = "/ {
	    name = $0; sub(/^name = "/, "", name); sub(/"$/, "", name); member = (name in wanted)
	  }
	  member && /^version = "/ { sub(/"[^"]*"/, "\"" new "\""); member = 0 }
	  { print }
	' Cargo.lock > Cargo.lock.tmp
	mv Cargo.lock.tmp Cargo.lock
	locked="$(awk '/^name = "flanforge"$/ { getline; print; exit }' Cargo.lock)"
	if [ "${locked}" != "version = \"${new}\"" ]; then
	  echo "error: Cargo.lock still records ${locked:-nothing} after the bump" >&2
	  exit 1
	fi
	echo "[workspace.package] version: ${cur} -> ${new} (Cargo.lock refreshed)"
	echo "Next: commit both, then 'just ci-tagged-release'."

check: fmt-check clippy test test-doc

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo nextest run --locked --workspace --all-targets

# nextest deliberately excludes doctests, so keep this as an explicit lane.
test-doc:
	cargo test --locked --workspace --doc

# The single full suite used by test.yml and tagged releases.
ci-tests: fmt-check clippy test test-doc

# Build the retained Tart template. Apple Silicon only; see .env.example.
template-build *args:
	./scripts/build-template.sh {{args}}

# Clone the built template, check it over SSH as the runner account, delete it.
template-test *args:
	./scripts/test-template.sh {{args}}

# Bump [workspace.package], then refresh Cargo.lock before a tagged release.
version-bump-bugfix: (_version-bump "patch")
version-bump-minor: (_version-bump "minor")
version-bump-major: (_version-bump "major")

# Push v<workspace-version> to the internal Forgejo remote and trigger release CI.
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
	if [ -n "$(git status --porcelain)" ]; then
	  echo "error: working tree dirty; commit or stash before tagging ${tag}" >&2
	  exit 1
	fi
	if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
	  echo "error: tag ${tag} already exists locally (bump the workspace version first)" >&2
	  exit 1
	fi
	set +e
	git ls-remote --exit-code --tags internal "refs/tags/${tag}" >/dev/null 2>&1
	remote_status=$?
	set -e
	case "${remote_status}" in
	  0) echo "error: tag ${tag} already exists on internal (bump the workspace version first)" >&2; exit 1 ;;
	  2) ;;
	  *) echo "error: cannot inspect tags on the internal remote" >&2; exit 1 ;;
	esac
	git tag -a "${tag}" -m "Release ${tag}"
	git push internal "${tag}"
	echo "Pushed ${tag} -> CI builds and publishes the Darwin/ARM64 release assets"

# Re-release the current version from HEAD. Uncommitted work still does not
# ship: CI builds the commit referenced by the force-updated tag.
ci-tagged-release-force:
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
	if [ -n "$(git status --porcelain)" ]; then
	  echo "warning: working tree dirty; CI builds the tagged commit, not local changes" >&2
	fi
	git tag -fa "${tag}" -m "Release ${tag}"
	git push --force internal "refs/tags/${tag}:refs/tags/${tag}"
	echo "Force-pushed ${tag} -> CI rebuilds and republishes the Darwin/ARM64 release assets"

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
	echo "==> ${asset} from ${release}"
	fj release asset download -r "${FORGEJO_REPO:-jim/flanforge}" "${release}" "${asset}" -o "${asset}"
	fj release asset download -r "${FORGEJO_REPO:-jim/flanforge}" "${release}" "${asset}.sha256" -o "${asset}.sha256"
	if command -v shasum >/dev/null 2>&1; then
	  shasum -a 256 -c "${asset}.sha256"
	else
	  sha256sum -c "${asset}.sha256"
	fi
	chmod +x "${asset}"

# Download the flanforged binary for the in-tree version, or a given tag.
fj-download tag="": (_fj-download "flanforged-darwin-arm64" tag)

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
	git tag -fa "${tag}" -m "Untested release ${tag}"
	git push --force internal "refs/tags/${tag}:refs/tags/${tag}"
	echo "Pushed ${tag} -> release-unsafe.yml builds and publishes it as a prerelease"
	echo "CI builds the tagged commit; uncommitted work is not included."

# Push a unique tag so test.yml exercises the committed tree in CI.
ci-test:
	#!/usr/bin/env bash
	set -euo pipefail
	tag="ci-test-$(date +%Y%m%d-%H%M%S)"
	if [ -n "$(git status --porcelain)" ]; then
	  echo "warning: working tree dirty; CI runs the tagged commit, not local changes" >&2
	fi
	git tag "${tag}"
	git push internal "${tag}"
	echo "Pushed ${tag} -> test.yml"
