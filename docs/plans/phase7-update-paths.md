# Updates for the Flatpak and Downloaded Packages (Phase 7 amendment)
Status: ANSWERED (human, 2026-10-17): Q15 (c). Written 2026-10-17 while writing PR #28's description. The fallback (b) is folded into Task 9's commit (`UpdateMechanism::detect`). The repositories (a) are part of Task 12, whose text in the plan now has them, and keeps only the latest release on Pages. It amends Q7 and Task 9 (the update check) and Tasks 7 and 12 (the Flatpak, and the apt and dnf repositories) of `docs/plans/phase7-packaging.md`.

## Problem
Q7 says that on Linux "the package manager" updates the Flatpak and the `.deb`. Task 9 implemented this in `crates/core/src/api/updates.rs` (`UpdateMechanism::detect`):
- **The Flatpak:** `/.flatpak-info` gives `Flatpak`.
- **`.deb`, `.rpm` and AUR:** a binary under `/usr` but not `/usr/local` gives `PackageManager`.
- **The result:** `checks_itself()` is false for both, and the settings card says "Your package manager keeps this copy of Twicecat up to date." (`crates/app/src/updates.rs:30`).

Today nothing keeps those copies up to date:
- **The Flatpak** ships only as the release's `.flatpak` bundle. `scripts/flatpak.sh:166` builds it with `flatpak build-bundle --runtime-repo=<Flathub>` and no `--repo-url`, so `flatpak update` has nothing to pull from (below).
- **A `.deb` or `.rpm` downloaded from the release** is installed with no repository behind it until Task 12's apt and dnf repositories exist and the user has added them by hand.

These users get no update notice. They have to come back to the release page on their own.

## Facts verified (2026-10-17)
- **A bundle install's update source is the bundle's `--repo-url`.** In flatpak 1.16.1, installing a bundle runs `flatpak_dir_ensure_bundle_remote` (`common/flatpak-dir.c:10150`). That creates a remote named after the last part of the app ID, `twicecat-origin` (`create_origin_remote_config`, `:14482`), whose `url` is the bundle's origin or `""` when there is none, with `gpg-verify` on only when the bundle carries keys. `flatpak-build-bundle(1)`: `--repo-url` is "the URL for the repository from which the application can be updated. Installing the bundle will automatically configure a remote for this URL."; `--gpg-keys` adds the key the remote verifies with. Our bundle sets neither.
- **The app cannot see its origin from inside the sandbox.** `/.flatpak-info` (written in `common/flatpak-run.c:1420-1489`) carries the app and runtime names, paths and commits, the branch, the architecture and flatpak's version, but no origin or remote. Whether a Flatpak can update has to be known when it is built.
- **A `.deb` or `.rpm` can tell whether its repository is set up.** Task 12 has users write `/etc/apt/sources.list.d/twicecat.sources` and `/etc/yum.repos.d/twicecat.repo`, and those paths are readable by any user. The AUR package is updated by pacman helpers; pacman records it as `/var/lib/pacman/local/twicecat-bin-<version>/`.
- **GitHub Pages:** a published site "may be no larger than 1 GB", with a soft bandwidth limit of 100 GB a month (docs.github.com, "GitHub Pages limits", read 2026-10-17). Task 6 measured the x86_64 `.deb` and `.rpm` at 83.6 MB each. The aarch64 ones were not measured; the aarch64 `.tar.zst` is 67.4 MB against 70.6 MB on x86_64. Task 12's "last three releases" of `.deb` and `.rpm` for both architectures therefore comes to about 1 GB, at the limit and not "far below" it as the plan says, before any Flatpak repository is added.

