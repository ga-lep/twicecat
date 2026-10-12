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

In the app, open **Settings → Voice & audio** and choose **DeepFilterNet3** as noise suppression. When the full build finds a GPU it can use, a **Use the GPU** checkbox appears, and the card shows where the cleaning runs, for example "Running on the GPU (CUDA)". If no GPU is offered, the card says why: the light build has no GPU support, or the full build found no usable GPU.
