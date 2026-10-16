Plan: drafted 2026-10-14 on feat/phase7-packaging (from origin/main 072206e), with the build measurements (CPU levels, LTO, GPU build, GPU pack sizes); answered and approved by the human 2026-10-14: Q2 (a), Q3 (b), Q5 MSI, Q6 Windows unsigned for now, Q7 (check and link on Windows/macOS; package manager and cargo-packager-updater on Linux), Q8 (a), Q9 (a), the rest as recommended; Q13 (apt, dnf, AUR) answered 2026-10-14: nfpm, GitHub Pages, no older distributions, AUR twicecat-bin
Task 1: complete (gate ok: 1735 passed; v4 refusal verified on Linux, Windows check assembled and inspected, not run)
Task 2: complete (gate ok: 1735 passed; measure-builds.sh reproduces the plan's numbers; v4 built via --config and --target; actionlint and shellcheck clean via Docker)
Task 3: complete (gate ok: 1738 passed; bundled models found from a fake prefix with an empty HOME; mutation caught)
Task 4: complete (gate ok: 1740 passed; desktop-file-validate and appstreamcli --strict valid; icon mutation caught)
Task 5: complete (gate ok; notices for 4 targets resolve; unaccepted licence and missing file both fail)
Task 6: complete (gate ok; tar, deb, rpm, AppImage built in Ubuntu 26.04 and all 4 smoke-tested; wrong dependency caught; MSI and DMG config dry-run only, built in CI)
Task 7: complete (gate ok on Windows; flatpak.sh check ok, two mutations caught; x86_64 bundle built in Flathub's CI image in Docker and smoke-tested; aarch64 in the release workflow)
Task 8: written (actionlint+shellcheck clean; signing hooks read in cargo-packager 0.11.8; workflow NOT run yet: the human pushes and dispatches the dry run)
