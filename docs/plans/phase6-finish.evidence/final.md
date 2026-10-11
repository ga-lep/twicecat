# Evidence — phase6-finish (whole feature)
Commit: "docs(plan): phase6-finish task 4 complete" (subject; SHAs were rewritten when the history was tidied)
Environment: Linux x86_64, AMD Ryzen 7 7800X3D; CC=clang-22 CXX=clang++-22; TWICECAT_MODEL_DIR=~/.cache/twicecat/models/deepfilternet3 (already fetched); LK_CUSTOM_WEBRTC=<scripts/fetch-libwebrtc.sh dir>; actionlint 1.7.12 via scratch pipx env (removed after). Output below is real; long listings trimmed where said.

## Branch history
Status: PROVEN
```console
$ git log --oneline main..HEAD   (17 commits at the time; the history was later tidied to 4 step commits and one docs commit; one feat per task, each with start/evidence/complete docs commits)
(sha) docs(plan): phase6-finish task 4 complete
(sha) feat(core): measure the silence options for the Opus encoder
(sha) feat(ci): add an on-demand aarch64 call-path profiling job
(sha) feat(scripts): measure the call path against the Frame budget in one command
(sha) feat(core): cover the switch from a failing GPU bank to the CPU
(sha) docs(plan): add the Phase 6 finish plan
(+ the start/evidence/complete docs(plan) commits between them)
$ scripts/check-commits.sh main
(no output)  exit=0
```

## Task 1: core's switch from a failing GPU bank to the CPU is covered
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib call::cleaning
running 13 tests
... policy::tests::* (8) ok
test call::cleaning::tests::the_cpu_filters_take_over_after_a_give_up ... ignored, needs scripts/fetch-models.sh
test call::cleaning::tests::a_voice_that_leaves_frees_its_row ... ok
test call::cleaning::tests::a_bank_too_slow_says_so ... ok
test call::cleaning::tests::a_bank_that_fails_hands_its_voices_to_the_cpu ... ok
test call::cleaning::tests::the_gpu_setting_turned_off_and_on_loads_a_new_bank_after_a_give_up ... ok
test result: ok. 12 passed; 0 failed; 1 ignored
$ cargo test -p twicecat-core --lib -- --ignored call::cleaning
test call::cleaning::tests::the_cpu_filters_take_over_after_a_give_up ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 478 filtered out
```
New tests pass, and the ignored CPU-takeover test passes with the real models.

## Task 2: one command measuring the call path against the budget
Status: PROVEN
```console
$ GITHUB_STEP_SUMMARY=<tmp>/summary.md scripts/profile-call-path.sh --voices 4 --frames 300     (exit=0)
Call path per 10 ms frame, 4 remote voices, 300 frames, 64 kb/s Opus complexity 9
Budget: 5 % of a core = 500 us per 10 ms
Host: x86_64, AMD Ryzen 7 7800X3D 8-Core Processor
Pinned to CPU: no
mode                   scenario  median us  budget    p99 us  budget
off                    talking        66.1   13.2%     140.3   28.1%
off                    silent         31.2    6.2%      97.8   19.6%
WebRTC NS              talking        79.8   16.0%     158.2   31.6%
WebRTC NS              silent         44.9    9.0%     115.7   23.1%
RNNoise                talking        94.5   18.9%     175.2   35.0%
RNNoise                silent         59.6   11.9%     132.7   26.5%
DeepFilterNet3 CPU     talking       317.4   63.5%     457.2   91.4%
DeepFilterNet3 CPU     silent        282.5   56.5%     414.7   82.9%
Parts (median / p99 us per 10 ms): voice engine mixing 4 voices 8.1 / 67.8; ...
  encode 64 kb/s c9 voice in noise unflushed           26.0 /     30.1
  decode 64 kb/s c9 voice in noise unflushed            7.4 /      9.9
  encode 64 kb/s c9 room noise unflushed               81.1 /     85.8
  decode 64 kb/s c9 room noise unflushed               12.4 /     15.6
  encode 64 kb/s c9 digital silence unflushed           8.7 /     10.8
  decode 64 kb/s c9 digital silence unflushed           3.0 /      4.1
  encode 64 kb/s c9 dither -110 dBFS unflushed         76.7 /     79.8
  decode 64 kb/s c9 dither -110 dBFS unflushed         10.2 /     11.9
  encode 64 kb/s c9 dither -90 dBFS unflushed          78.2 /     81.9
  decode 64 kb/s c9 dither -90 dBFS unflushed          10.8 /     12.5
  encode 64 kb/s c9 no frames unflushed                 0.0 /      0.0
