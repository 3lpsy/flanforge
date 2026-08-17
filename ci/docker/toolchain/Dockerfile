# syntax=docker/dockerfile:1
# Darwin/ARM64 release toolchain. The base supplies the dependency-routed Rust
# installation and shared CI utilities; this layer adds the Apple cross lane.

# No default: CI supplies the in-cluster base derived from the root-domain var.
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
WORKDIR /src

# Keep the cargo-zigbuild pin in step with the release workflow's build
# contract. cargo install uses the base image's crates.io source replacement.
ARG CARGO_ZIGBUILD_VERSION=0.22.3

# coreutils supplies install, mktemp, and sha256sum. List inherited release
# utilities explicitly so a future base trim cannot silently break publishing.
RUN dnf -y install --setopt=install_weak_deps=False \
        ca-certificates coreutils curl file findutils git golang grep jq make sed tar zig \
    && dnf clean all

RUN rustup component add rustfmt clippy \
    && rustup target add aarch64-apple-darwin \
    && CARGO_BUILD_JOBS=2 cargo install --locked \
         cargo-zigbuild --version "${CARGO_ZIGBUILD_VERSION}" \
    && rm -rf "${CARGO_HOME}/registry" "${CARGO_HOME}/git"

# Fail the image build if any required release tool or routing configuration is
# absent. Go dependency resolution remains checksum-locked by Forgejo Runner.
RUN cargo --version \
    && cargo clippy --version \
    && cargo fmt --version \
    && cargo nextest --version \
    && cargo zigbuild --help >/dev/null \
    && just --version \
    && sccache --version \
    && rustup target list --installed | grep -Fx aarch64-apple-darwin \
    && zig version \
    && go version \
    && file --version \
    && jq --version \
    && sha256sum --version >/dev/null \
    && test -s "${CARGO_HOME}/config.toml" \
    && grep -Fq 'replace-with = "chilled-proxy"' "${CARGO_HOME}/config.toml"
