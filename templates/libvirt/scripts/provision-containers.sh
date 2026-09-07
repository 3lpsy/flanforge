#!/usr/bin/env bash
# Configure rootless Podman and its Docker-compatible API for the runner.
set -Eeuo pipefail

trap 'echo "container provisioning failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

runner_home=/home/runner
runner_uid=2000
runtime_dir="/run/user/${runner_uid}"
socket_path="${runtime_dir}/podman/podman.sock"

sudo install -d -m 0700 -o runner -g runner \
  "${runner_home}/.config/containers" \
  "${runner_home}/.config/environment.d" \
  "${runner_home}/.config/systemd/user/sockets.target.wants"

sudo tee "${runner_home}/.config/containers/containers.conf" >/dev/null <<'EOF'
[containers]
pids_limit = 2048

[engine]
cgroup_manager = "systemd"
events_logger = "file"
EOF

sudo ln -sfn /usr/lib/systemd/user/podman.socket \
  "${runner_home}/.config/systemd/user/sockets.target.wants/podman.socket"

sudo tee "${runner_home}/.config/environment.d/10-containers.conf" >/dev/null <<EOF
CONTAINER_HOST=unix://${socket_path}
DOCKER_HOST=unix://${socket_path}
EOF

sudo tee "${runner_home}/.config/flanforge-containers.sh" >/dev/null <<EOF
export CONTAINER_HOST=unix://${socket_path}
export DOCKER_HOST=unix://${socket_path}
EOF

for shell_file in .bash_profile .bashrc; do
  printf '%s\n' 'source "$HOME/.config/flanforge-containers.sh"' \
    | sudo tee -a "${runner_home}/${shell_file}" >/dev/null
done

sudo chown -R runner:runner "${runner_home}/.config" \
  "${runner_home}/.bash_profile" "${runner_home}/.bashrc"
sudo chmod 0600 "${runner_home}/.config/flanforge-containers.sh" \
  "${runner_home}/.bash_profile" "${runner_home}/.bashrc"
sudo loginctl enable-linger runner

echo "==> rootless Podman socket configured at ${socket_path}"
