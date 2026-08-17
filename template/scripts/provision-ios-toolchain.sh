#!/bin/bash
# Native iOS build dependencies shared by Halogen and LiftFG.
set -euo pipefail

eval "$(/opt/homebrew/bin/brew shellenv)"
export HOMEBREW_NO_AUTO_UPDATE=1

# node: JavaScript Forgejo actions (including checkout).
# python: LiftFG's xcresult/screenshot collector.
# xcodegen + typeshare: both native SwiftUI project pipelines.
brew install node python xcodegen typeshare

runner_home=/Users/runner
sudo -H -u runner env \
  HOME="${runner_home}" \
  RUST_TOOLCHAIN="${RUST_TOOLCHAIN}" \
  JUST_VERSION="${JUST_VERSION}" \
  NEXTEST_VERSION="${NEXTEST_VERSION}" \
  SCCACHE_VERSION="${SCCACHE_VERSION}" \
  /bin/bash <<'RUNNER_TOOLCHAIN'
set -euo pipefail

echo "==> rustup ${RUST_TOOLCHAIN} for runner"
curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
  | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "${RUST_TOOLCHAIN}"
# shellcheck disable=SC1091
source "${HOME}/.cargo/env"
rustup component add rustfmt clippy
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

echo "==> pinned CI cargo tools"
curl --proto '=https' --tlsv1.2 -fsSL \
  -o /tmp/cargo-binstall.zip \
  https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-aarch64-apple-darwin.zip
unzip -q -o /tmp/cargo-binstall.zip -d "${HOME}/.cargo/bin"
rm -f /tmp/cargo-binstall.zip
cargo binstall -y \
  "just@${JUST_VERSION}" \
  "cargo-nextest@${NEXTEST_VERSION}" \
  "sccache@${SCCACHE_VERSION}"

touch "${HOME}/.zshenv"
grep -Fqx 'eval "$(/opt/homebrew/bin/brew shellenv)"' "${HOME}/.zshenv" \
  || echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> "${HOME}/.zshenv"
grep -Fqx 'source "$HOME/.cargo/env"' "${HOME}/.zshenv" \
  || echo 'source "$HOME/.cargo/env"' >> "${HOME}/.zshenv"
grep -Fqx 'export PATH="$HOME/bin:$PATH"' "${HOME}/.zshenv" \
  || echo 'export PATH="$HOME/bin:$PATH"' >> "${HOME}/.zshenv"
RUNNER_TOOLCHAIN

sudo chown runner:staff "${runner_home}/.zshenv"
