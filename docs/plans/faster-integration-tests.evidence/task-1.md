# Evidence — Task 1: time the homeserver integration tests, split the CI build
Commit: a97e0ad
Environment: CC=clang-22 CXX=clang++-22; docker present but no containers running (`docker ps` empty), so the homeserver stack was not started. shellcheck is not installed. Stack-dependent criteria are NOT PROVEN.

## Builds once, prints build seconds, lists every test binary with ignored tests
Status: PROVEN (build and discovery only; the build was already warm)
```console
$ scripts/time-integration-tests.sh --dry-run
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.30s
build: 0.4 s
would run: avatars (1 ignored tests)
would run: bridge (3 ignored tests)
... (middle trimmed)
would run: e2ee_recovery (7 ignored tests)
would run: e2ee_rooms (6 ignored tests)
would run: e2ee_verification (9 ignored tests)
would run: log_hygiene (1 ignored tests)
would run: login (4 ignored tests)
would run: media (8 ignored tests)
would run: notifications (7 ignored tests)
would run: oauth (1 ignored tests)
would run: room_list (8 ignored tests)
would run: rooms (14 ignored tests)
would run: rtc_stack (3 ignored tests)
would run: threads (8 ignored tests)
would run: timeline_actions (12 ignored tests)
would run: timeline_items (3 ignored tests)
would run: timeline_live (4 ignored tests)
would run: timeline (3 ignored tests)
```
Dry run printed "build: N s" and the binaries with ignored tests, and wrote no TSV (`target/integration-timings` absent afterwards).

## Runs each binary and writes binary<TAB>seconds<TAB>result TSV; --per-test rows; --first-sync joined-room counts
Status: NOT PROVEN (needs the local homeserver stack, not running; HUMAN)

## Refuses a non-local homeserver
Status: PROVEN
```console
$ HOMESERVER_URL=https://matrix.example.org scripts/time-integration-tests.sh --dry-run; echo exit=$?
refusing to use HOMESERVER_URL: it must be http://127.0.0.1:<port>, http://localhost:<port> or http://[::1]:<port> (test accounts are local only)
exit=1
```
Refused before building anything.

## Unknown flag handling / help
Status: PROVEN
```console
$ scripts/time-integration-tests.sh --bogus; echo exit=$?
unknown argument: --bogus
exit=2
$ scripts/time-integration-tests.sh --help   # exit=0, prints usage header (flags --per-test, --first-sync, --dry-run)
```

## Never prints tokens, passwords, room names, bodies
Status: NOT PROVEN at runtime (code read only: test output goes to the .log file; first-sync prints only counts; password passed via env to jq, token in a header file). Needs the stack.

## Passes shellcheck
Status: NOT PROVEN (shellcheck not installed here; the script carries SC2086 disable directives)

## ci.yml: separate "Build integration tests" step before the run step
Status: PROVEN (diff)
```console
$ git diff 7e86efd..a97e0ad -- .github
+      # Built before the run so the build and run durations show separately.
+      - name: Build integration tests
+        run: cargo test -p twicecat-core -p twicecat-app --no-run --locked
       # The real-keychain test needs a desktop session, and hardware_ tests a
       # microphone or speakers; both are human-run.
       - name: Run homeserver integration tests
```
Only the added step; run step untouched, so coverage is unchanged. Step durations in CI: NOT PROVEN (HUMAN).

## Gate passes, no test changes
Status: NOT PROVEN (gate not run by me). `git diff --stat 7e86efd..a97e0ad` touches only ci.yml, PROGRESS.md, the progress ledger and the new script: no test files.
