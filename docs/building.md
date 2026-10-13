# Building Twicecat

There are two builds of the desktop app. They differ only in where voices are cleaned.

| Build | Command | Voice cleaning |
| --- | --- | --- |
| Light (default) | `cargo run --release -p twicecat-app` | CPU only: WebRTC, RNNoise, DeepFilterNet3 on the CPU |
| Full | `cargo run --release -p twicecat-app --features gpu` | Also DeepFilterNet3 on the GPU (ONNX Runtime) |

`--release` matters for calls: the audio chain is much slower in a debug build.

RustRover has a run configuration for each build, shared in `.run/`. They appear in the run menu as **Twicecat full (GPU)** and **Twicecat light (CPU only)**.

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

## Push-to-talk on Linux

On Wayland the global push-to-talk key goes through the desktop's shortcuts portal (KDE Plasma, and GNOME and Hyprland in recent versions). The portal knows Twicecat by its app ID, `io.github.ga_lep.Twicecat`, and looks for a desktop entry with that name. A build run with `cargo run` has none, so install the one in the repository once:

```bash
install -Dm644 packaging/linux/io.github.ga_lep.Twicecat.desktop ~/.local/share/applications/io.github.ga_lep.Twicecat.desktop
```

The first time push-to-talk starts (a call, the mic test, or the Voice & audio card in push-to-talk mode), the desktop asks you to confirm the shortcut, and may let you pick another key. The card then shows the key it chose. The entry's `Exec=twicecat` only matters for starting Twicecat from the app menu, which needs `twicecat` on your `PATH`.

On X11, Windows and macOS the key is registered directly; nothing needs installing. In SteamOS game mode (gamescope) there is no global shortcut: the key works while Twicecat's window has focus, and a gamepad button works everywhere.

To see what push-to-talk receives on a desktop, without the app: `cargo run -p twicecat-ptt --example ptt_probe -- --key ctrl+shift+KeyT --button left_thumb`.

## Disk use

Cargo never deletes an older build. Every new mix of features (the GPU build, `--all-features`), of packages picked with `-p`, or of dependencies gets a full set of build files beside the others, so `target/debug` only grows. One debug build of all the tests is about 27 GB: each of the ~40 test binaries links the core and WebRTC.

- On Linux, debug info goes into split DWARF files (`-C split-debuginfo=unpacked` in `.cargo/config.toml`) instead of into every test binary, which roughly halves each binary. Backtraces, gdb and lldb still show file and line.
- `scripts/trim-target.sh [GB]` removes `target/debug` once it is over the limit (80 GB by default, or `TWICECAT_TARGET_LIMIT_GB`); release builds stay. Run it now and then, or from a timer.
- `cargo clean --profile dev` removes the debug builds at once.
