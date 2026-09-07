# syntax=docker/dockerfile:1
# Linux/x86-64 and Darwin/ARM64 release toolchain. The base supplies the
# dependency-routed Rust installation and shared CI utilities.

# No defaults: CI supplies both, derived from the root-domain var, so the
# images come through the dependency proxy rather than a public registry.
ARG BASE_IMAGE
ARG PACKER_IMAGE
FROM ${PACKER_IMAGE} AS packer
FROM ${BASE_IMAGE}
WORKDIR /src

COPY --from=packer /bin/packer /usr/local/bin/packer

# Keep the cargo-zigbuild pin in step with the release workflow's build
# contract. cargo install uses the base image's crates.io source replacement.
ARG CARGO_ZIGBUILD_VERSION=0.22.3
# cargo-chef cooks the dependency layer the release image build caches.
ARG CARGO_CHEF_VERSION=0.1.72
# Must match the wasm-bindgen library pin in Cargo.toml; `just _ui-check-wbg`
# enforces the pairing against Cargo.lock at build time.
ARG WASM_BINDGEN_VERSION=0.2.125

# coreutils supplies install, mktemp, and sha256sum. List inherited release
# utilities explicitly so a future base trim cannot silently break publishing.
RUN dnf -y install --setopt=install_weak_deps=False \
        ca-certificates coreutils curl file findutils git golang grep jq ShellCheck \
        libvirt-devel make pkgconf-pkg-config qemu-img sed tar zig \
    && dnf clean all

RUN rustup component add rustfmt clippy \
    && rustup target add aarch64-apple-darwin wasm32-unknown-unknown \
    && CARGO_BUILD_JOBS=2 cargo install --locked \
         cargo-zigbuild --version "${CARGO_ZIGBUILD_VERSION}" \
    && CARGO_BUILD_JOBS=2 cargo install --locked \
         cargo-chef --version "${CARGO_CHEF_VERSION}" \
    && CARGO_BUILD_JOBS=2 cargo install --locked \
         wasm-bindgen-cli --version "${WASM_BINDGEN_VERSION}" \
    && rm -rf "${CARGO_HOME}/registry" "${CARGO_HOME}/git"

# Fail the image build if any required release tool or routing configuration is
# absent. Go dependency resolution remains checksum-locked by Forgejo Runner.
RUN cargo --version \
    && cargo clippy --version \
    && cargo fmt --version \
    && cargo nextest --version \
    && cargo zigbuild --help >/dev/null \
    && cargo chef --version \
    && just --version \
    && sccache --version \
    && rustup target list --installed | grep -Fx aarch64-apple-darwin \
    && rustup target list --installed | grep -Fx wasm32-unknown-unknown \
    && wasm-bindgen --version \
    && zig version \
    && go version \
    && node --version | grep -Eq '^v22\.[0-9]+\.[0-9]+$' \
    && packer version \
    && pkg-config --exists libvirt \
    && qemu-img --version \
    && ldd --version \
    && shellcheck --version \
    && file --version \
    && jq --version \
    && sha256sum --version >/dev/null \
    && test -s "${CARGO_HOME}/config.toml" \
    && grep -Fq 'replace-with = "chilled-proxy"' "${CARGO_HOME}/config.toml"
