# Building Twicecat

There are two builds of the desktop app. They differ only in where voices are cleaned.

| Build | Command | Voice cleaning |
| --- | --- | --- |
| Light (default) | `cargo run --release -p twicecat-app` | CPU only: WebRTC, RNNoise, DeepFilterNet3 on the CPU |
| Full | `cargo run --release -p twicecat-app --features gpu` | Also DeepFilterNet3 on the GPU (ONNX Runtime) |

`--release` matters for calls: the audio chain is much slower in a debug build.

RustRover has a run configuration for each build, shared in `.run/`. They appear in the run menu as **Twicecat full (GPU)** and **Twicecat light (CPU only)**.

## Hardware video encoding (NVIDIA, Linux)

Both builds encode call video with LiveKit's own encoders. On Linux, LiveKit's NVENC encoder and NVDEC decoder are compiled in only when NVIDIA's `cuda.h` is there at build time. Get it once with `scripts/fetch-cuda-headers.sh` (about 2 MB, from NVIDIA's pinned CUDA runtime wheel, under NVIDIA's licence). It is linked into `dev/cuda`, where `.cargo/config.toml` points `CUDA_HOME`. A system CUDA toolkit works too: set `CUDA_HOME` to it.

If you built before fetching the header, run `cargo clean -p webrtc-sys` once. webrtc-sys does not notice the header appearing.

Without the header, or on a machine without an NVIDIA GPU, video is encoded on the CPU (VP8, up to 1080p30). The NVIDIA libraries are loaded at run time, so the same binary runs everywhere.

## What the full build needs at run time

Nothing GPU-related is linked at build time. The full build loads these files when a call or mic test starts. Each script checks a pinned SHA-256 and unpacks into `~/.cache/twicecat/`.

- **The DeepFilterNet3 models**, which both builds use: `scripts/fetch-models.sh` (about 8 MB). Without them, DeepFilterNet3 falls back to RNNoise.
- **ONNX Runtime**: `scripts/fetch-onnxruntime.sh cuda13` for NVIDIA. That is 240–450 MB; the script lists the other variants.
- **NVIDIA's CUDA and cuDNN libraries**, unless they are installed system-wide: `scripts/fetch-cuda-runtime.sh` (about 1.1 GB, under NVIDIA's licences).
- **The GPU graph of DeepFilterNet3**: `scripts/export-deepfilternet.sh`. It builds a Python venv with CPU PyTorch, about 1 GB, and needs `uv` or Python 3.13.
- **Optional, for more backends:** TensorRT (`scripts/fetch-tensorrt.sh`, a 3.7 GB download) and WebGPU (`scripts/fetch-webgpu-ep.sh`).

In the app, open **Settings → Voice & audio**.

- **Where each option runs.** Every noise suppression choice says whether it runs on the CPU or the GPU.
- **The GPU entry.** When the full build finds a GPU, the menu adds **DeepFilterNet3 on the GPU**, naming the backend it starts on:
  - NVIDIA's own: **NVIDIA · TensorRT** or **NVIDIA · CUDA**.
  - Any vendor's GPU: **any GPU · WebGPU on Vulkan** (Direct3D 12 on Windows, Metal on macOS), or **any GPU · DirectML**.
  - Apple's: **Apple · Core ML**.
- **The GPU line.** It says which backend is used and which others are available here. Without a GPU it says why: the light build has no GPU support, or the full build found no usable GPU.
- **Cleaning other voices.** It offers **Clean on the GPU** with the same backend name.
- **During a call or a mic test**, the card shows where the cleaning actually runs, for example "Running on the GPU (NVIDIA · CUDA)". If the GPU fails or falls behind, it says the CPU took over. "Falls behind" means late on 10 of the last 100 frames (one second), not one slow frame. Your microphone's cleaning then tries the GPU again after 30 seconds, and waits twice as long after each try that falls behind again, up to 5 minutes.

The core prefers TensorRT, then CUDA, DirectML, WebGPU and Core ML, and uses the first that starts.

## Release builds

Releases are built with the `dist` profile: `cargo build --profile dist -p twicecat-app`. It is `release` with fat link-time optimisation over one codegen unit: 1–3 % faster on the voice chain and 22 % smaller once stripped, but 16 minutes instead of 4½ on a 24-core laptop. Use `--release` for everyday work.

