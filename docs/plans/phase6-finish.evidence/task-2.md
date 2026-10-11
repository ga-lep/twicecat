# Evidence — Task 2: profile the call path against the Frame budget
Commit: "feat(scripts): measure the call path against the Frame budget in one command" (subject; SHAs were rewritten when the history was tidied)
Environment: CC=clang-22 CXX=clang++-22, release builds, x86_64 AMD Ryzen 7 7800X3D, DeepFilterNet3 models present. Outputs below are trimmed (cut/head) where noted; figures vary run to run.

## voice_profile / opus_profile --tsv print only TSV lines, same rows/figures as table mode
Status: PROVEN
```console
$ target/release/examples/voice_profile --tsv default 200      (exit=0, stderr 0 bytes)
chain: default (HPF AEC3 RNN AGC2)	108.8	110.8	138.9	156.9	51.5
chain: default with DeepFilterNet3	369.9	366.3	480.7	510.3	1074.5
engine: default, 0 voices mixed	100.2	102.8	113.1	115.9	51.5
engine: default, 4 voices mixed	109.5	112.5	124.1	151.1	51.5
engine: default, 16 voices mixed	112.4	114.5	123.2	124.0	51.5
engine: default, 4 voices, 1 cleaned	370.9	362.4	558.4	616.0	1074.5
engine: default, 4 voices, 4 cleaned	1140.1	1120.0	1465.1	1562.3	4143.5
engine: default, 8 voices, 8 cleaned	1140.2	1126.0	1325.1	1351.6	4143.5
$ target/release/examples/voice_profile default 200   (table mode, same 8 rows; mean/median/p99/worst/allocs)
chain: default (HPF AEC3 RNN AGC2)     99.6    102.0    119.5    145.3     51.5
... (same 8 row names; allocs identical, timings differ by run noise)
$ target/release/examples/opus_profile --tsv 2     (exit=0, stderr 0 bytes; first rows of 36)
encode 64 kb/s c9 voice in noise unflushed	26.5	26.1	35.9	35.9	100
decode 64 kb/s c9 voice in noise unflushed	7.5	7.4	11.8	11.8	100
encode 64 kb/s c9 digital silence unflushed	8.8	8.7	11.5	11.5	5
$ target/release/examples/opus_profile 2    (table, mean)
 64 kb/s complexity  9, voice in noise : encode   26.7 µs, decode   7.5 µs, 100 % of packets sent
 64 kb/s complexity  9, digital silence: encode    8.9 µs, decode   3.0 µs,   5 % of packets sent
```
Only tab-separated rows on stdout, nothing on stderr; same rows and matching figures (opus mean 26.5 vs 26.7, packets-sent % equal; run-to-run noise on timings). Opus table shows mean only, TSV adds median/p99.

## scripts/profile-call-path.sh [--voices N] [--frames N] [--cpu C]: per-mode median/p99 as share of 500 us budget, host info (evidence hint command)
Status: PROVEN (OVER mark not triggered, see below)
```console
$ scripts/profile-call-path.sh --voices 4 --frames 1000      (exit=0, 3m12s incl. the build step)
building the profile examples (release, --locked)...
Call path per 10 ms frame, 4 remote voices, 1000 frames, 64 kb/s Opus complexity 9
Budget: 5 % of a core = 500 us per 10 ms

Host: x86_64, AMD Ryzen 7 7800X3D 8-Core Processor
Pinned to CPU: no
...
mode                   scenario  median us  budget    p99 us  budget
off                    talking        63.9   12.8%      69.6   13.9%
off                    silent         28.5    5.7%      32.2    6.4%
WebRTC NS              talking        78.3   15.7%      91.4   18.3%
WebRTC NS              silent         42.9    8.6%      54.0   10.8%
RNNoise                talking        94.7   18.9%     111.8   22.4%
RNNoise                silent         59.3   11.9%      74.4   14.9%
DeepFilterNet3 CPU     talking       311.3   62.3%     358.0   71.6%
DeepFilterNet3 CPU     silent        275.9   55.2%     320.6   64.1%
(then Parts and Chain rows sections, 12 more lines)
```
All four modes (DeepFilterNet3 included since models present) with median and p99 as % of budget and host line. Build uses `cargo build --release --locked` (read in script). No row exceeded the budget on this host, so the OVER mark was not observed in output; its logic is in the script's awk (`flag = med>budget || p99>budget`). That part is NOT PROVEN by run.

## --cpu pins with taskset
Status: PROVEN
```console
$ GITHUB_STEP_SUMMARY=<tmp>/summary.md scripts/profile-call-path.sh --voices 2 --frames 200 --cpu 3   (exit=0)
Call path per 10 ms frame, 2 remote voices, 200 frames, 64 kb/s Opus complexity 9
Pinned to CPU: 3
... table printed for all 4 modes ...
$ taskset -c 3 sh -c 'taskset -p $$'
pid 1112215's current affinity mask: 8
```
Report says "Pinned to CPU: 3" and the runs succeeded under taskset (an invalid CPU fails, see below, which shows taskset is really invoked).

## writes to $GITHUB_STEP_SUMMARY when set
Status: PROVEN
```console
$ head -5 <tmp>/summary.md
### Call path against the Frame budget
```
Call path per 10 ms frame, 2 remote voices, 200 frames, 64 kb/s Opus complexity 9
Budget: 5 % of a core = 500 us per 10 ms

$ tail -2 <tmp>/summary.md
  chain: DeepFilterNet3 (CPU)                         250.0 /    260.3
```
```
Report appended in a fenced block (29 lines) to the temp file.

## exits non-zero if a profile run fails
Status: PROVEN
```console
$ scripts/profile-call-path.sh --frames 200 --cpu 9999
building the profile examples (release, --locked)...
taskset: failed to set pid 1112268's affinity: Invalid argument
exit=1
$ scripts/profile-call-path.sh --voices abc   -> usage: ... ; exit=2
$ scripts/profile-call-path.sh --frames 50    -> error: --frames must be at least 100 ; exit=2
$ scripts/profile-call-path.sh --bogus        -> usage ; exit=2
```
A failing profile run (taskset on a nonexistent CPU) aborts with exit 1 and no report; bad arguments exit 2. Repo unchanged apart from this file (git status clean before writing it).