Chain rows (median / p99 us):
  chain: nothing (gate, limiter)  2.4 / 2.8
  chain: WebRTC NS (high)        16.1 / 20.7
  chain: RNNoise                 30.8 / 37.7
  chain: DeepFilterNet3 (CPU)   253.7 / 319.7
$ cat <tmp>/summary.md
### Call path against the Frame budget
```
(identical report, fenced, appended to GITHUB_STEP_SUMMARY)
```
```
The script consumed voice_profile's TSV (chain/engine rows) and opus_profile's TSV (incl. Task 4's silence/dither/no-frames rows, shown in Parts), and wrote the table and the summary.

## Task 4: silence options measured (opus_profile -- 10 --tsv)
Status: PROVEN
```console
$ target/release/examples/opus_profile 10 --tsv     (name, then cost columns, packets sent, kb/s; first 64 kb/s c9 unflushed rows; 72 rows total, flushed/unflushed x 3 bitrates)
encode 64 kb/s c9 voice in noise unflushed	26.5	26.2	31.6	36.0	100	64.4
encode 64 kb/s c9 digital silence unflushed	8.8	8.7	10.8	12.1	5	0.1
encode 64 kb/s c9 dither -110 dBFS unflushed	76.8	76.4	84.5	86.5	100	30.6
encode 64 kb/s c9 dither -90 dBFS unflushed	78.5	78.2	83.0	89.5	100	35.7
encode 64 kb/s c9 no frames unflushed	0.0	0.0	0.0	0.0	0	0.0
encode 32 kb/s c9 digital silence unflushed	67.9	67.6	73.4	78.5	5	0.7
encode 12 kb/s c10 dither -90 dBFS unflushed	71.4	71.1	75.7	76.0	100	6.4
$ target/release/examples/voice_profile --tsv default 200
chain: default (HPF AEC3 RNN AGC2)	108.2	105.1	163.6	201.9	51.5
chain: default with DeepFilterNet3	339.2	332.0	541.5	564.0	1074.5
engine: default, 0 voices mixed	93.0	95.4	103.8	104.7	51.5
engine: default, 4 voices mixed	100.5	103.5	114.5	115.6	51.5
engine: default, 16 voices mixed	112.3	114.4	141.8	153.0	51.5
engine: default, 4 voices, 1 cleaned	364.6	361.7	432.6	436.7	1074.5
engine: default, 4 voices, 4 cleaned	1136.5	1129.9	1295.6	1353.9	4143.5
engine: default, 8 voices, 8 cleaned	1147.8	1137.1	1420.5	1566.4	4143.5
```
Silence rows show cost, packets sent (5 vs 100) and kb/s; stopping frames costs 0, dither costs a full encode.

## Task 3: profile.yml workflow lints and has the right step order
Status: PROVEN (static only)
```console
$ actionlint .github/workflows/profile.yml     (actionlint 1.7.12)
actionlint_exit=0
$ python3 -I yaml parse: on = {'workflow_dispatch': None}
1 actions/checkout@v6
2 Install Linux build dependencies
3 Use clang 22
4 Fetch pinned libwebrtc
5 Swatinem/rust-cache@v2
6 Fetch models
7 Profile the call path
8 No unverified libwebrtc download
```
Manual trigger only; libwebrtc and models fetched before the profile step; the download check runs after the build.

## scripts/check-no-webrtc-download.sh after the build
Status: PROVEN
```console
$ scripts/check-no-webrtc-download.sh
exit=0
```
(Run after builds with LK_CUSTOM_WEBRTC set to the fetch-libwebrtc.sh directory.)

## NOT PROVEN
- The aarch64 job summary: the workflow has not been run on a GitHub aarch64 runner (needs the human to push and trigger it); only actionlint and the step order were checked.
- E-core numbers (--cpu pinned run) and any Steam Frame numbers: no device; the run above was unpinned on an x86_64 P-core host.
- By-ear items (cleaning quality, audible effect of dither or stopped frames) and the Frame's call defaults: human checkpoint.
- Step 15 stays open, as the plan states.
