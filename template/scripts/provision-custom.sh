#!/bin/bash
# Deployment-specific additions. Keep this file credential-free.
set -euo pipefail

echo "==> running custom provisioner"
eval "$(/opt/homebrew/bin/brew shellenv)"
export HOMEBREW_NO_AUTO_UPDATE=1

echo "==> Bun and uv"
brew install oven-sh/bun/bun uv

# Homebrew is available to this provisioning account. Examples:
# brew install tailwindcss
# Community Swift tools; enable only with matching repository configuration.
# brew install swiftformat swiftlint swiftgen xcbeautify
# brew install libimobiledevice ideviceinstaller ios-deploy
# brew install mint

# Install extra Rust targets or cargo tools for the non-admin runner. Keep
# versions pinned so rebuilding the template remains reproducible.
# sudo -H -u runner env HOME=/Users/runner /bin/zsh -lc '
#   set -e
#   source "$HOME/.cargo/env"
#   rustup target add wasm32-unknown-unknown
#   cargo binstall -y "dioxus-cli@0.7.9"
#   cargo binstall -y "wasm-bindgen-cli@0.2.100"
# '

# A standalone Tailwind binary can be used instead of Homebrew. Pin both its
# version and SHA-256 before enabling this example.
# TAILWIND_VERSION=4.1.14
# TAILWIND_SHA256=replace-with-published-sha256
# curl --proto '=https' --tlsv1.2 -fsSL \
#   -o /tmp/tailwindcss \
#   "https://github.com/tailwindlabs/tailwindcss/releases/download/v${TAILWIND_VERSION}/tailwindcss-macos-arm64"
# echo "${TAILWIND_SHA256}  /tmp/tailwindcss" | shasum -a 256 -c -
# sudo install -m 0755 /tmp/tailwindcss /usr/local/bin/tailwindcss
# sudo rm -f /tmp/tailwindcss

# Project-specific cache warming can run here. Fetch only trusted revisions,
# never retain repository credentials, and remove the checkout afterward.
# sudo -H -u runner env HOME=/Users/runner /bin/zsh -lc '
#   set -e
#   export SCCACHE_DIR="$HOME/.sccache"
#   export CARGO_INCREMENTAL=0
#   # Clone a trusted, pinned revision and run its build commands here.
# '