The CPU level is set per target in `.cargo/config.toml`. x86_64 Linux and Windows build for x86-64-v3 (AVX2, FMA, BMI2), which saves about 11 % of the voice chain against the x86-64 baseline. The app refuses an older CPU at start with a message instead of crashing: on Linux on stderr, on Windows in a message box. macOS and aarch64 use the target's default. Phase 7's plan (`docs/plans/phase7-packaging.md`) has the measurements behind these choices.

To measure them again, on this machine or another:

```bash
scripts/measure-builds.sh
```

It builds each CPU level, prints the size of `twicecat` and times the voice chain per 10 ms on one core of each kind. The defaults are x86-64 and v3, plus v4 where the CPU has AVX-512; on aarch64, generic, Cortex-A76 and the CPU's own. `--variants`, `--profile dist`, `--cores`, `--rounds` and `--frames` change that. Other levels are built with `--target`, so their flags never reach the build scripts, which run on this machine. The "Profile" workflow (Actions tab) runs the same script on GitHub's x86_64 or arm runner.

## Packages

`scripts/package.sh` builds the release binary and packages it for the machine it runs on, into `target/package/<triple>/`:

| Machine | Packages | Made by |
| --- | --- | --- |
| Linux | `.tar.zst`, `.deb`, `.rpm`, AppImage, plus a symbols archive | nfpm, appimagetool |
| macOS | `.app` in a DMG | cargo-packager |
| Windows | MSI | cargo-packager |

Every package carries the full (GPU) build, the DeepFilterNet3 models, `LICENSE` and `THIRD_PARTY_NOTICES.html`. Linux packages also carry the "any GPU" pack: ONNX Runtime's CPU build, its WebGPU plugin and the stateful graph. On Linux these resources go in `<prefix>/lib/twicecat`, beside `<prefix>/bin/twicecat`. They don't go in `<prefix>/share/twicecat`, because that is the app's data directory when the `.tar.zst` is unpacked into `~/.local`. They need:

- the models, ONNX Runtime and the WebGPU plugin, which the script fetches with the pinned fetch scripts;
- the stateful graph from `scripts/export-deepfilternet.sh`, or `TWICECAT_STATEFUL_MODEL` (checked against its pinned SHA-256);
- the packaging tools, fetched pinned by `scripts/fetch-packaging-tools.sh`: nfpm, appimagetool and the AppImage runtime (Linux), or `cargo install cargo-packager@0.11.8 --locked` (macOS, Windows);
- `cargo install cargo-about@0.9.2 --locked --features cli`, for the notices.

Options: `--profile release` (faster, for testing; releases use `dist`), `--formats "tar deb"`, `--no-build`.

Linux packages need the build machine's glibc or newer. Release CI builds them in the Ubuntu 24.04 image in `dev/ubuntu-24.04/Dockerfile` (glibc 2.39), so they run on the Steam Frame, Ubuntu 24.04, Debian 13, and Fedora 43 or newer. A package built on a newer distribution needs that distribution's glibc. `package.sh` stops when the binary needs a newer glibc than the floor `packaging/nfpm.yaml` declares, so build in that image; its header has the commands.

To build for the Steam Frame (aarch64) on an x86_64 machine, use the cross image in `dev/ubuntu-24.04-aarch64-cross/Dockerfile`: it builds the binary in about 5 minutes, and `scripts/package.sh --no-build` packages it on an aarch64 machine.

`scripts/smoke-test-packages.sh target/package/<triple>` then checks each Linux package headless:

- **`.tar.zst` and AppImage:** run here.
- **`.deb`:** installed with apt in `ubuntu:24.04` and `ubuntu:26.04`.
- **`.rpm`:** installed with dnf in `fedora:43` and `fedora:44`.

Each must print its version, resolve every library and carry its resources where the app looks for them.

### Flatpak

