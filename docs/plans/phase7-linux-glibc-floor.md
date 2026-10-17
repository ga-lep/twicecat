# Linux glibc Floor and the Steam Frame (Phase 7 amendment)
Status: ANSWERED (human, 2026-10-17): Q14 (c), with (d), as recommended, and apt.llvm.org approved as a package source for release CI. Written 2026-10-17 after installing Twicecat on a Steam Frame by hand. Implemented in Task 6's commit (`dev/ubuntu-24.04`, the cross image, the floor in `nfpm.yaml`, `package.sh`'s glibc guard, the smoke-test images) and Task 8's (the release workflow builds in the image). It amends Phase 7 Tasks 6 and 8 (`docs/plans/phase7-packaging.md`) and touches Q13's "older distributions are not served for now".

## Problem
Release CI builds the Linux packages on `ubuntu-26.04` and `ubuntu-26.04-arm` (`.github/workflows/release.yml`, job `linux`), so they need glibc 2.43, and `packaging/nfpm.yaml` says so (`libc6 (>= 2.43)`, `glibc >= 2.43`). The Steam Frame, one of the four first-release targets (Q1), has glibc 2.39. None of the aarch64 `.tar.zst`, AppImage, `.deb` or `.rpm` starts on it.

The Flatpak is not affected: it brings its own runtime (`org.freedesktop.Platform` 26.08), and it is the plan's route for the Frame (Task 11, `docs/install.md`; the checkpoint's Frame item). These are affected:
- **The aarch64 `.tar.zst` and AppImage:** the "Frame without Flatpak" route Q5 names. The AppImage's self-update (Q7) is also lost there.
- **The AUR `twicecat-bin` (Task 12):** the checkpoint says `yay -S twicecat-bin` "on Arch or SteamOS desktop mode". That fails on the Frame, since the package unpacks the same `.tar.zst`.
- **Common x86_64 distributions** (measured in containers 2026-10-17): Ubuntu 24.04 LTS (2.39), Debian 13 (2.41) and Fedora 43 (2.42) are below the floor. Q13 accepted that "for now". Whether the Steam Deck's SteamOS is above or below 2.43 was not measured.

## Facts verified (2026-10-17)
- **The Frame:** SteamOS `VERSION_ID=0.4.5`, `VARIANT_ID=vr`, `BUILD_ID=20261007.6125817`. Kernel 6.18, aarch64, 8 cores, 16 GB, Adreno 750 (Mesa Turnip 26.3.0-devel). `ldd (GNU libc) 2.39`, clang 19.1.7, GCC 15.1.1. It has lld, cmake, pkg-config, podman and flatpak, plus the headers for PipeWire 1.6.8, ALSA 1.2.11, udev 257, Wayland, X11, D-Bus, OpenSSL and GLib. The root filesystem is read-only (`steamos-readonly`); `/home` is writable.
- **A native build on the Frame fails:** `webrtc-sys` 0.3.49's `build.rs:598` refuses clang 19, because LiveKit's libwebrtc ships a hermetic libc++ that needs clang 21 or newer. SteamOS cannot build Twicecat without a newer clang, for example in a container.
- **A build against glibc 2.39 works.** In an x86_64 `ubuntu:24.04` container I cross-compiled with arm64 multiarch packages from `ports.ubuntu.com`, `g++-aarch64-linux-gnu`, and `clang-22`, `lld-22` and `libclang-22-dev` from apt.llvm.org (`llvm-toolchain-noble-22`). The command was `cargo build --locked --release --target aarch64-unknown-linux-gnu -p twicecat-app --bin twicecat --features gpu`, with libwebrtc from `scripts/fetch-libwebrtc.sh linux-arm64`. It built first time in 5 min 3 s on 24 threads.
  - The binary's newest symbol version is `GLIBC_2.39`, and its `NEEDED` list equals the one `package.sh` checks.
  - `scripts/package.sh --profile release --formats tar --no-build`, run on the Frame on that binary, passed all its checks: `NEEDED`, the stateful graph's SHA-256 and the notices.
  - Unpacked under `~/.local/opt/twicecat`, it started under gamescope (game mode, Xwayland `:0`). wgpu picked Vulkan on the Turnip Adreno 750, with no error in the log.
  - Task 6 had measured the same rule on another machine: a build on CachyOS needs `GLIBC_2.44`, its own glibc. The floor is the build image's glibc.
- **Ubuntu 24.04 has every package dependency under the names `nfpm.yaml` already uses:** `libpipewire-0.3-0t64` 1.0.5, `libasound2t64` 1.2.11, `libudev1` 255.4, `libgcc-s1` 14.2, and the recommends `libwayland-client0`, `libxkbcommon0`, `libxkbcommon-x11-0`, `libx11-xcb1` and `libvulkan1`. The `pipewire` crate is built with `v0_3_53`, which PipeWire 1.0.5's headers satisfy, since the cross build compiled against them.

