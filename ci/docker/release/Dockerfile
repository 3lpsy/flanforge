# syntax=docker/dockerfile:1
# The released flanforged image. cargo-chef cooks the dependency layer so a
# registry layer cache skips it when only sources changed; the runtime stage
# ships the binary and its runtime dependencies, nothing else. The builder
# must carry cargo-chef — the flanforge-toolchain image does.
#
#   podman build -f ci/docker/release/Dockerfile \
#     --build-arg BUILDER_IMAGE=flanforge-toolchain:latest \
#     --build-arg RUNTIME_IMAGE=docker.io/library/fedora:44 \
#     -t flanforged:dev .
#
# Both bases are arguments so the build routes through whatever registry the
# operator uses; nothing here reaches a public registry by default. Pass
# --platform to cross-build, though the runtime targets Linux x86-64: the
# domain XML this daemon emits declares arch="x86_64" machine="q35".

ARG BUILDER_IMAGE
ARG RUNTIME_IMAGE

FROM ${BUILDER_IMAGE} AS chef
WORKDIR /src
# libvirt's development headers are needed to link; the builder must also
# carry pkg-config, because virt-sys locates libvirt through it and silently
# links nothing when the probe fails.
RUN if command -v dnf >/dev/null; then \
        dnf -y install --setopt=install_weak_deps=False libvirt-devel pkgconf-pkg-config \
        && dnf clean all; \
    elif command -v apt-get >/dev/null; then \
        apt-get update && apt-get install -y --no-install-recommends libvirt-dev pkg-config \
        && rm -rf /var/lib/apt/lists/*; \
    else \
        echo "unsupported builder base: install libvirt headers and pkg-config" >&2; exit 1; \
    fi \
    && pkg-config --exists libvirt

# Capture the dependency graph alone, so the cooked layer below only changes
# when a dependency does — not on every source edit.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
# The cooked layer is the cache here; sccache's server cannot start under
# buildah's chroot isolation.
ENV RUSTC_WRAPPER=""
# .cargo first so the cooked artifacts use the same source replacement as the
# real build, or the cache misses.
COPY .cargo .cargo
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --package flanforge --recipe-path recipe.json
COPY . .
# The UI bundle first: the daemon embeds webui/dist at compile time.
RUN just ui-build \
    && cargo build --locked --release --package flanforge \
    && ldd target/release/flanforged | grep -Eq 'libvirt\.so\.'

FROM ${RUNTIME_IMAGE}

# libvirt-libs is the client library the helper links; libvirt-client supplies
# virsh, openssh-clients the qemu+ssh transport and the optional SSH guest
# channel, and qemu-img the image inspection the importer performs before
# publishing. The default libvirt guest channel is the QEMU guest agent, which
# needs none of ssh, scp, or a route to the guest.
RUN dnf -y install --setopt=install_weak_deps=False \
        ca-certificates libvirt-client libvirt-libs openssh-clients qemu-img \
    && dnf clean all

COPY --from=build /src/target/release/flanforged /usr/local/bin/flanforged

# A real account, because `ssh` — used by the qemu+ssh transport, and by the
# SSH guest channel when it is selected — needs a home to resolve known_hosts
# and a passwd entry to resolve its own uid; a bare numeric USER has neither.
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
# pass --config. State and any qemu+ssh or TLS credentials are mounted too, plus
# a guest SSH key only when [guest.ssh] is configured; nothing here needs root,
# and a remote URI never touches the node's own libvirt socket.
USER flanforge
ENV HOME=/home/flanforge
WORKDIR /home/flanforge
ENTRYPOINT ["/usr/local/bin/flanforged"]
CMD ["daemon", "run"]