The Flatpak (`packaging/flatpak/io.github.ga_lep.Twicecat.yml`) is built from this source tree on the Freedesktop 26.08 runtime, with clang 22 from its LLVM extension and Rust 1.97.1 from the standalone installer. The build is offline: every crate comes from `packaging/flatpak/cargo-sources.json`, and every other download (libwebrtc, the models, the "any GPU" pack, CUDA's header) is a source pinned by the same SHA-256 as in `scripts/checksums/`. The fetch scripts take those files from `TWICECAT_DOWNLOADS` instead of the network, and `scripts/package.sh --formats prefix` installs the same tree as the `.deb` into `/app`. It needs `flatpak` and `flatpak-builder` from your distribution.

```bash
scripts/flatpak.sh build
```

That makes `target/package/flatpak/Twicecat-<version>-<arch>.flatpak` for this machine's architecture (x86_64 or aarch64). Then `scripts/flatpak.sh smoke` installs it for your user and checks `--version` and the bundled files.

When `Cargo.lock` changes, regenerate the crate list with `scripts/flatpak.sh sources` and commit it. CI's `scripts/flatpak.sh check` fails until you do. It uses flatpak-cargo-generator.py at a pinned commit in a small Python venv.

### Releases

The "Release" workflow (`.github/workflows/release.yml`) builds every package on GitHub's runners:

- **Linux x86_64 and aarch64:** `scripts/package.sh` and its smoke test, then the Flatpak in Flathub's CI image.
- **Windows x64:** the MSI.
- **macOS on Apple Silicon:** the DMG.

It runs on a `v*` tag, after `scripts/check-version.sh` passes, and makes a **draft** release (a pre-release when the tag has a label, as in `v0.1.0-alpha`; the packages keep the workspace version, as WiX takes only a numeric pre-release) with `SHA256SUMS` and build provenance attestations; publishing it is up to you. "Run workflow" on the Actions tab is a dry run: the same packages as one artifact, and no release. Signing runs only when its secrets exist (the workflow's header lists them); otherwise the job summary and the release notes say "unsigned".

## Push-to-talk on Linux

On Wayland the global push-to-talk key goes through the desktop's shortcuts portal (KDE Plasma, and GNOME and Hyprland in recent versions). The portal knows Twicecat by its app ID, `io.github.ga_lep.Twicecat`, and looks for a desktop entry with that name. A build run with `cargo run` has none, so install the one in the repository once:

```bash
install -Dm644 packaging/linux/io.github.ga_lep.Twicecat.desktop ~/.local/share/applications/io.github.ga_lep.Twicecat.desktop
```

The first time push-to-talk starts (a call, the mic test, or the Voice & audio card in push-to-talk mode), the desktop asks you to confirm the shortcut, and may let you pick another key. The card then shows the key it chose. The entry's `Exec=twicecat` only matters for starting Twicecat from the app menu, which needs `twicecat` on your `PATH`.

On X11, Windows and macOS the key is registered directly; nothing needs installing. In SteamOS game mode (gamescope) there is no global shortcut: the key works while Twicecat's window has focus, and a gamepad button works everywhere.

A mouse button (the middle one, the side buttons "mouse 4" and "mouse 5", or on X11 and macOS another one, picked with **Set key…**) works in every app on X11 (XInput 2.1 or newer), Windows and macOS. Other apps still receive its clicks, so a side button still goes back in a browser. Wayland shows an app the mouse only over its own windows, so there, and in game mode, the button works while Twicecat's window has focus.

To see what push-to-talk receives on a desktop, without the app: `cargo run -p twicecat-ptt --example ptt_probe -- --key ctrl+shift+KeyT --mouse back --button left_thumb`.

## Disk use

Cargo never deletes an older build. Every new mix of features (the GPU build, `--all-features`), of packages picked with `-p`, or of dependencies gets a full set of build files beside the others, so `target/debug` only grows. One debug build of all the tests is about 27 GB: each of the ~40 test binaries links the core and WebRTC.

- On Linux, debug info goes into split DWARF files (`-C split-debuginfo=unpacked` in `.cargo/config.toml`) instead of into every test binary, which roughly halves each binary. Backtraces, gdb and lldb still show file and line.
- `scripts/trim-target.sh [GB]` removes `target/debug` once it is over the limit (80 GB by default, or `TWICECAT_TARGET_LIMIT_GB`); release builds stay. Run it now and then, or from a timer.
- `cargo clean --profile dev` removes the debug builds at once.
