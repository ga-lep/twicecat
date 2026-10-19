# Ubuntu 24.04 with what the Linux packages need to build: glibc 2.39, so
# they run on the Steam Frame (SteamOS 0.4.5), Ubuntu 24.04, Debian 13 and
# Fedora 43 or newer (Phase 7 plan Q14). Clang 22 from apt.llvm.org (the
# distribution's clang 18 is too old for libwebrtc's libc++), Rust 1.97.1 and
# cargo-about. Release CI builds the Linux packages in it on x86_64 and
# aarch64; build and run it the same way locally, as your own user, with the
# repository and the Twicecat cache mounted at their usual paths:
#
#   docker build -t twicecat-ubuntu-24.04-build dev/ubuntu-24.04
#   docker run --rm --user "$(id -u):$(id -g)" -e HOME="$HOME" -e CARGO_TARGET_DIR=target/ubuntu-24.04 \
#       -v "$PWD:$PWD" -v "$HOME/.cache/twicecat:$HOME/.cache/twicecat" \
#       --workdir "$PWD" twicecat-ubuntu-24.04-build scripts/package.sh --profile release
FROM ubuntu:24.04
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl git gnupg python3 python3-venv zstd xz-utils file binutils pkg-config \
    libxkbcommon-dev libwayland-dev libx11-dev libdbus-1-dev libssl-dev libglib2.0-dev \
    libasound2-dev libpipewire-0.3-dev libudev-dev desktop-file-utils appstream \
    && rm -rf /var/lib/apt/lists/*
# apt.llvm.org's key, trusted only with its published fingerprint.
RUN curl -fsSL --proto '=https' --tlsv1.2 https://apt.llvm.org/llvm-snapshot.gpg.key -o /tmp/llvm.asc \
    && fpr=$(gpg --show-keys --with-colons /tmp/llvm.asc | awk -F: '$1 == "fpr" { print $10; exit }') \
    && [ "$fpr" = 6084F3CF814B57C1CF12EFD515CF4D18AF4F7421 ] \
    && gpg --dearmor -o /usr/share/keyrings/llvm.gpg /tmp/llvm.asc && rm /tmp/llvm.asc \
    && echo 'deb [signed-by=/usr/share/keyrings/llvm.gpg] https://apt.llvm.org/noble/ llvm-toolchain-noble-22 main' \
        > /etc/apt/sources.list.d/llvm.list \
    && apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends clang-22 lld-22 \
    && rm -rf /var/lib/apt/lists/* \
    && ln -s /usr/bin/ld.lld-22 /usr/local/bin/ld.lld && ln -s /usr/bin/lld-22 /usr/local/bin/lld
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH CC=clang-22 CXX=clang++-22
RUN curl -fsSL --proto '=https' --tlsv1.2 https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.97.1 --no-modify-path \
    && chmod -R a+rwX /opt/rustup /opt/cargo
RUN cargo install cargo-about@0.9.2 --locked --features cli && rm -rf /opt/cargo/registry/* && chmod -R a+rwX /opt/cargo
