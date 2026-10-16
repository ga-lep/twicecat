# Phase 7 Packaging and Release Plan
Status: APPROVED (2026-10-14). The human answered Q2 (a), Q3 (b, bundle the "any GPU" pack), Q5 (MSI), Q6 (Windows unsigned for now), Q7 (a check with a link on Windows and macOS; the package manager and cargo-packager-updater on Linux), Q8 (a) and Q9 (a, our own Flatpak), approved Q1, Q4, Q10, Q11 and Q12 as recommended, and approved the plan. Added 2026-10-14 at the human's request: apt, dnf and AUR distribution (Q13, Task 12).

## Goal
Roadmap Phase 7: installers for each OS built by CI on a version tag, signing hooks, updates, opt-in crash reports, and the Flatpak sandbox tested before release. The human added three targets on 2026-10-14:
- **Serve four kinds of machines:** Windows, Linux (Wayland and X11), macOS, and the **Steam Frame** (SteamOS on a Snapdragon, so Linux aarch64).
- **Use modern hardware well:** AMD and NVIDIA GPUs, and modern x86_64 CPUs.
- **Measure** whether lighter and heavier builds (CPU levels, the GPU build, the release profile) are worth shipping side by side, or whether the difference is negligible. The numbers are in "Measurements" below; Q2, Q3 and Q4 rest on them.

The work happens on branch `feat/phase7-packaging`, cut from `origin/main` at `072206e`. Phase 9 (video, `feat/video`) is not on `main`; what it will add to packaging is listed under "After Phase 9" so the formats chosen here already fit it.

