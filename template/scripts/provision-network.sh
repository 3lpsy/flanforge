#!/bin/bash
# Optionally persist deployment-specific dependency proxy routing for every
# runner shell. Tailscale has a separate provisioner and lifecycle.
set -euo pipefail

eval "$(/opt/homebrew/bin/brew shellenv)"
export HOMEBREW_NO_AUTO_UPDATE=1

domain="${SERVICES_ROOT_DOMAIN:-}"
if [ -z "${domain}" ]; then
  echo "==> dependency routing not baked"
  exit 0
fi
[[ "${domain}" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ ]] \
  || { echo "invalid SERVICES_ROOT_DOMAIN" >&2; exit 1; }

runner_home=/Users/runner
deps_host="deps.${domain}"
sudo mkdir -p "${runner_home}/.config/flanforge" "${runner_home}/.cargo"

route_file="${runner_home}/.config/flanforge/dependency-routing.zsh"
sudo tee "${route_file}" >/dev/null <<EOF
export SERVICES_ROOT_DOMAIN=${domain}
export CARGO_REGISTRIES_CHILLED_PROXY_INDEX=sparse+https://${deps_host}/crates/index/
export npm_config_registry=https://${deps_host}/npm/
export BUN_CONFIG_REGISTRY=https://${deps_host}/npm/
export PIP_INDEX_URL=https://${deps_host}/pypi/simple/
export UV_DEFAULT_INDEX=https://${deps_host}/pypi/simple/
EOF

sudo tee "${runner_home}/.cargo/config.toml" >/dev/null <<EOF
[source.crates-io]
replace-with = "chilled-proxy"

[source.chilled-proxy]
registry = "sparse+https://${deps_host}/crates/index/"
EOF

if ! sudo -H -u runner grep -Fqx \
  'source "$HOME/.config/flanforge/dependency-routing.zsh"' \
  "${runner_home}/.zshenv"; then
  # The source line is intentionally literal for evaluation by future shells.
  # shellcheck disable=SC2016
  echo 'source "$HOME/.config/flanforge/dependency-routing.zsh"' \
    | sudo tee -a "${runner_home}/.zshenv" >/dev/null
fi

sudo chown -R runner:staff \
  "${runner_home}/.config" "${runner_home}/.cargo" "${runner_home}/.zshenv"
sudo chmod 0600 "${route_file}" "${runner_home}/.cargo/config.toml"