## Q14. Which glibc floor for the Linux packages?
- **(a) Flatpak only for the Frame.** No build change. Write the 2.43 floor into `docs/install.md`, send Frame users to the Flatpak, and drop "SteamOS desktop mode" from the AUR checkpoint item.
  - Cost: none.
  - Downsides: the Frame has no unsandboxed build, the Flatpak in game mode is still unverified (Secret Service, a checkpoint item), and the aarch64 AppImage and `.tar.zst` serve only aarch64 distributions with glibc 2.43 or newer.
- **(b) Build only aarch64 in Ubuntu 24.04.** The `ubuntu-26.04-arm` runner builds natively inside `container: ubuntu:24.04` with clang 22 from apt.llvm.org; x86_64 stays on 26.04.
  - Gains: the Frame, Ubuntu 24.04 on arm and Debian 13 on arm.
  - Costs: two floors to document, and `nfpm.yaml` takes the floor as a variable per architecture.
- **(c) (R) Build both Linux architectures in Ubuntu 24.04**, the same container on both runners. One floor, glibc 2.39: the Frame, Ubuntu 24.04 LTS, Debian 13, Fedora 43 and newer, and the Steam Deck if its SteamOS has 2.39 or newer.
  - Cost: the same work as (b), with one floor instead of two.
  - It reverses Q13's "older distributions are not served for now", which is why it is a question and not a fix.
- **(d) Also, with any of a–c: keep the cross-compile image for local builds** as `dev/ubuntu-24.04-aarch64-cross/Dockerfile`, beside `dev/ubuntu-26.04/` or replacing it under (c). It is how the Frame's binary was built today, and it builds aarch64 packages on an x86_64 machine in minutes. I don't recommend it for CI, which already has native arm runners and tests on them.

### Considerations for (b) and (c)
- **apt.llvm.org is a new package source** for release CI (rule 3 and Q11 style approval). Ubuntu 24.04 ships clang 18, which is too old for libwebrtc. The image would fetch LLVM's signing key over HTTPS and check its fingerprint before trusting it. The alternative is LLVM's own release tarball, pinned by SHA-256 like the other tools in `scripts/checksums/`.
- **Rust comes from rustup in the container** at the toolchain `rust-toolchain.toml` pins, as `dev/ubuntu-26.04/Dockerfile` already does.
- **What gets tested:** tests keep running on `ubuntu-26.04` (`ci.yml`), so released packages are built against older system headers than the tested build, with the same crates and Rust. The smoke test covers what that changes: the package installs, every library resolves, and the app prints its version.
- **A guard that keeps the floor true:** `package.sh` reads the binary's newest `GLIBC_` symbol version with `objdump -T` and stops if it is above the floor `nfpm.yaml` declares. A later change of build image can then not raise the floor silently.

### Files for (c)
- `.github/workflows/release.yml` (job `linux`): `container: ubuntu:24.04` on both runners, an install step for the packages, apt.llvm.org's clang 22 and rustup, and `CC`/`CXX` as today. The Flatpak job is unchanged.
- `packaging/nfpm.yaml`: `libc6 (>= 2.39)` and `glibc >= 2.39`, with the comment updated.
- `scripts/package.sh`: the comment, and the glibc guard above.
- `scripts/smoke-test-packages.sh`: the `.deb` in `ubuntu:24.04` (the oldest served) and the `.rpm` in `fedora:43`, plus the newest of each if the run time allows.
- `dev/ubuntu-24.04/Dockerfile`, replacing `dev/ubuntu-26.04/`, plus the cross image from (d); `docs/building.md` ("Packages"); `docs/install.md` (Task 11).
- `docs/plans/phase7-packaging.md`: Q13's note and the release matrix. PROGRESS.md and the ledger.
- Per AGENTS.md rule 5 the change goes into the commits that added the code: fixups of Task 6 (`feat(packaging): build the Linux packages…`) and Task 8 (`feat(ci): build, sign and draft every release…`). It is not a new `fix` commit, because none of this is on `main` yet.

### Acceptance for (c)
- In Docker on the developer machine, the `ubuntu:24.04` image builds the x86_64 packages. The newest `GLIBC_` symbol version is 2.39 or lower, and the guard fails when the binary is built on 26.04 instead (mutation check).
- The smoke test passes for the `.deb` in `ubuntu:24.04` and the `.rpm` in `fedora:43`.
- The cross image builds the aarch64 binary, and the Frame unpacks, starts and finds its resources (done by hand on 2026-10-17 with the `release` profile; repeated with `dist`).
- `actionlint` and `shellcheck` are clean on the changed workflow and scripts. The release workflow's dry run is the human's (Task 8).

### Human checkpoint additions (rule 9)
- [ ] Steam Frame, without Flatpak: the aarch64 `.tar.zst` and AppImage start in desktop mode and, through "Add a non-Steam game", in game mode. Login is kept (Secret Service in each mode), and a call works on the built-in mic and speakers.
- [ ] The Steam Deck: `ldd --version` in desktop mode, to record whether the floor covers it.
