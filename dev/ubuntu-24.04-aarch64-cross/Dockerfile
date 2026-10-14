# x86_64 Ubuntu 24.04 cross-compiling Twicecat for aarch64-unknown-linux-gnu
# (the Steam Frame) with clang 22, against Ubuntu's arm64 libraries: glibc
# 2.39, as the packages from dev/ubuntu-24.04 (Phase 7 plan Q14 d). For
# local builds on a machine with no arm64 CPU; release CI builds natively on
# its arm runner. About 5 minutes for a release build on 24 threads.
#
#   docker build -t twicecat-aarch64-cross dev/ubuntu-24.04-aarch64-cross
#   docker run --rm --user "$(id -u):$(id -g)" -e HOME="$HOME" -e CARGO_TARGET_DIR=target/aarch64-cross \
#       -v "$PWD:$PWD" -v "$HOME/.cache/twicecat:$HOME/.cache/twicecat" --workdir "$PWD" \
#       twicecat-aarch64-cross bash -c 'export LK_CUSTOM_WEBRTC=$(scripts/fetch-libwebrtc.sh linux-arm64) &&
#           cargo build --locked --release --target aarch64-unknown-linux-gnu -p twicecat-app --bin twicecat --features gpu'
#
# The binary then goes onto an aarch64 machine, where
# `scripts/package.sh --no-build` packages it (it reads the host's triple).
FROM ubuntu:24.04
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# arm64 packages come from ports.ubuntu.com; the stock sources serve amd64 only.
RUN sed -i 's/^Types: deb$/Types: deb\nArchitectures: amd64/' /etc/apt/sources.list.d/ubuntu.sources \
    && printf 'Types: deb\nURIs: http://ports.ubuntu.com/ubuntu-ports\nSuites: noble noble-updates noble-security\nComponents: main universe\nArchitectures: arm64\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n' \
        > /etc/apt/sources.list.d/ports.sources \
    && dpkg --add-architecture arm64 \
    && apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg git python3 zstd xz-utils file binutils pkg-config make cmake perl \
        build-essential g++-aarch64-linux-gnu \
        libc6-dev:arm64 libstdc++-13-dev:arm64 \
        libxkbcommon-dev:arm64 libwayland-dev:arm64 libx11-dev:arm64 libdbus-1-dev:arm64 libssl-dev:arm64 \
        libglib2.0-dev:arm64 libasound2-dev:arm64 libpipewire-0.3-dev:arm64 libudev-dev:arm64 \
    && rm -rf /var/lib/apt/lists/*
# apt.llvm.org's key, trusted only with its published fingerprint.
RUN curl -fsSL --proto '=https' --tlsv1.2 https://apt.llvm.org/llvm-snapshot.gpg.key -o /tmp/llvm.asc \
    && fpr=$(gpg --show-keys --with-colons /tmp/llvm.asc | awk -F: '$1 == "fpr" { print $10; exit }') \
    && [ "$fpr" = 6084F3CF814B57C1CF12EFD515CF4D18AF4F7421 ] \
    && gpg --dearmor -o /usr/share/keyrings/llvm.gpg /tmp/llvm.asc && rm /tmp/llvm.asc \
    && echo 'deb [arch=amd64 signed-by=/usr/share/keyrings/llvm.gpg] https://apt.llvm.org/noble/ llvm-toolchain-noble-22 main' \
        > /etc/apt/sources.list.d/llvm.list \
    && apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        clang-22 lld-22 llvm-22 libclang-22-dev \
    && rm -rf /var/lib/apt/lists/*
# Compiler wrappers for the target, so cc-rs, CMake and rustc's linker agree.
RUN for t in clang clang++; do \
        printf '#!/bin/sh\nexec /usr/bin/%s-22 --target=aarch64-linux-gnu "$@"\n' "$t" > /usr/local/bin/aarch64-linux-gnu-$t; \
        chmod +x /usr/local/bin/aarch64-linux-gnu-$t; done \
    && ln -s /usr/bin/ld.lld-22 /usr/local/bin/ld.lld && ln -s /usr/bin/lld-22 /usr/local/bin/lld
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH \
    CC=clang-22 CXX=clang++-22 \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-clang CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-clang++ \
    AR_aarch64_unknown_linux_gnu=llvm-ar-22 \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-clang \
    PKG_CONFIG_ALLOW_CROSS=1 \
    PKG_CONFIG_LIBDIR_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig \
    BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_linux_gnu=--target=aarch64-linux-gnu \
    LIBCLANG_PATH=/usr/lib/llvm-22/lib
RUN curl -fsSL --proto '=https' --tlsv1.2 https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.97.1 --no-modify-path \
    && rustup target add aarch64-unknown-linux-gnu --toolchain 1.97.1 \
    && chmod -R a+rwX /opt/rustup /opt/cargo
