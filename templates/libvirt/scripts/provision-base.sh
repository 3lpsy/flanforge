#!/usr/bin/env bash
# Install only the Linux guest services needed by FlanForge jobs.
set -Eeuo pipefail

trap 'echo "base provisioning failed at ${BASH_SOURCE[0]}:${LINENO}" >&2' ERR

# cloud-init exits 2 for "done with recoverable errors": first boot finished
# and only warnings remain, which --long prints into the build log. Anything
# else nonzero is a real first-boot failure.
cloud_init_status=0
sudo cloud-init status --wait --long || cloud_init_status=$?
if [ "${cloud_init_status}" -ne 0 ] && [ "${cloud_init_status}" -ne 2 ]; then
  echo "cloud-init reported first-boot failure (exit ${cloud_init_status})" >&2
  exit 1
fi
sudo dnf -y upgrade --refresh
# Weak deps stay off: docker-cli recommends moby-engine, which would put a
# root Docker daemon with an enabled socket into every guest.
sudo dnf -y install --setopt=install_weak_deps=False \
  bash buildah ca-certificates cloud-init curl file findutils fuse-overlayfs git git-lfs \
  docker-cli jq openssh-clients openssh-server openssl passt podman \
  qemu-guest-agent shadow-utils shadow-utils-subid slirp4netns tar unzip util-linux

# cloud-init 24.3 renamed cloud-init.service to cloud-init-network.service.
cloud_init_network_unit=cloud-init.service
[ ! -f /usr/lib/systemd/system/cloud-init-network.service ] \
  || cloud_init_network_unit=cloud-init-network.service
sudo systemctl enable \
  cloud-config.service cloud-final.service cloud-init-local.service \
  "${cloud_init_network_unit}" qemu-guest-agent.service sshd.service

echo "==> Linux base packages installed"
