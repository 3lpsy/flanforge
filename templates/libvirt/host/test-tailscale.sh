#!/usr/bin/env bash
# Build the read-only guest command for the Tailscale image contract.
# shellcheck disable=SC2034 # The sourcing entrypoint consumes the command.

build_remote_tailscale_contract() {
  remote_tailscale_contract='set -Eeuo pipefail;
    test "$(command -v tailscale)" = /usr/bin/tailscale;
    systemctl is-enabled tailscaled.service >/dev/null;
    systemctl is-active tailscaled.service >/dev/null;
    systemctl is-enabled flanforge-tailscale-operator.service >/dev/null;
    systemctl is-active flanforge-tailscale-operator.service >/dev/null;
    grep -Fqx "Requires=tailscaled.service" /etc/systemd/system/flanforge-tailscale-operator.service;
    grep -Fqx "After=tailscaled.service" /etc/systemd/system/flanforge-tailscale-operator.service;
    grep -Fqx "Before=sshd.service" /etc/systemd/system/flanforge-tailscale-operator.service;
    grep -Fqx "ExecStart=/usr/bin/tailscale set --operator=runner" /etc/systemd/system/flanforge-tailscale-operator.service;
    tailscale set --help 2>&1 | grep -Fq -- --operator;
    if [ -f /usr/local/share/flanforge/tailscale-retained-login ]; then
      tailscale status --json | jq -e '\''
        .BackendState == "Running" and
        ((.TailscaleIPs // []) | length > 0) and
        ((.HaveNodeKey // false) == true)
      '\'' >/dev/null;
    else
      tailscale status --json | jq -e '\''
        .BackendState != "Running" and
        ((.TailscaleIPs // []) | length == 0) and
        ((.HaveNodeKey // false) == false) and
        ((.CurrentTailnet // null) == null)
      '\'' >/dev/null;
    fi;
    test ! -e /tmp/flanforge-tailscale-preauth-key'
}
