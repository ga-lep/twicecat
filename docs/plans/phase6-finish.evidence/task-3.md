# Evidence — Task 3: on-demand aarch64 call-path profiling job
Commit: "feat(ci): add an on-demand aarch64 call-path profiling job" (subject; SHAs were rewritten when the history was tidied)
Environment: x86_64 (Ryzen 7 7800X3D), CC=clang-22 CXX=clang++-22, actionlint-py via `pipx run` into a scratch PIPX_HOME (repo untouched), PyYAML under `python3 -I`.

## .github/workflows/profile.yml is lint-clean
Status: PROVEN
```console
$ pipx run --spec actionlint-py actionlint .github/workflows/profile.yml
exit=0
```
actionlint (no output) exits 0.

## Structure: only workflow_dispatch, contents: read, persist-credentials false, only checkout@v6 and rust-cache@v2, ubuntu-26.04-arm, step order
Status: PROVEN
```console
$ python3 -I (yaml.safe_load) ; grep -n 'uses:' .github/workflows/profile.yml
{'workflow_dispatch': None} {'contents': 'read'}
actions/checkout@v6 {'persist-credentials': False}
Install Linux build dependencies None
Use clang 22 None
Fetch pinned libwebrtc None
Swatinem/rust-cache@v2 None
Fetch models None
Profile the call path None
No unverified libwebrtc download None
ubuntu-26.04-arm
20:      - uses: actions/checkout@v6
36:      - uses: Swatinem/rust-cache@v2
```
Only dispatch trigger; read-only permissions; only the two allowed actions; order is fetch libwebrtc, rust-cache, fetch models, profile, check-no-webrtc-download last. The profile step's output goes to the summary via $GITHUB_STEP_SUMMARY (see next).

## Local dry run of the script step, report to job summary
Status: PROVEN (x86_64 only)
```console
$ GITHUB_STEP_SUMMARY=<tmp>/summary.md scripts/profile-call-path.sh --frames 100
exit=0
building the profile examples (release, --locked)...
Call path per 10 ms frame, 4 remote voices, 100 frames, 64 kb/s Opus complexity 9
Budget: 5 % of a core = 500 us per 10 ms
Host: x86_64, AMD Ryzen 7 7800X3D 8-Core Processor
mode                   scenario  median us  budget    p99 us  budget
off                    talking        68.7   13.7%      96.8   19.4%
off                    silent         31.9    6.4%      50.6   10.1%
WebRTC NS              talking        84.6   16.9%     121.5   24.3%
WebRTC NS              silent         47.8    9.6%      75.3   15.1%
RNNoise                talking        98.0   19.6%     127.4   25.5%
RNNoise                silent         61.2   12.2%      81.2   16.2%
DeepFilterNet3 CPU     talking       338.4   67.7%     513.5  102.7%  OVER
DeepFilterNet3 CPU     silent        301.6   60.3%     467.3   93.5%
(parts and chain rows trimmed)
$ wc -l summary.md; head -2 summary.md
33 summary.md
### Call path against the Frame budget
```
The report was appended to the summary file.

## check-no-webrtc-download.sh afterwards, against the resulting target dir
Status: PROVEN
```console
$ scripts/check-no-webrtc-download.sh; echo "exit=$?"
exit=0
$ mkdir -p <tmp>/bad/release/build/scratch-x/out/livekit_webrtc; CARGO_TARGET_DIR=<tmp>/bad scripts/check-no-webrtc-download.sh; echo "exit=$?"
error: webrtc-sys downloaded libwebrtc unverified:
<tmp>/bad/release/build/scratch-x/out/livekit_webrtc
exit=1
```
Passes on the real build output and fails on a planted download marker.

## Job summary shows the call-path table for aarch64 after the human runs it
Status: NOT PROVEN (needs the human to push and run the workflow on GitHub's arm runner; the numbers above are x86_64 only)

## Verdict
EVIDENCE: PROVEN (aarch64 run NOT PROVEN, as expected)