**Phase order (rule 1).** The Phase 6 human checkpoint still has open items (calls by ear, Element Call interop, push-to-talk on each OS, the Frame's call defaults). The human asked for Phase 7 now. Packaging does not change call code, and several Phase 6 items (Flatpak, the Frame, macOS prompts) can only be checked on packaged builds, so this plan does not wait for them; it does not tick them either.

Rules for every task (AGENTS.md):
- **Rule 2:** every tool and crate API used is read in its source or docs at the pinned version first and cited in PROGRESS.md (cargo-packager's config keys, flatpak-builder's manifest keys, GitHub Actions inputs).
- **Rule 3:** no pinned version changes. New build tools (cargo-packager, cargo-about, flatpak-builder) and any new crate are listed in Q11 for approval; each is pinned exactly where it is installed.
- **Rule 5:** one `feat` or `docs` commit per task, with a `Step: phase7.<n>` trailer; PROGRESS.md and `docs/plans/phase7-packaging.progress.md` updated in the same commit.
- **Rule 6 and 8:** the agent never handles signing certificates, Apple credentials, store accounts or real Matrix accounts. Workflow steps that sign run only when the human has added the secrets; without them the release is unsigned and says so.
- **Rule 9:** installing on clean machines, OS prompts, SmartScreen and Gatekeeper, the Flatpak sandbox on a desktop, and the Frame itself go on the human checkpoint.
- **Gate before each commit:** `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings` (and `--all-features`); `cargo test --workspace`; `scripts/check-app-deps.sh` (also with `livekit`); `scripts/check-core-api.sh`; `scripts/check-commits.sh main`; plus, from the task that adds them, `actionlint` on new workflows (or a YAML check when it is not installed, as in Phase 6), `desktop-file-validate` and `appstreamcli validate`.

## Facts verified while planning (2026-10-14)
- **No release profile.** `Cargo.toml` tunes only `[profile.dev]`. `cargo build --release` uses Cargo's defaults: `opt-level = 3`, thin local LTO, 16 codegen units, unwinding, symbols not stripped.
- **`panic = "abort"` is not an option:** `crates/audio/src/pw/thread.rs:129` relies on `catch_unwind` to keep a panic in the PipeWire thread from taking the app down.
- **CPU levels today** (`.cargo/config.toml`): x86_64 Linux builds for x86-64-v3; x86_64 Windows and macOS stay at the x86-64 baseline (Phase 6 Task 15 part 2). `twicecat_core::require_cpu_level` (`crates/core/src/cpu_level.rs`) refuses a CPU below v3 at start on Linux only, and only in a build that targets v3.
- **Steam hardware survey, September 2026** (store.steampowered.com/hwsurvey): AVX2 95.95 %, FMA 96.12 %, BMI2 95.91 %, AVX512F 27.46 %, SSE4.2 98.21 %. A v3-only build turns away about 4 % of Steam's PCs; AVX-512 (x86-64-v4) is on about a quarter, mostly AMD Zen 4 and 5.
- **libwebrtc is prebuilt and statically linked**, pinned by SHA-256 for six targets (`scripts/checksums/libwebrtc.sha256`): linux x64 and arm64, mac x64 and arm64, win x64 and arm64. Opus, AEC in libwebrtc and the video codecs are compiled by LiveKit, so our `target-cpu` does not reach them. Our flags reach the Rust code: sonora (AEC3, AGC2, NS), nnnoiseless, rubato, tract, matrix-sdk and its crypto.
- **The GPU build adds code, not libraries.** `--features gpu` adds `ort` with `load-dynamic`: ONNX Runtime and the vendor libraries are loaded at run time from what the `scripts/fetch-*.sh` scripts put in `~/.cache/twicecat`. Measured on this machine: ONNX Runtime 1.28.3 with CUDA 13 is 296 MB, NVIDIA's CUDA and cuDNN libraries 1.8 GB. No user-facing way to get them exists yet: the scripts are developer tools.
- **Which GPU runs what** (Phase 6 Task 13b): NVIDIA through TensorRT or CUDA; AMD and Intel through WebGPU on Vulkan (Linux, Windows) or DirectML (Windows); Apple through Core ML. The window itself draws with wgpu on Vulkan, Direct3D 12 or Metal, on any vendor. ONNX Runtime has no aarch64 CUDA build, but its CPU build and the WebGPU plugin both exist for Linux aarch64 (`scripts/checksums/`), so the Frame could clean on its GPU through Vulkan (not tried).
- **The DeepFilterNet3 models** (8.3 MB, MIT or Apache-2.0) are looked up only in `TWICECAT_MODEL_DIR` or `~/.cache/twicecat/models` (`crates/audio/src/dfn/mod.rs:45-48`, `crates/audio/src/paths.rs`); `cache_dir()` reads `XDG_CACHE_HOME` or `HOME`, which Windows does not set by default. A packaged app finds no models and falls back to RNNoise.
- **The binary:** `twicecat` (`crates/app`, version 0.1.0 from its own `Cargo.toml`, not the workspace). It has `--help`, `--data-dir`, `--homeserver`, `--ephemeral`, and refuses anything else, so there is no `--version` yet (the roadmap's smoke test needs one). On Windows it has no `windows_subsystem` attribute, so a console window opens beside it.
- **Linux desktop files:** `packaging/linux/io.github.ga_lep.Twicecat.desktop` exists (no icon). There is no icon file and no AppStream metainfo. The tray icon is drawn in code (`crates/app/src/tray.rs:40-42`, a rounded square with a "T").
- **Sandbox-relevant services** (all through D-Bus or portals, which matters for Flatpak): keychain through the Secret Service (`keyring` 4.2.0 with `zbus-secret-service-keyring-store`), notifications through `org.freedesktop.Notifications` (`notify-rust`), the tray through StatusNotifierItem (`tray-icon` with `ksni`), push-to-talk through the GlobalShortcuts portal (`ashpd`), file dialogs through the portal (`rfd` `xdg-portal`), audio through our own PipeWire client, gamepads through evdev (`gilrs`).
- **Data directories:** `directories::ProjectDirs::from("io.github", "ga_lep", "Twicecat")` (`crates/core/src/paths.rs:84`), so Flatpak's `XDG_*` variables move the data under `~/.var/app/io.github.ga_lep.Twicecat/` without code changes.
- **CI today** (`.github/workflows/ci.yml`): tests on `ubuntu-26.04`, `ubuntu-26.04-arm` (aarch64, the Frame's stand-in), `windows-latest`, `macos-latest` (arm64). `profile.yml` profiles the call path on the arm runner on demand. GitHub also offers `windows-11-arm` and `macos-26-intel` runners.
- **Tools, latest releases** (crates.io, 2026-10-14): cargo-packager 0.11.8 (2025-11-27; MSI through WiX, NSIS, `.app` and DMG, AppImage, `.deb`, pacman; still "public preview"), cargo-packager-updater 0.2.3, cargo-deb 3.8.0, cargo-wix 0.3.9, cargo-bundle 0.12.0, cargo-about 0.9.2, sentry 0.49.3. None is installed here, nor `flatpak`/`flatpak-builder`; `desktop-file-validate`, `appstreamcli`, `lld` and `clang` are.
- **The Steam Frame:** Snapdragon 8 Gen 3 (Cortex-X4, A720 and A520 cores, Armv9.2-A), SteamOS for aarch64. Native `linuxarm64` builds run directly; desktop mode is KDE Plasma with Flathub, as on the Steam Deck. Game mode is gamescope, where there is no global shortcut (already handled) and possibly no Secret Service (not checked: human item).

## Measurements (2026-10-14, this machine)
Machine: Intel Core Ultra 9 275HX (Arrow Lake: 8 P-cores 0–7, 16 E-cores 8–23; AVX2, no AVX-512), 30 GB RAM, RTX 5080 Laptop. Rust 1.97.1. Each variant was built from `072206e` into its own target directory with `RUSTFLAGS="-C target-cpu=…"` (which replaces `.cargo/config.toml`'s flags) and, for "dist", `CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`. Speed is `voice_profile --tsv chain 3000` (the per-10-ms processing chain, the CPU-heavy part of a call), pinned with `taskset` to a P-core (2) and an E-core (12), three interleaved rounds, median of the three medians. Task 2 turns the two throwaway scripts used here into `scripts/measure-builds.sh`.

### CPU levels and the release profile: speed
Median µs per 10 ms frame (median of three rounds; the rounds agree within ±1.5 µs). Lower is better; 1 % of one core is 100 µs.

| Chain | v1 | v2 | v3 (today) | native | v1 + LTO | v3 + LTO |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| **P-core:** default (HPF, AEC3, RNNoise, AGC2) | 97.7 | 100.5 | 86.5 | 85.3 | 96.3 | 84.1 |
| AEC3 | 51.1 | 55.0 | 43.5 | 44.5 | 50.0 | 42.5 |
| WebRTC NS (high) | 16.9 | 17.6 | 13.2 | 13.6 | 16.3 | 12.7 |
| RNNoise | 28.4 | 26.3 | 26.2 | 24.0 | 28.4 | 24.9 |
| DeepFilterNet3 (CPU) | 346.5 | 347.2 | 334.9 | 336.1 | 341.8 | 328.6 |
| default with DeepFilterNet3 | 421.0 | 426.3 | 400.6 | 401.5 | 415.4 | 392.5 |
| **E-core:** default | 121.1 | 128.9 | 108.9 | 108.5 | 121.9 | 107.6 |
| default with DeepFilterNet3 | 434.1 | 446.4 | 424.9 | 438.7 | 427.6 | 412.8 |

"v1" is the x86-64 baseline, "native" is `arrowlake-s`, "LTO" is fat LTO with one codegen unit. v4 (AVX-512) built (with `--target`, below) but cannot run here; it needs the AMD machine.

### Sizes and build times
| Build | Build time | `twicecat` | stripped | stripped, zstd -19 |
| --- | ---: | ---: | ---: | ---: |
| v3 release (today) | 4 min 24 s | 235.7 MB | 169.5 MB | 42.8 MB |
| v1 / v2 / native / v4 release | 4 min 15–31 s | 234.4–236.0 MB | 168.1–169.9 MB | |
| v3 release with `--features gpu` | 3 min 38 s (app only) | 236.4 MB | 170.0 MB | |
| v3 + fat LTO, 1 codegen unit | 16 min 19 s | 159.1 MB | 133.0 MB | 39.0 MB |
| v1 + fat LTO, 1 codegen unit | 16 min 4 s | 156.1 MB | 130.1 MB | |

The binary links only `libpipewire-0.3`, `libasound`, `libudev`, `libgcc_s` and glibc; Wayland, X11, xkbcommon, D-Bus (zbus is pure Rust) and Vulkan are loaded at run time.

### What the GPU paths cost to ship (downloads, pinned versions)
| Pack | Linux x64 | Windows x64 | Linux aarch64 | macOS arm64 |
| --- | ---: | ---: | ---: | ---: |
| ONNX Runtime 1.28.3, CPU build | 9 MB | 79 MB | 8 MB | 33 MB |
| + WebGPU plugin 0.4.0 (any GPU: AMD, Intel, NVIDIA over Vulkan, D3D12, Metal) | 7 MB | 13 MB | 6 MB | 5 MB |
| ONNX Runtime 1.28.3 with CUDA 13 (replaces the CPU build) | 241 MB | 366 MB | none | none |
| + NVIDIA CUDA runtime, cuBLAS, cuRAND, NVRTC, cuDNN | 1,128 MB | 951 MB | none | none |

On the RTX 5090 (Phase 6 Task 13b) one voice of DeepFilterNet3 costs this much CPU per frame: TensorRT 37 µs, CUDA 58 µs, WebGPU over Vulkan 420 µs, DirectML 110 µs. DeepFilterNet3 on the CPU costs about 335 µs. So on NVIDIA only the heavy pack actually frees the CPU, and the light "any GPU" pack is a CPU saver only where its driver path is cheaper (AMD's Vulkan numbers are still a human item). DirectML (Windows, any vendor) is cheap on the CPU but has to be built from source (15 min, `scripts/build-onnxruntime-directml.sh`).

### What the numbers say
- **CPU level: one binary per OS and architecture is enough.** v3 saves about 11 % of the voice chain against the baseline (P-core 97.7 → 86.5 µs, E-core 121.1 → 108.9 µs), but that is **11–12 µs per 10 ms, about 0.1 % of one core**. The CPU-heavy model (DeepFilterNet3 in tract) gains only 3 %, because tract, like aws-lc, already picks its AVX2, FMA or AVX-512 kernels at run time. A second x86_64 binary for older CPUs would buy 0.1 % of a core, and Arrow Lake tuning (`native`) buys nothing over v3. x86-64-v2 is slower than v1 here (AEC3 +8 %), so it is not a middle ground. The 17 % Phase 6 measured has shrunk to 11 % on this CPU.
- **v4 (AVX-512, for AMD Zen 4 and 5)** cannot gain more than what remains after tract's own dispatch: the autovectorised sonora code. Worth one measurement on the AMD machine before deciding. A v4 build also needs `require_cpu_level` to check AVX-512: today it checks only v3's bits, and a v4 app started here passed the check (it would have died later on the first AVX-512 instruction).
- **Building other CPU levels needs `--target`.** Without `--target`, `RUSTFLAGS` also applies to build scripts. Those run on the build machine: a v4 build on this CPU died in `zerocopy`'s build script with SIGILL. The release workflow passes `--target` always.
- **LTO: worth it for releases only.** Fat LTO with one codegen unit is 1–3 % faster, 22 % smaller stripped (170 → 133 MB installed), but only 9 % smaller to download (42.8 → 39.0 MB compressed), and it takes 16 minutes instead of 4½. A `dist` profile for release CI; development keeps `release`.
- **Light against full (GPU) build: negligible.** The `gpu` feature adds 0.7 MB (0.3 %) to the binary and nothing at start (ONNX Runtime is loaded only when a GPU mode is chosen). There is no reason to ship two binaries: every package can be the full build. The weight is in the GPU runtime packs, from 16 MB ("any GPU", Linux) to 1.37 GB (NVIDIA CUDA, Linux), and those are separate downloads, not separate binaries (Q3).
- **aarch64 (the Frame):** the WebGPU plugin and ONNX Runtime both exist for Linux aarch64 (6 + 8 MB), so DeepFilterNet3 on the Frame's Adreno GPU through Vulkan is possible in principle; it is not tested. CPU tuning for the Frame (`cortex-a720`, Armv9.2-A) against `generic` can only be measured on aarch64: Task 2 adds it to the on-demand arm job (Neoverse N2, Armv9.0, also a stand-in).

## Questions for the human
Each question has a recommendation, marked (R).

**Q1. Targets for the first release. Answered (human, 2026-10-14): (a).**
- (a) (R) Linux x86_64, Linux aarch64 (the Frame, also any aarch64 Linux), Windows x64, macOS arm64. These four are the ones CI already tests.
- (b) Also Windows arm64 (`windows-11-arm`) and macOS x86_64 (`macos-26-intel`). libwebrtc is pinned for both, but neither is tested in CI today; adding them means two more test jobs first. Recommended later, once someone can try them.

**Q2. CPU levels (from the measurements). Answered (human, 2026-10-14): (a).**
- (a) (R) One binary per OS and architecture. x86_64 Linux stays x86-64-v3 (your Phase 6 decision); x86_64 Windows also moves to v3, with `require_cpu_level` extended to Windows (a CRT initializer, the same CPUID check), so the 4 % of PCs without AVX2 get a message instead of a crash. macOS stays on Apple Silicon only (Q1). aarch64 stays `generic`, unless the arm job (Task 2) shows Frame tuning is worth a separate Frame build.
- (b) The same, but Windows stays at the baseline: no new startup check, about 11 µs per frame more on Windows.
- (c) Two x86_64 binaries (baseline and v3, or v3 and v4) chosen by a launcher. Not recommended: twice the CI and packaging for about 0.1 % of a core; revisit only if the AMD v4 numbers show a large gain.

**Q3. GPU support in packages (from the measurements). Answered (human, 2026-10-14): (b), bundle the "any GPU" pack.** The NVIDIA pack stays a later in-app download (its own plan after Phase 7).
- (a) (R) Every package is the full build (`--features gpu`, +0.7 MB). No GPU runtime is bundled. A later task (after Phase 7, its own plan) adds in-app downloads of the pinned packs (the fetch scripts' SHA-256 lists, done in Rust): "any GPU" (ONNX Runtime CPU + WebGPU, 14–92 MB) and "NVIDIA" (ONNX Runtime with CUDA, plus CUDA and cuDNN: 1.3–1.4 GB; TensorRT optional, 3.7 GB more; NVIDIA's licences shown first). Until then the Voice & audio card says the GPU needs a download and points to `docs/install.md`.
- (b) The same, but the "any GPU" pack (16 MB on Linux, 20 MB on the Frame) is bundled in the Linux packages and the Flatpak, so AMD, Intel and the Frame get GPU cleaning on first start; NVIDIA's pack stays a download. Bigger Linux packages (+16 MB, about +40 %), a second set of licences in them, and WebGPU's CPU cost on AMD is not measured yet.
- (c) Two packages per OS, light and full (with the "any GPU" pack). Not recommended: the binary difference is 0.3 %.

**Q4. Release profile (from the measurements). Answered (human, 2026-10-14): (a).**
- (a) (R) A `[profile.dist]` inheriting `release` with `lto = "fat"`, `codegen-units = 1`, used only by release CI (16 min per target). Symbols are kept in the binary (for Task 10's backtraces) and stripped only from the AppImage and `.deb`, whose debug info goes to a separate `twicecat-<version>-debug.tar.zst` on the release.
- (b) `lto = "thin"`, `codegen-units = 16`: a middle ground, not measured; Task 2's script would measure it first.
- (c) No new profile: the default `release` (4½ min, 43 MB download).

**Q5. Formats and tools. Answered (human, 2026-10-14): (a), with an MSI on Windows.**
- (a) (R) cargo-packager 0.11.8 for MSI (WiX), `.app` and DMG, AppImage and `.deb`, from one configuration; flatpak-builder for the Flatpak; a `.tar.zst` with the binary for everything else (AUR, the Frame without Flatpak). It is "public preview" and its last release is 2025-11-27, so the plan keeps it behind one script (`scripts/package.sh`) that could call cargo-wix, cargo-bundle and cargo-deb instead.
- (b) The single-format tools: cargo-wix 0.3.9, cargo-bundle 0.12.0, cargo-deb 3.8.0, and appimagetool. More tools, each better maintained.
- Windows installer: MSI (the roadmap's choice; per-machine, needs admin, managed deployment works) (R), or NSIS `.exe` (per-user, no admin, the usual choice for chat apps). Either can be added later beside the other.

**Q6. Signing. Answered (human, 2026-10-14) for Windows: not yet; the human will look into it.** Windows builds ship unsigned for now (c), and the workflow keeps a signing step that runs only once secrets exist. macOS and Linux as recommended unless the human says otherwise. The secrets are always yours (rule 8); the workflow signs only when they exist.
- macOS: Apple Developer ID Application certificate plus notarization (an App Store Connect API key for `notarytool`). (R) as the roadmap says.
- Windows: (a) (R) Azure Trusted Signing: a monthly fee and no hardware token, signs from CI with a GitHub OIDC login; (b) an OV or EV certificate on a cloud HSM (DigiCert KeyLocker, SSL.com eSigner); (c) unsigned at first, with the SmartScreen warning documented.
- Linux: no package signing (Flatpak is signed by its repository); `SHA256SUMS` and GitHub build provenance attestations for every artifact. (R)

**Q7. Updates. Answered (human, 2026-10-14): (a) on Windows and macOS; on Linux the package manager (Flatpak, `.deb`) and, for the AppImage, (b) cargo-packager-updater.** The AppImage updater needs a signing key pair that the human creates and keeps (rule 8): the public key is built into the AppImage, the private key is a CI secret; without it the AppImage ships with its updater off.
- (a) (R) A version check, opt-in (asked once at first start, changeable in Settings): once a day the core asks GitHub's latest-release API, and the app shows "Twicecat X.Y is available" with a link. Nothing is downloaded or run. Off in Flatpak and `.deb` builds. No new crate: the core already has an HTTP client through matrix-sdk (Q11).
- (b) cargo-packager-updater 0.2.3: downloads and installs signed updates (its own minisign key, which you would hold). More convenient, more to secure.
- (c) None in Phase 7.

**Q8. Crash reports. Answered (human, 2026-10-14): (a).**
- (a) (R) Local only: a scrubbed report file written on a panic, offered at the next start with "Open an issue" (the user sees and sends it themselves). No server, no account, no new crate.
- (b) Sentry (sentry 0.49.3), opt-in, to a Sentry project you own (a DSN in the build). Automatic, but a third-party service and a new dependency tree.
- (c) None in Phase 7.

**Q9. Flatpak and Flathub. Answered (human, 2026-10-14): ship our own Flatpak (a).** Whether and when to ask Flathub for an exception is left to the human. Flathub requires everything built from source, with no prebuilt binaries in the submission, and no network during the build (docs.flathub.org, "Requirements"). We link LiveKit's prebuilt libwebrtc, and building libwebrtc from source needs Chromium's toolchain (gn, ninja, its own clang and sysroot).
- (a) (R) Phase 7 ships our own Flatpak: the manifest takes the pinned libwebrtc archive as a source with its SHA-256, CI builds a `.flatpak` bundle for x86_64 and aarch64 and attaches it to the release. In parallel you ask Flathub's reviewers (an issue on flathub/flathub) whether the pinned LiveKit archive can be an exception; the submission itself waits for their answer.
- (b) Build libwebrtc from source inside the Flatpak (LiveKit's build scripts, offline). Weeks of work and hours per build; a later phase if Flathub refuses (a).
- Portals: Flathub asks for portals where they exist. Notifications (`notify-rust` talks to `org.freedesktop.Notifications` directly) and the keychain (`keyring`'s Secret Service store talks to `org.freedesktop.secrets` directly) both have portals. (R) keep the direct D-Bus names in our own Flatpak for Phase 7, and move to the portals only if Flathub asks (a change in the core and the app, and maybe a new crate).

**Q10. What every package bundles. Answered (human, 2026-10-14): (a).**
- (a) (R) The DeepFilterNet3 models (8.3 MB, MIT or Apache-2.0), so DeepFilterNet works on first start everywhere, the Frame included; the licence notices; the icon and Linux desktop files.
- (b) Models fetched on first use instead (smaller packages, but a download and a SHA-256 check in the app).

**Q11. New tools and crates (rule 3). Answered (human, 2026-10-14): approved, plus cargo-packager-updater =0.2.3 for the AppImage (Q7).** Build tools only, pinned where CI installs them: cargo-packager 0.11.8 (or Q5 b's tools), cargo-about 0.9.2, flatpak-builder from the runner's distribution, `flatpak-cargo-generator.py` at a pinned commit, `actionlint` (CI only). No new crate in the recommended answers. Q7 (a) makes `reqwest` a direct dependency of the core, at the version matrix-sdk already locks (no new package in `Cargo.lock`). Approve the list, or say which to drop.

**Q12. Plan, branch, steps and scope. Answered (human, 2026-10-14): as recommended, with the new `packaging` scope (added to AGENTS.md rule 5 in the plan's commit).** (R) This plan, ledger `docs/plans/phase7-packaging.progress.md`, branch `feat/phase7-packaging`, steps `phase7.1` to `phase7.12`. Packaging-only commits use a new scope `packaging` (AGENTS.md lists the scopes; `scripts/check-commits.sh` does not restrict them), or `dev` and `ci` as today if you prefer.


**Q13. apt, dnf and pacman (asked by the human 2026-10-14). Answered (human, 2026-10-14):** the agent picks the RPM tool and the hosting; older distributions are not served for now (they can build from source); an AUR `twicecat-bin` package. Not the distributions' own repositories: they build everything from source, package every crate separately, and would refuse the prebuilt libwebrtc, as Flathub does.
- **RPM tool: nfpm** (v2.48.0 when chosen; pinned at v2.47.0, 2026-06-20, in Task 6) (goreleaser, MIT; one static binary, pinned with its published SHA-256). One YAML builds the `.deb` and the `.rpm` with the same contents and per-format dependency names, and signs RPMs itself. It takes the `.deb` over from cargo-packager, so both formats stay identical. (cargo-generate-rpm 0.21.0 makes only RPMs and would need a second description of the same package.)
- **Hosting: GitHub Pages of this repository**, built by the release workflow from the last three releases. No third-party account; the cost is one OpenPGP key for signing, which the human creates. COPR and the Open Build Service were the alternatives: both need an account, and both rebuild the package from a spec on their own machines.
- **Tools added to Q11:** nfpm (v2.48.0 chosen here, pinned at v2.47.0 when implemented), `apt-ftparchive` (`apt-utils`), `createrepo_c` and `gpg` from the Ubuntu runner; for the AppImage, appimagetool 1.9.1 and the type2 runtime 20251108 (Task 6).

## Release matrix (as recommended)
| Target | Runner | CPU | Packages | Signed by |
| --- | --- | --- | --- | --- |
| Linux x86_64 | `ubuntu-26.04` | x86-64-v3 | Flatpak bundle, AppImage, `.deb`, `.rpm`, `.tar.zst`; apt, dnf and AUR (Q13) | `SHA256SUMS`, attestations; the AppImage's updater signature (Q7) |
| Linux aarch64 (Steam Frame) | `ubuntu-26.04-arm` | generic (Q2) | Flatpak bundle, AppImage, `.deb`, `.rpm`, `.tar.zst`; apt, dnf and AUR (Q13) | `SHA256SUMS`, attestations |
| Windows x64 | `windows-latest` | x86-64-v3 (Q2), with the startup check | MSI | unsigned for now (Q6) |
| macOS arm64 | `macos-latest` | Apple M1 baseline | DMG with the `.app` | Developer ID, notarized |

Every package: the full build (Q3), the `dist` profile (Q4); the Linux ones and the Flatpak also carry the "any GPU" pack (Q3 b); the models (Q10), `LICENSE`, `THIRD_PARTY_NOTICES.html`, the icon. The glibc floor of the Linux builds is Ubuntu 26.04's; Task 6 records it and decides whether an older build image is needed for the AppImage and `.tar.zst` (the Flatpak does not care).

## Tasks (one commit each)
Step numbers follow Q12 (13 tasks since Q13 added Task 12). Each task lists its files and its acceptance; "smoke test" means the packaged binary prints `twicecat <version>` for `--version` and exits 0, headless.

### Task 1: `--version`, the Windows window, the CPU check, and a version check in CI (phase7.1, `feat(app)`)
Files: `crates/app/src/main.rs`, `crates/core/src/bin/twicecat-cli.rs`, `Cargo.toml` (workspace `version`, inherited by the crates), `scripts/check-version.sh` (new), PROGRESS.md, ledger.
- `twicecat --version` and `twicecat-cli --version` print `twicecat <version>` (from `CARGO_PKG_VERSION`) and exit 0, before logging. (Done differently from the draft: the CPU check runs before `main`, so `--version` cannot come first; instead the refusal message itself names the version.)
- `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`; with no console, `--help` and `--version` attach to the parent console (`AttachConsole(ATTACH_PARENT_PROCESS)`, `windows` 0.62.2, already in the tree) so they still print from a terminal.
- Per Q2 (a): `require_cpu_level` also on Windows x86_64 (a `.CRT$XCU` initializer, the same CPUID and XGETBV checks, a message box when there is no console), and the check covers the level the build targets: v4's AVX-512 bits and XCR0 state in a build for v4 (done instead of the draft's compile error, which would have blocked the v4 measurement on the AMD machine).
- `scripts/check-version.sh vX.Y.Z` fails unless the tag equals the workspace version (used by the release workflow).
- Tests: argument parsing (`--version` alone; with other arguments it is refused); the check script against a matching and a mismatching tag.

### Task 2: The release profile and the measurement script (phase7.2, `feat(dev)`)
Files: `Cargo.toml` (`[profile.dist]` per Q4), `.cargo/config.toml` (CPU levels per Q2), `scripts/measure-builds.sh` (new: the measurement above, reproducible: variants, sizes, the chain on a P-core and an E-core, a Markdown table on stdout and in `$GITHUB_STEP_SUMMARY`), `.github/workflows/profile.yml` (an input to run the measurement, and on the arm runner a `generic` against Frame-tuned comparison, Q2), `docs/building.md` ("Release builds"), PROGRESS.md, ledger.
- Acceptance: the script reproduces the table in this plan within noise on this machine; the arm job's YAML passes the workflow check.

### Task 3: Bundled resources (phase7.3, `feat(audio)`)
Files: `crates/audio/src/paths.rs`, `crates/audio/src/dfn/mod.rs`, `crates/audio/src/onnx/mod.rs`, `crates/audio/src/onnx/webgpu.rs`, PROGRESS.md, ledger.
- Models are looked up in order: `TWICECAT_MODEL_DIR`; the package's resource directory (beside the executable under `../share/twicecat/models` on Linux, which covers `/usr`, `/app` in Flatpak and `$APPDIR/usr` in an AppImage; `Contents/Resources/models` in the macOS bundle; `models\` beside the `.exe` on Windows); then the user cache. The cache on Windows is `%LOCALAPPDATA%\twicecat` (today it needs `HOME`). (Changed 2026-10-17: on Linux the directory is `../lib/twicecat`, because with the `.tar.zst` unpacked into `~/.local`, `../share/twicecat` is the app's data directory `~/.local/share/twicecat`, found while installing on the Steam Frame. Done differently: the user cache comes first and the package second, as the next point needs, and both use the cache's layout; on Windows without `HOME` the cache is `%USERPROFILE%\.cache\twicecat`, the folder the scripts already use from Git Bash.)
- Per Q3 (b), the same resource directory also holds the "any GPU" pack: ONNX Runtime's CPU build (`onnxruntime/lib`), the WebGPU plugin (`onnxruntime/webgpu-ep`) and the stateful DeepFilterNet3 graph (`models/deepfilternet3-stateful`). `default_runtime_dir` (`onnx/mod.rs:167`), `default_webgpu_dir` (`onnx/webgpu.rs:47`) and `default_stateful_model` (`dfn/mod.rs:52`) look there after their variable and after the user cache, so a CUDA build the user fetched still wins over the bundled CPU build.
- Tests: table tests of the lookup order with a fake executable path and environment (variable, then cache flavours, then bundled); a missing directory falls through.

### Task 4: Icon, AppStream metainfo, desktop entry (phase7.4, `feat(app)`)
Files: `packaging/icons/twicecat.svg` (new, the tray's rounded "T" as a vector), generated PNGs (16–512), `.ico`, `.icns` (by a script, so they are reproducible: `scripts/make-icons.sh`), `packaging/linux/io.github.ga_lep.Twicecat.metainfo.xml` (new), the desktop entry gains `Icon=io.github.ga_lep.Twicecat`, the window icon (`iced::window::Settings::icon`, read in iced 0.14's source first), PROGRESS.md, ledger.
- Acceptance: `desktop-file-validate` and `appstreamcli validate --strict` pass; a test that the tray icon and the SVG agree on colours (the tray keeps drawing in code).

### Task 5: Licence notices (phase7.5, `feat(packaging)`)
Files: `about.toml`, `packaging/about.hbs` (new), `scripts/third-party-notices.sh` (new), PROGRESS.md, ledger.
- One `THIRD_PARTY_NOTICES.html` per build target from `cargo about generate` (the target's dependency tree), plus libwebrtc's `LICENSE.md` from the pinned archive, DeepFilterNet3's licence, and on Linux (Q3 b) ONNX Runtime's `LICENSE` and `ThirdPartyNotices.txt` and the WebGPU plugin's licence files (both MIT, from the pinned archives). Every package ships it next to `LICENSE` (GPL-3.0-or-later) and a link to the tagged source.
- Acceptance: the script runs offline after `cargo fetch`; it fails on a licence `deny.toml` would refuse.

### Task 6: Desktop packages: cargo-packager and nfpm (phase7.6, `feat(packaging)`)
Files: `crates/app/Cargo.toml` (`[package.metadata.packager]`) or `packaging/Packager.toml`, `packaging/nfpm.yaml` (new, Q13), `packaging/macos/Info.plist` additions and `entitlements.plist` (`com.apple.security.device.audio-input`, hardened runtime), `packaging/windows/` (WiX template only if the default needs changes), `scripts/package.sh` (new: builds with the dist profile, then packages for the host), PROGRESS.md, ledger.
- Formats per Q5 and Q13: cargo-packager makes the Windows MSI and the macOS `.app` in a DMG; nfpm makes the `.deb` and the `.rpm` from one file list; appimagetool makes the AppImage; plus the `.tar.zst` with the binary, resources and notices. (Changed while implementing: cargo-packager 0.11.8's AppImage downloads linuxdeploy's "continuous" build and an AppRun at packaging time with no checksum (`src/package/appimage/mod.rs:24-57`), unlike its WiX download, which is pinned and verified (`wix/mod.rs:28-30`). So the AppImage is built from our own AppDir with appimagetool 1.9.1 and the type2 runtime 20251108, both pinned by the publishers' SHA-256 in `scripts/checksums/packaging-tools.sha256`; nfpm is pinned at 2.47.0, not the two-day-old 2.48.0.) Each bundles the models (Task 3), the icon (Task 4) and the notices (Task 5); the Linux packages also bundle the "any GPU" pack (Q3 b): ONNX Runtime 1.28.3 CPU, the WebGPU plugin 0.4.0 and the stateful graph, all from the pinned archives with their SHA-256 checked (about +16 MB on x86_64, +14 MB on aarch64, before compression).
- macOS: `NSMicrophoneUsageDescription` ("Twicecat uses the microphone for calls."), `LSMinimumSystemVersion` from the oldest runner we test, `CFBundleIdentifier io.github.ga_lep.Twicecat`.
- `.deb` and `.rpm` (nfpm `overrides` per format): the libraries the binary links (`libpipewire-0.3`, `libasound`, `libudev`, from `ldd`, checked by the script so a new link fails the build) under each family's names (Debian `libpipewire-0.3-0t64`/`libasound2t64`/`libudev1`, Fedora `pipewire-libs`/`alsa-lib`/`systemd-libs`; exact names checked against Ubuntu 26.04 and Fedora 44), and as recommends the ones loaded at run time (`libwayland-client0`, `libxkbcommon0`, `libvulkan1` and their Fedora names); both install the binary, the resources (Task 3), the desktop entry, the metainfo and the icons. Built on Ubuntu 26.04, so they need its glibc (2.43) or newer: Ubuntu 26.04 and Fedora 44 or newer, not Fedora 43 (2.42), measured in the containers (Q13: older distributions build from source; `docs/install.md` says so).
- Acceptance on this machine: the AppImage, the `.deb` and the `.rpm` build; the AppImage's smoke test passes with `--appimage-extract-and-run`; the `.deb` installs with `apt` and passes the smoke test in an `ubuntu:26.04` container, the `.rpm` with `dnf` in a `fedora:44` container (dependencies resolved from the distribution, so a wrong name fails here). MSI and DMG are built in CI (Task 8).

### Task 7: Flatpak (phase7.7, `feat(packaging)`)
Files: `packaging/flatpak/io.github.ga_lep.Twicecat.yml` (new), `packaging/flatpak/cargo-sources.json` (generated by `flatpak-cargo-generator.py` from `Cargo.lock`, checked in CI to match the lock), `scripts/flatpak-sources.sh`, PROGRESS.md, ledger.
- Runtime `org.freedesktop.Platform` 25.08 with the `rust-stable` and LLVM (clang 21 or newer) SDK extensions, both checked to exist on that branch before use; libwebrtc per Q8; x86_64 and aarch64 (`only-arches` on the libwebrtc source).
- `finish-args`, each with a comment saying why: `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc`, `--device=dri` (wgpu, later VA-API), `--share=network`, `--filesystem=xdg-run/pipewire-0` (our PipeWire client), `--socket=pulseaudio` only if PipeWire alone is refused, `--talk-name=org.freedesktop.secrets` (keyring) unless Q9 moves to the Secret portal, `--talk-name=org.freedesktop.Notifications` unless Q9 moves to the portal, `--talk-name=org.kde.StatusNotifierWatcher` (tray), `--device=input` (gamepads; needs Flatpak 1.15.6). The GlobalShortcuts and file-chooser portals need nothing.
- The "any GPU" pack (Q3 b) as `archive` and `file` sources with their pinned SHA-256 (`only-arches` per architecture), installed under `/app/lib/twicecat` (`/app/share/twicecat` until 2026-10-17, see Task 3).
- Acceptance: the manifest builds in CI on x86_64 and aarch64; `flatpak run --command=twicecat io.github.ga_lep.Twicecat --version` passes headless; a `.flatpak` bundle is produced for the release.
- (Changed while implementing, 2026-10-16.) Runtime `org.freedesktop.Platform` **26.08**, not 25.08: Flathub's extensions have a 26.08 branch, and the LLVM one is `llvm22` (the clang CI uses), which has 25.08 and 26.08 branches; `llvm21` has only 25.08. Rust is **1.97.1 from the standalone installer** (`scripts/checksums/rust.sha256`), not the `rust-stable` extension, which follows the latest release while `rust-toolchain.toml` pins 1.97.1. The build calls the same scripts as the other packages: the fetch scripts take their pinned archives from `TWICECAT_DOWNLOADS` (flatpak-builder `file` sources with the same SHA-256s; `scripts/flatpak.sh check` compares them with `scripts/checksums/`), and `scripts/package.sh --formats prefix` installs into `/app`. The notices page and the stateful graph are made outside the sandbox (cargo-about and PyTorch) and given as local files. flatpak-cargo-generator.py is pinned at flatpak-builder-tools `41c20aa1` (2026-09-21, SHA-256 `0a2db6be…b380`), run with tomlkit 0.15.1 and aiohttp 3.14.3. The CI checks job runs `scripts/flatpak.sh check`; the CI build itself is in the release workflow (Task 8).

### Task 8: Release workflow (phase7.8, `feat(ci)`)
Files: `.github/workflows/release.yml` (new), PROGRESS.md, ledger.
- One job exports the stateful DeepFilterNet3 graph (`scripts/export-deepfilternet.sh`, CPU PyTorch in a venv; the result must match the pinned SHA-256 in `scripts/checksums/models.sha256` or the release stops) and hands it to the Linux packaging jobs as an artifact. It is the same file on every architecture.
- On a `v*` tag (and `workflow_dispatch` for a dry run that uploads artifacts but makes no release): `check-version.sh`, then a matrix per Q1 and Q2 that builds with the dist profile, runs Tasks 5–7's packaging, smoke-tests every Linux package (AppImage, `.deb` in a container, Flatpak), and `--version` on the Windows and macOS binaries.
- Signing (Q6) only when its secrets exist: macOS `codesign --options runtime` with the entitlements, `notarytool submit --wait`, `stapler staple`; Windows the chosen signer on the `.exe` and the MSI. Without secrets the job writes "unsigned" in the summary and the release notes.
- `SHA256SUMS` over all artifacts, GitHub build provenance attestations (`actions/attest-build-provenance`), and a **draft** release with the artifacts and generated notes. The human publishes it.
- Least privilege: `contents: read` everywhere except the job that creates the draft (`contents: write`, `id-token: write`, `attestations: write`); actions pinned by full commit SHA; `persist-credentials: false`.
- Acceptance: workflow check passes; a `workflow_dispatch` dry run on the human's push produces every artifact (human runs it, rule: pushing is the human's).

### Task 9: Updates (phase7.9, `feat(core)` + `feat(app)` per Q7)
- Windows and macOS, Q7 (a): `Command::CheckForUpdates` and `CoreEvent::UpdateAvailable { version, url }` / `UpToDate`. The core asks GitHub's "latest release" API over its own HTTP client, at most once a day and only when the user turned it on (first-run question, Settings → About). It compares versions; it never downloads or runs anything. The app shows a banner with a link to the release page. Off in Flatpak, `.deb`, `.rpm` and AUR builds (a build-time `TWICECAT_DISTRIBUTION` value), where the package manager updates.
- The AppImage, Q7 (b): cargo-packager-updater =0.2.3, its API read in its source first (rule 2) and its dependency tree listed (packages added to `Cargo.lock`, `cargo deny check`) before it is added. It lives in the core behind the same commands (`CheckForUpdates`, plus `InstallUpdate`), so the app keeps no HTTP or update dependency (rule 7); it is compiled in only for Linux and active only when `APPIMAGE` is set and the build has the public key. Opt-in like the check; the user confirms each install, and the app restarts into the new AppImage. The release workflow signs the AppImage with the private key from a secret (`cargo packager signer`) and publishes the updater's manifest beside it; without the secret there is no manifest and the updater stays off.
- Tests: version comparison, the once-a-day throttle (paused time), which mechanism each distribution gets (Windows, macOS, AppImage, Flatpak, `.deb`, `.rpm`, AUR, `.tar.zst`), off when not opted in, and the banner. No network in unit tests (a fake server on localhost in an ignored integration test).

### Task 10: Crash reports (phase7.10, `feat(app)` per Q8)
- Per Q8 (a): a panic hook writes `crash-<time>.txt` in the data directory: version, OS, CPU level, GPU backend in use, the panicking thread and a backtrace with our crate paths only. No message text, room or user IDs, server names, file paths under the home directory, or environment. The next start shows "Twicecat closed unexpectedly" with **Show report**, **Open an issue** (a GitHub new-issue link with the report's text, which the user reviews in the browser before sending), and **Delete**.
- Tests: the report's content from a panic with sensitive-looking strings in its message and in a path (they are removed); the scrubber's table tests; the banner.

### Task 11: The sandbox and the Frame (phase7.11, `feat(core)`/`feat(app)` as needed)
- Code changes found while running the Flatpak locally (if `flatpak` is installed by the human or allowed in a container), for example a clear error when no Secret Service answers (Frame game mode) instead of a failed login, and Q9's portal moves.
- `docs/install.md` (new): every package, what each needs, the GPU packs (Q3), and the Frame: install from the Flatpak in desktop mode, then "Add a non-Steam game" for game mode.

### Task 12: apt and dnf repositories, AUR (phase7.12, `feat(packaging)`, Q13)
Files: `.github/workflows/release.yml` (a `repositories` job), `scripts/build-repos.sh` (new), `packaging/aur/twicecat-bin/PKGBUILD` and `.SRCINFO` template, `scripts/aur-pkgbuild.sh` (new), `docs/install.md` (the one-time setup for each), PROGRESS.md, ledger.
- **Where:** GitHub Pages of this repository (`https://ga-lep.github.io/twicecat/`), deployed with `actions/deploy-pages`. The site is rebuilt on every published release from the `.deb` and `.rpm` assets of the **last three** releases (downloaded from GitHub Releases, checked against their `SHA256SUMS`), so no branch grows and the site stays far below Pages' 1 GB. Runs on `release: published` (the human publishes the draft) and on `workflow_dispatch`.
- **apt** (`/apt`): a flat repository per architecture built with `apt-ftparchive` (Ubuntu's `apt-utils`), `Release` signed into `InRelease` and `Release.gpg` with the repository key. Users: the key into `/etc/apt/keyrings/twicecat.asc` and a deb822 `/etc/apt/sources.list.d/twicecat.sources` with `Signed-By`.
- **dnf** (`/rpm`): each `.rpm` signed by nfpm with the same key, `createrepo_c` metadata, `repomd.xml` detach-signed; users add `/etc/yum.repos.d/twicecat.repo` with `gpgcheck=1` and `repo_gpgcheck=1`.
- **The key:** one OpenPGP key that the human creates and keeps (rule 8); its private part and passphrase are CI secrets, used only in this job and the `.rpm` signing step; the public key is committed (`packaging/repo-key.asc`) and published on the site. Without the secrets the job stops with a message and nothing is deployed.
- **AUR `twicecat-bin`:** a `PKGBUILD` that installs the release's `.tar.zst` (x86_64 and aarch64 sources, `sha256sums` from `SHA256SUMS`), with the same dependencies under Arch's names (`libpipewire`, `alsa-lib`, `systemd-libs`). `scripts/aur-pkgbuild.sh vX.Y.Z` fills it for a release and writes `.SRCINFO`; the release workflow attaches both. Pushing to the AUR needs an AUR account and its SSH key, which are the human's: the human pushes, or adds the key as a secret for a later CI job.
- **Acceptance:** with a throwaway key made in the test, `scripts/build-repos.sh` builds both repositories from local packages; in containers, `apt install twicecat` (`ubuntu:26.04`) and `dnf install twicecat` (`fedora:44`) from a local web server serving them pass the signature checks and the smoke test, and an unsigned or tampered `Release` is refused; `makepkg --printsrcinfo` matches the generated `.SRCINFO` and `namcap` is clean (in an `archlinux` container).

### Task 13: Wrap-up (phase7.13, `docs(plan)`)
- An independent read-only review of `main..HEAD`; fixes folded into their commits.
- PROGRESS.md: a "Phase 7" section with steps, notes, open issues and the checkpoint below.

## After Phase 9 (video)
When `feat/video` merges, packaging gains, without changing the formats. **Phase 9 Tasks 1-11 reached `main`, and this branch was rebased on it on 2026-10-16**; what that settled:
- macOS `Info.plist`: `NSCameraUsageDescription` (done in Task 6; screen recording needs no key, macOS asks itself); entitlements `com.apple.security.device.camera` (done in Task 6). macOS capture itself is Phase 9 Task 12, not on `main` yet.
- Flatpak: the camera and ScreenCast portals (no extra permission), and NVENC through the GL extension (`org.freedesktop.Platform.GL.*`, which Flatpak installs to match the host driver) and `--device=dri` (Task 7). No VA-API: Phase 9 settled on NVENC and LiveKit's software encoders (its Q6).
- Linux packages: no new dependency (corrected: `libva` is not used, see above; NVENC loads the driver's libraries at run time).
- The Inter typeface (UI rework, also on `main`): every package ships its `OFL.txt` and `THIRD-PARTY-NOTICES.md`, and the notices page carries the OFL (Tasks 5 and 6).
- The measurement script gains the video path (`scripts/measure-encoders.sh` already exists there): left for Phase 9 Task 14.

## Human checkpoint (collected in PROGRESS.md, never ticked by the agent; rule 9)
- [x] Plan questions Q1–Q12 answered and the plan approved (2026-10-14).
- [ ] Add the signing secrets yourself (never handed to the agent): Apple Developer ID certificate and notarization key; the Windows signer once chosen (Q6: not yet); the AppImage updater's key pair (`cargo packager signer generate`; the public key goes in the repository, the private key and its password in CI secrets).
- [ ] With the secrets added, run the release workflow's dry run and check `codesign --verify --deep --strict`, `spctl -a -vv`, `stapler validate`, and the Windows signature once there is one.
- [ ] The AppImage updates itself from one release to the next (after opting in), and the Windows and macOS banner links to the new release.
- [ ] Create the repository signing key (Q13; OpenPGP, the private part and passphrase as CI secrets), enable GitHub Pages with "GitHub Actions" as the source, publish a release, then on a clean Ubuntu 26.04 and Fedora 44 follow `docs/install.md`: install, then upgrade to the next release with `apt upgrade` / `dnf upgrade`.
- [ ] Create the AUR account and push `twicecat-bin` (the agent never holds the AUR key); `yay -S twicecat-bin` on Arch or SteamOS desktop mode.
- [ ] Push the branch and run the release workflow's dry run (`workflow_dispatch`): every artifact is produced, the Linux smoke tests pass, `SHA256SUMS` and the attestations are attached.
- [ ] Install each artifact on a clean machine and start it: Windows 11 (SmartScreen with and without signing), macOS on Apple Silicon (Gatekeeper; the microphone prompt; push-to-talk without Input Monitoring), Ubuntu or Fedora from the `.deb` and the AppImage, the Flatpak on GNOME and on KDE.
- [ ] Flatpak sandbox on a desktop: login survives a restart (Secret Service), a notification shows, the microphone works in a call (PipeWire), the push-to-talk portal dialog appears and the key works, the tray icon shows with its badge, a gamepad button works, the file picker opens.
- [ ] Steam Frame: the aarch64 Flatpak installs in desktop mode; Twicecat starts, logs in and keeps the login (Secret Service in desktop mode, and in game mode after "Add a non-Steam game"), a call works on the built-in mic and speakers; the CPU numbers from Task 2's aarch64 job against the Frame itself.
- [ ] The bundled "any GPU" pack (Q3 b) from the packages, not the scripts: DeepFilterNet3 on the GPU through WebGPU on an AMD GPU, an Intel GPU and the Frame's Adreno (Vulkan), by ear and with the CPU per frame the card reports; fetching CUDA afterwards still makes NVIDIA use it.
- [ ] AMD machine (Galep's Ryzen 7 7800X3D and Radeon RX 6950 XT): `scripts/measure-builds.sh` for the x86-64-v3 against v4 rows (this Intel machine has no AVX-512), and the WebGPU path in the packaged full build.
- [ ] The icon (Task 4) as drawn, and the AppStream text (name, summary, description, screenshots you choose).
- [ ] The update banner (Task 9) and the crash report screen (Task 10) in both themes.
- [ ] Flathub: ask the reviewers about the prebuilt libwebrtc (Q9), then submit; the project website and the Matrix room for users; list the client on matrix.org once stable.