## Q15. How do Flatpak and downloaded-package users get updates?
- **(a) Real repositories for everything, in Task 12.**
  - **Flatpak:** a repository on the same Pages site, built with `flatpak build-update-repo` and signed with Task 12's key. The bundle is built with `--repo-url=https://ga-lep.github.io/twicecat/flatpak/` and `--gpg-keys`, so a bundle install becomes a tracked `twicecat-origin` remote that `flatpak update`, GNOME Software and KDE Discover update. A `.flatpakref` gives a one-click install.
  - **`.deb` and `.rpm`:** each package ships its own repository entry and key, as VS Code's and Chrome's do: `/etc/apt/sources.list.d/twicecat.sources` with `Signed-By` and the key in `/usr/share/keyrings/`, and `/etc/yum.repos.d/twicecat.repo` with `gpgcheck=1`. A downloaded package then enrols itself in updates.
  - **Cost:** the Pages site keeps only the **latest** release. That is about 0.33 GB of `.deb`/`.rpm` and, by the sizes above, roughly as much again for the Flatpak objects, so under 1 GB. Older versions stay on the release pages.
  - **Downside:** Debian discourages packages that add their own sources, though it is common for third-party apps.
- **(b) A check-and-link fallback, now, in Task 9's code.**
  - **Flatpak:** the Flatpak checks like Windows and macOS (opt-in, daily, a link to the release page), unless the build says it comes from a repository. That would be a build-time variable the manifest sets, like `TWICECAT_UPDATER_PUBKEY` for the AppImage.
  - **`.deb`, `.rpm` and AUR:** a `/usr` install counts as `PackageManager` only when its repository entry exists (`/etc/apt/sources.list.d/twicecat.sources`, `/etc/yum.repos.d/twicecat.repo`) or pacman installed it. Otherwise it checks and links.
  - **Cost:** small. `UpdateMechanism::detect` gains two inputs, its table tests gain the new rows, and the card's note names the right mechanism.
  - **Downside:** Flatpak users update by downloading the new bundle and running `flatpak install --user --reinstall`.
- **(c) (R) Both: (b) now, (a) in Task 12.**
  - **Order:** (b) is folded into Task 9's commits (rule 5), so no install is left silent. (a) then replaces the link with real updates as each repository goes live: the `.deb`/`.rpm` check sees the entry the package installed, and the Flatpak manifest sets the build-time variable once the bundle carries `--repo-url`.
  - **Task 12 changes:** keep only the latest release on Pages, and add the Flatpak repository to its job.

### Files for (c)
- **For (b):** `crates/core/src/api/updates.rs` (`detect`: the repository entries, pacman's record, and the Flatpak build variable; tests), `crates/app/src/updates.rs` (the note per mechanism; tests), `packaging/flatpak/io.github.ga_lep.Twicecat.yml` (the variable left unset until (a)), `docs/plans/phase7-packaging.md` (Q7's note), PROGRESS.md and the ledger.
- **For (a), in Task 12:** `scripts/flatpak.sh` (`--repo-url`, `--gpg-keys`, `build-update-repo`), `scripts/build-repos.sh` (the Flatpak repository, only the latest release), `packaging/nfpm.yaml` (the repository entry and key as config files), `packaging/flatpak/` (a `.flatpakref`), `.github/workflows/release.yml` (the `repositories` job), and `docs/install.md`.

### Acceptance for (c)
- **(b):** table tests of `detect` cover a Flatpak with and without the build variable; `/usr` with an apt entry, a dnf entry, pacman's record and none; and `/usr/local`. The app tests show the card's note and that a check runs for each.
- **(a):** in containers, a bundle built with `--repo-url` pointing at a local web server installs and then updates with `flatpak update` to a second build. A downloaded `.deb` (`ubuntu:26.04`) and `.rpm` (`fedora:44`) install their repository entry, and `apt upgrade` or `dnf upgrade` takes the next version from the local server, with signatures checked. A tampered repository is refused.

### Human checkpoint additions (rule 9)
- [ ] The Flatpak installed from the release bundle shows the update link (b), and once (a) is live, Discover (KDE, the Steam Frame's desktop mode) and GNOME Software offer the next release.
- [ ] A `.deb` downloaded from the release on a clean Ubuntu 26.04 updates with `apt upgrade` once (a) is live.
