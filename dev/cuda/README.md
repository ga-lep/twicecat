# CUDA header for the NVENC build (fetched, never committed)

`.cargo/config.toml` points `CUDA_HOME` here. webrtc-sys compiles LiveKit's
NVENC encoder and NVDEC decoder only when `$CUDA_HOME/include/cuda.h` exists
at build time (webrtc-sys 0.3.49 build.rs:281-313); without it the build goes
on without them and video is encoded in software. `libcuda` and `libnvcuvid`
are loaded at run time, so the binary still runs on machines without NVIDIA.

`scripts/fetch-cuda-headers.sh` takes `cuda.h` from NVIDIA's CUDA runtime
wheel (pinned in `scripts/checksums/cuda-runtime.sha256`), keeps it in the
user cache and links `include` (and NVIDIA's licence, `licenses`) here.

webrtc-sys's build script does not watch this directory: after fetching the
header, run `cargo clean -p webrtc-sys` once so the next build includes NVENC.
A `CUDA_HOME` from the environment (a system CUDA toolkit) takes precedence
over the config.
