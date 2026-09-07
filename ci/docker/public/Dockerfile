# syntax=docker/dockerfile:1
# Public flanforged image, built on the GitHub mirror. The binary is compiled
# in the workflow and copied in, so this is only the runtime stage; keep it in
# step with ci/docker/release/Dockerfile's runtime stage.
#
# Build context: a directory holding the prebuilt Linux/x86-64 `flanforged`.

ARG RUNTIME_IMAGE=docker.io/library/fedora:44
FROM ${RUNTIME_IMAGE}

# libvirt-libs is the client library the daemon links; libvirt-client supplies
# virsh, openssh-clients the qemu+ssh transport and the optional SSH guest
# channel, and qemu-img the image inspection the importer performs before
# publishing.
RUN dnf -y install --setopt=install_weak_deps=False \
        ca-certificates libvirt-client libvirt-libs openssh-clients qemu-img \
    && dnf clean all

COPY flanforged /usr/local/bin/flanforged

# A real account, because `ssh` needs a home to resolve known_hosts and a
# passwd entry to resolve its own uid; a bare numeric USER has neither.
RUN useradd --create-home --uid 1000 --user-group --shell /usr/sbin/nologin flanforge \
    && install -d -o flanforge -g flanforge -m 0700 /home/flanforge/.ssh \
    && install -d -o flanforge -g flanforge -m 0700 /var/lib/flanforge \
    && chmod 0755 /usr/local/bin/flanforged

# The configured defaults are absolute, so a missing tool must fail the build
# here rather than the first allocation. Running the daemon proves it links
# against the libvirt this image installed.
RUN test -x /usr/bin/ssh \
    && test -x /usr/bin/scp \
    && test -x /usr/bin/qemu-img \
    && test -x /usr/bin/virsh \
    && flanforged --help >/dev/null

# Mount the configuration at the Linux default, /etc/flanforge/config.toml, or
# pass --config. Nothing here needs root, and a remote URI never touches the
# node's own libvirt socket.
USER flanforge
ENV HOME=/home/flanforge
WORKDIR /home/flanforge
ENTRYPOINT ["/usr/local/bin/flanforged"]
CMD ["daemon", "run"]
