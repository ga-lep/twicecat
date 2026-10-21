# Custom Theme Plan
Status: APPROVED

## Goal
Replace the desktop app's four built-in colour sets with one built-in colour theme, and let the user add their own colour themes as files: light, dark, or a light/dark pair.

- **The built-in theme** is "Twicecat", taken from the colours in the project's design folder (`twicecat-design/design/tokens.json`, `README.md` and `components/*.md`). It has one look. The four families (Twicecat, Frost, Ember, High contrast) and the swatch picker go away.
- **Custom themes** are JSON files in a `themes/` folder in the OS config directory, in the same schema as the design's `tokens.json`, with neutral token names (`accent`, not `violet`). Only colours are read from them. The user picks one on Settings → Appearance → Theme.
  - A theme with one look (one file, light or dark) is shown as it is, in its own mode, whatever the OS mode or the stored Light/Dark choice says. No radio is shown for it.
  - `<Name>.light.json` and `<Name>.dark.json` together form one theme "Name" with both looks. For that theme the page shows the "Follow the system" / "Light" / "Dark" radios: System follows the OS mode, Light and Dark force one look.
- **Safety:** a theme file is untrusted input. It is loaded under hard limits, off the UI thread. It may set only the 15 colour tokens listed below. It is checked for readability and for distinct warning, error and success colours: any theme the app shows has readable text and visible trust signals. If a colour is too faint it is adjusted. If the status colours collide, the theme is refused and the built-in one is used, with a one-line note.
- **Examples ship:** `Daylight.light.json`, a light theme, in `docs/themes.md` and as a test fixture; and `crates/app/assets/themes/Twicecat-copy.json`, the built-in colours under the neutral names, as the starting file.

Only the colours change (answers Q5 and Q6):
- fonts stay Inter 4.1;
- `ui/tokens.rs` sizes, spacing and radii stay as they are;
- `scripts/check-font-assets.sh` and `THIRD-PARTY-NOTICES.md` do not change;
- no screen is redesigned: layouts, the kit's components and the slot invariants stay as they are.

**Where the work goes (Q1):** the current branch `feat/call-sounds`, after the call-sounds work there (the `snd.*` commits). No new branch is made. Commits carry `Step: thm.N` trailers.
- The ledger is `docs/plans/custom-theme.progress.md`.
- **State at this revision:** Tasks 1 and 2 are committed (`Step: thm.1`, `Step: thm.2`) with their evidence and ledger commits. Commit hashes are not written into this plan, because the branch history is being rewritten while it is revised: each task finds its commits with `git log --format=%H --grep='^Step: thm.N$' main..HEAD`.
- **Revision (user decisions of the previous round):** neutral token keys; the user chooses the theme, light or dark; no "dark" wording for the built-in theme; a light example "Daylight" ships; the theme loader runs off the UI thread. Task 2b folds the first four into the `thm.1` and `thm.2` commits; Task 4 carries the loader change.
- **Revision (this round):** light/dark pairs stay. `<Name>.light.json` + `<Name>.dark.json` form one theme with both looks, exactly as thm.2 already loads them (`Resolved.both_modes`, pairing in `load_dir`, the duplicate check by name and mode); Task 2b no longer removes any of that. The radios are shown only for a paired theme; single-look themes (Twicecat, Daylight, `Nord.json`, a lone `Nord.dark.json`) show no radio and no note. `ThemeChoice`, `UiSettings.theme` and the OS mode tracking are used again (for pairs), so they stay (Q13). Q12 and Q14 are resolved.

### What exists today (surveyed at `3994f82`, the base of `feat/call-sounds`; Tasks 1 and 2 since replaced the theme code, see their sections)
- **`crates/app/src/theme.rs`** (after thm.2): `Tokens` (15 colours, fields `violet_wash`, `violet`, `on_violet`, `violet_bright` among them), `BUILT_IN`, `map_tokens`, `Resolved { name, dark, both_modes, palette, semantic, report }`, `built_in_resolved()`, `BUILT_IN_NAME = "Twicecat"`, `theme_of()` (the only `Theme::custom`), `built_in()`, the registry (`RwLock<Vec<(String, Palette, Semantic)>>`, `REGISTRY_CAP = 65`), `semantic()`, `readable_on/at/over/on_every`, `TOKEN_NAMES` (with `violet*`), `Refusal`, `resolve()` with V1–V5, `delta_e76`, `test_themes()`. The module docs say the theme is dark "whatever the system or the stored `ThemeChoice` says".
- **`crates/app/src/themes.rs`** (thm.2): `parse`, `load_dir`, `ParseNotes`, `FileProblem`, `Note`, `ReportLine`, `Loaded`, `name_allowed`, `read_theme_file`. `load_dir` pairs `X.light.json`/`X.dark.json` into `both_modes` (both `Resolved` entries of the pair are in `Loaded.themes`, `themes.rs:431-445`); a duplicate is "same name and mode" (`themes.rs:411-418`, text "same name and mode as another file" at :80). Its doc comment has one line over 100 columns. `read_theme_file` documents the residual FIFO-swap race.
- **`crates/app/src/screens/settings.rs`** `appearance_page`: "Theme: Twicecat", then, since `both_modes` is false, the note "This theme has a dark look only." instead of the "Follow the system" / "Light" / "Dark" radios (the radio code is still there, `screens/settings.rs:295-337`).
- **`crates/app/src/settings.rs`**: `SettingsMessage::Theme(ThemeChoice)` (only the hidden radios send it), `ALL_NAMES: [&str; 24]`.
- **`crates/app/src/state.rs`**: `current_theme()` returns `theme::built_in()`; `Message::ThemeMode` and `theme_mode: theme::Mode` track the OS mode (phase 2.8 code) without effect on colours. Before thm.1, phase 2.8's `resolve_mode` mapped `System` to the OS mode and `Light`/`Dark` to themselves, and anything but `Dark` (including an unknown OS mode, `Mode::None`) gave the light look (`git show 3994f82:crates/app/src/theme.rs`, :375-392).
- **`crates/app/src/blocking.rs`**: `run()` runs jobs on their own threads under `MAX_PARALLEL = 2` permits shared with image decoding; iced's executor is not a tokio runtime (`blocking.rs:8`).
- **`crates/app/tests/ui_gallery.rs`**: `THEMES: [(ThemeChoice, &str); 1]` (:38), `the_appearance_page_names_the_one_theme_and_has_no_swatches_or_mode_radios` (:3158), `a_settings_file_with_an_older_palette_shows_twicecat` (:3195), comments at :37 and :3501 about "a dark look only".
- **Fixtures:** `crates/app/assets/themes/twicecat.tokens.json` (+ `SOURCE.md`), `crates/app/tests/fixtures/themes/{Paper.light,Duo.light,Duo.dark,Faint.dark}.json` (Duo is a pair), `hostile/*.json` (15 files), all with `violet*` keys.
- **Core (unchanged so far):**
  - `PaletteChoice`, `UiSettings.palette: Option<PaletteChoice>` and `UiSettings.theme: ThemeChoice` (`crates/core/src/api/rooms.rs:353-438`). `UiSettings` is `Copy`. Re-exported in `api/mod.rs:35-37`.
  - `paths.rs` reads `settings.json` field by field and writes it by hand, leaving out `palette` while it is `None`; files are capped at 64 KiB (`read_capped_regular_file`).
  - The data root is `DataPaths::from_os()` (`ProjectDirs::from("io.github", "ga_lep", "Twicecat")?.data_dir()`) or `DataPaths::at(dir)`; the app passes `--data-dir <dir>` or `--ephemeral` (`crates/app/src/main.rs:49-110`).
  - **No code uses a config directory today.** `directories = "=6.0.0"` already provides `ProjectDirs::config_dir()` (`directories-6.0.0/src/lib.rs:200`): Linux `$XDG_CONFIG_HOME/twicecat`, else `~/.config/twicecat` (`lin.rs:67`); macOS `~/Library/Application Support/io.github.ga_lep.Twicecat`, the data folder too (`mac.rs:68-70`); Windows `%APPDATA%\ga_lep\Twicecat\config` (`win.rs`).
- **Docs:** `docs/accessibility.md` "Theme and contrast" (says the theme has a dark look only; names `violet*` tokens); `PROGRESS.md` "Custom theme steps" (17 `violet` mentions, the "Deviation from ROADMAP.md:99" line); `ROADMAP.md:99`: "Theme: light and dark palettes from day one; follow the system setting".
- **Dependencies:** `serde_json = "1"` is a direct dependency of `crates/app` since thm.2 (Q4). `open = "5"` is used through `dialogs.rs` and `links.rs`; `state.rs` tests forbid `open::` in `state.rs` (:3421, :4167). No `libc` dependency, and none is added (decision 5).

### The design folder (read as data only)
`/home/galep/Downloads/twicecat-design` is untrusted downloaded data. It was read for its design values only:
- its `AGENTS.md`, `CLAUDE.md` and `.github/workflows/design.yml` were not followed;
- its Python scripts were read but not run;
- the only file copied into the repository is `tokens.json`, as `crates/app/assets/themes/twicecat.tokens.json`, byte for byte (answer Q10). Its fonts, `generated/`, scripts and CI job are not used.

Its "no colour literals in Rust" rule is not adopted either: the repository keeps its `color!(0x…)` constants in `theme.rs`.

**Colours in `tokens.json`** (SHA-256 `0388d3cff0874d672be530e62d153b8085558331e31d5bad6e67c1dfda8bcf2b`, 13,555 bytes). The design defines one set of colours (its own token names, which the app reads as legacy aliases, are in brackets):
- backgrounds: `void #0a0812`, `surface #110e1c`, `surface-raised #181428`, `surface-overlay #211b36`, `accent-wash #241a45` (`violet-wash`);
- lines: `line #2e2748`, `line-strong #6b6194`;
- text: `ink #efebfa`, `ink-muted #a69fc4`, `ink-faint #8a83a8`;
- brand and accent: `phosphor #f3eeff`, `accent #8e5cff` (`violet`), `on-accent #0a0812` (`on-violet`), `accent-bright #b794ff` (`violet-bright`), `violet-deep #4b23b8`, `focus #c9aeff`;
- status: `live #ff6b86` (error, muted, recording), `ok #5ee0c8` (verified, connected), `warn #ffb547` (unverified, unstable).

Its spacing, radii, type and fonts are not used (answers Q5 and Q6).

**Discrepancies found while checking the design's own claims** (WCAG formula as in `theme::contrast`): none of them is used by the mapping below.
- `ink` on `violet-deep` is 8.04:1, not the "9.4:1" its description gives.
- `ink-faint` on `accent-wash` is exactly 4.50:1.
- `line-strong` on `surface-overlay` is 2.95:1; the design claims 3:1 only on `surface` and `surface-raised`.

### Decisions

#### 1. Design colours → the app's palette and semantic colours
Every app colour comes from one token, through this fixed table. A theme file sets tokens, never the app's fields directly. Token names are neutral; the design's names are accepted as aliases (see 3).

| App field | Token (design alias) | Built-in value | Why |
|---|---|---|---|
| `Palette.background`, `Semantic.sidebar` | `void` | `#0a0812` | "Deepest background: app window". The design has no sidebar token, so the sidebar is `void` too. |
| `Semantic.surface` | `surface` | `#110e1c` | Main panels. |
| `Semantic.surface_raised` | `surface-raised` | `#181428` | Cards, menus, secondary buttons. |
| `Semantic.message_hover` | `surface-overlay` | `#211b36` | "Row hover". |
| `Semantic.selection`, `Semantic.message_highlight` | `accent-wash` (`violet-wash`) | `#241a45` | "Selected row". |
| `Palette.text` | `ink` | `#efebfa` | |
| `Semantic.muted` | `ink-muted` | `#a69fc4` | |
| `Palette.primary`, `Semantic.highlight` | `accent-bright` (`violet-bright`) | `#b794ff` | The app also uses `primary` as text: links, the info severity and the quote bar. The design allows only this accent as text (`accent` is 4.38:1 on `surface-raised`). |
| `Semantic.badge`, `Semantic.speaking` | `accent` (`violet`) | `#8e5cff` | Unread pill and speaking ring. |
| `Semantic.badge_text`, `Semantic.highlight_text` | `on-accent` (`on-violet`) | `#0a0812` | Text on the accent. |
| `Semantic.border` | `line-strong` | `#6b6194` | Control outlines, which need 3:1 on raised surfaces. `line` is not used. |
| `Semantic.focus` | `focus` | `#c9aeff` | |
| `Palette.danger` | `live` | `#ff6b86` | Error, muted, leave, identity changed. |
| `Palette.success` | `ok` | `#5ee0c8` | Verified, connected. |
| `Palette.warning` | `warn` | `#ffb547` | Unverified, unstable. |
| `Semantic.shield_red`, `Semantic.shield_grey` | `live`, `ink-muted` (glyph `on-accent` when it reaches 4.5:1) | | Added in thm.1 after the security audit. |
| `Semantic.danger_soft` | derived: `mix(surface-raised, live, 0.15)` | `#3b2136` | The design has no tinted backgrounds. The error banner keeps a tint so that it does not lose a cue the audits relied on. |

**Two accepted departures from the design's components (Q9).** They are documented in the `theme.rs` module docs, `docs/themes.md` and `docs/accessibility.md`:
- **Primary buttons** fill with `accent-bright`, not `accent`. `kit::primary_style` fills with `primary`, which is also the accent text colour. The label is black from `on_color` (8.69:1).
- **The mention badge** (`highlight`) is `accent-bright`, not the unread pill prefixed with "@". The app tells mentions and unread apart by colour.

The built-in theme, as computed while planning (the tests recompute these values): the tightest text pair is `on-accent` on `accent` at 4.85:1; `ink-muted` is at least 5.75:1 on every background, including `danger_soft`; `live` is at least 5.29:1; `focus` is at least 7.54:1; `accent` (the ring) is at least 4.38:1 on `void`, `surface` and `surface-raised`; `line-strong` is at least 3.22:1 on `surface` and `surface-raised`; the closest pair of status or accent colours, `accent-bright` and `ink-muted`, is ΔE76 39.5.

**Wording (user decision 3):** the built-in theme is called "Twicecat" (or "the built-in theme") everywhere: UI, docs, report lines, code comments. Nothing calls it "the dark theme", "dark-only" or "a dark look". Code that is about a theme's side (`Resolved.dark`, `is_dark`, `DARK_BELOW`, the `.dark.json` suffix) keeps those words, since it describes any theme.

#### 1b. Light, dark, or a pair (user decisions 2 and, this round, 1; replaces the old Q2 "dark only for now")
**Rule: a file holds one look, and its mode is the side its backgrounds are on (V2). A theme is either one look, shown in its own mode whatever anything else says, or a light/dark pair of one name, whose look the System/Light/Dark choice decides.**
- **Names.** `<Name>.json`, `<Name>.light.json` and `<Name>.dark.json` all name the theme `<Name>`. The optional `.light`/`.dark` suffix is a declaration the loader checks against the colours (a mismatch is refused: "says light but its background is dark" and the reverse, as in thm.2). A `<Name>.json` file counts in the mode its colours have.
- **Pairs (as thm.2 already does; nothing is removed).** When the accepted files of one name are one light and one dark look (for example `Nord.light.json` + `Nord.dark.json`, or `Nord.json` light + `Nord.dark.json`), they form one theme "Nord" with both looks: both `Resolved` entries stay in `Loaded.themes` and both have `both_modes == true`.
- **Duplicates (thm.2's check, kept).** A file with the same name **and** the same mode as one already accepted (the earlier in byte order of file names wins) is reported with thm.2's fixed text "same name and mode as another file" and not used. So `Nord.dark.json` + `Nord.json` (dark) gives one dark "Nord" and one report line (see Q16 for the wording).
- **A pair with one side refused** (or a lone side): the remaining look loads as a single-look theme (`both_modes == false`), and the refused file has its own report line.
- **Single-look themes** (the built-in Twicecat, Daylight, `Nord.json`, a lone `Nord.dark.json`): shown in their own mode. The stored `ThemeChoice` and the OS mode do not change them, and the Appearance page shows **no radio and no note** for them. The "This theme has a dark look only." note goes; nothing replaces it.
- **Paired themes:** the Appearance page shows the existing "Follow the system" / "Light" / "Dark" radios (`SettingsMessage::Theme`, phase 2.8 code, kept) under the theme menu. `Light` and `Dark` force that look; `System` follows the OS mode (`Message::ThemeMode`); an unknown OS mode (`Mode::None`) gives the light look, as phase 2.8's `resolve_mode` did. The radios are shown only while the selected theme is loaded and paired (not while it falls back to Twicecat).
- **What is kept, and why (Q13, resolved):** the core keeps `ThemeChoice` and `UiSettings.theme`, and the app keeps tracking the OS mode (`Message::ThemeMode`, `theme_mode`). With pairs they are used. They are deleted only if a later review finds them unused.
- **ROADMAP.md:99** ("light and dark palettes from day one; follow the system setting") is not edited. PROGRESS.md "Deviation from ROADMAP.md:99" is reworded: "The built-in Twicecat theme has one look. Light and dark pairs are supported again as theme files (`<Name>.light.json` + `<Name>.dark.json`): for a paired theme the window follows the system's light/dark setting, or the user's Light/Dark choice. A single-look theme, the built-in one included, is shown in its own mode. Daylight ships as the light example."
- High contrast is no longer built in. `docs/accessibility.md` says so and points to custom themes.

#### 2. What replaces `Family`, `PaletteChoice` and `DEFAULT_FAMILY`
- **`theme.rs`** (shipped in thm.1/thm.2, renamed in Task 2b): `Tokens` (15 colours, fields `accent_wash`, `accent`, `on_accent`, `accent_bright` and the unchanged others), `BUILT_IN`, `Resolved { name, dark, both_modes, palette, semantic, report }` (`both_modes` kept, 1b), `built_in()` named "Twicecat" (Q8, one look, `both_modes == false`), `resolve(tokens) -> Result<Resolved, Refusal>`, `theme_of(&Resolved) -> Theme`, the only `Theme::custom` call.
- **`semantic(&Theme)`** looks the theme up by name and palette in the process-wide registry (cap 65, unchanged; a pair takes two entries, which 32 files × 2 looks + Twicecat = 65 already allowed for); unknown themes fall back to the built-in semantic colours.
- **The Appearance page:** a "Theme" row. Task 2b makes it the label "Theme" with the value "Twicecat", and no radio and no note (Twicecat has one look; the radio code stays, shown only when `both_modes`). Task 4 makes the value a `kit::menu(pick_list(…))` inside `ui::guard::no_wheel`: "Twicecat (default)" first (Q8), then the custom theme names found (a pair listed once), with **Reload themes** and **Open themes folder** buttons, and the three radios under the menu when the selected theme is paired.
- **Users with a stored `"palette"` (Q3, approved):** thm.1 ignores `UiSettings.palette`; Task 3 replaces `PaletteChoice`/`palette` with `ThemeName`/`theme_name` under a new `theme_name` key; the old key is ignored and never written again; nothing panics and nothing is renamed `.bad`.
- **`ThemeName`** is `Copy`: a `[u8; 32]` plus a length, with `as_str()` and `TryFrom<&str>`. Valid names are 1 to 32 bytes of `[A-Za-z0-9 _-]`, with no leading or trailing space; "twicecat" and "default" are reserved, case-insensitively. `None` means the built-in theme.

#### 3. The custom theme files
- **Where they are (Q7):** `themes/` in the OS config directory, from `ProjectDirs::config_dir()` of the pinned `directories 6.0.0`.

  | OS | Folder |
  |---|---|
  | Linux | `$XDG_CONFIG_HOME/twicecat/themes`, else `~/.config/twicecat/themes` |
  | macOS | `~/Library/Application Support/io.github.ga_lep.Twicecat/themes` (next to `settings.json`; accepted, Q12) |
  | Windows | `%APPDATA%\ga_lep\Twicecat\config\themes` |

  - With `--data-dir <dir>`, themes are read from `<dir>/themes`; with `--ephemeral`, from `<ephemeral dir>/themes`, which is empty. No environment variable is added.
  - Core API (Task 3, additive): `DataPaths` gains a `themes: PathBuf` field, `themes_dir()` and `create_themes_dir()` (recursive, 0o700 on Unix).
  - The folder is created only by **Open themes folder**.
- **File names, mode and pairs:** see 1b. The theme's name is the file stem without `.json` and without a `.light`/`.dark` suffix, checked with the `ThemeName` rules. Nothing inside the file names the theme. `settings.json` stores the name only (`theme_name`); for a pair, which look is shown comes from the stored `theme` (`ThemeChoice`) and the OS mode.
- **Format:** JSON in the design's `tokens.json` (DTCG) schema. Only the `color` group is read:
  - each `color.<token>.$value` is read as a string or as an object's `hex` string; it must be `#rrggbb`, or `#rrggbbaa` with `aa = ff`, in ASCII hex; if `colorSpace` is present it must be `"srgb"`;
  - `components`, `alpha`, `$type`, `$description`, `$schema`, `$extensions` and every other group are ignored;
  - aliases (`"{color.ink}"`), named colours, `rgb()`, numbers, `null`, three-digit hex, full-width digits and anything non-opaque are invalid: that token takes the built-in value, and the report counts it;
  - unknown token names are ignored and counted; `line`, `ink-faint`, `phosphor` and `violet-deep` are known but unused, so they are ignored without a count;
  - no file contents are ever turned into floats.
- **Tokens a theme may set** (exactly 15, `TOKEN_NAMES`, in this order): `void`, `surface`, `surface-raised`, `surface-overlay`, `accent-wash`, `line-strong`, `ink`, `ink-muted`, `accent`, `on-accent`, `accent-bright`, `focus`, `live`, `ok`, `warn`. Nothing else: no fonts, sizes, includes, URLs or paths.
- **Legacy names (user decision 1):** `violet-wash`, `violet`, `on-violet` and `violet-bright` are read as aliases of `accent-wash`, `accent`, `on-accent` and `accent-bright`, so the design's `tokens.json` and files written from it still load.
  - An alias alone sets its token, with no report line.
  - If a file has both a neutral key and its alias, the neutral key wins, whatever either value is (presence decides, not validity), and the alias is counted. The report says, in fixed text: "1 older colour name ignored, its new name is set" / "N older colour names ignored, their new names are set".
  - The alias table is one `const` in `themes.rs`; it is the only place in app code and user docs that spells the old names. `docs/themes.md` says "files that use the design's original token names still load" without listing them.
- **Partial files:** a missing token takes the built-in value. A light theme therefore has to set its backgrounds and its text; otherwise V2 refuses it.
- **Loading limits** (thm.2, unchanged): at most 1,024 directory entries looked at; at most 32 `*.json` files with allowed names, in byte order, the rest reported as "N more files not read"; `symlink_metadata` before opening (symlinks, FIFOs and devices refused unopened); the opened handle checked again (same device and inode, regular file); `take(64 KiB + 1)`; one leading UTF-8 BOM stripped; `serde_json::from_slice::<Value>` with its recursion limit of 128; duplicate keys: the last wins.
- **Who parses (Q4, approved):** the app, with `serde_json = "1"` (thm.2).
- **When and where files are read (user decision 5):** never on the UI thread, and never through `blocking::run` (whose two permits are shared with image decoding, so a stalled read would starve images).
  - `themes::load_off_thread(dir) -> impl Future<Output = Option<Loaded>>` runs `load_dir` on a thread of its own (named `twicecat-themes`) and hands back the result through a `tokio::sync::oneshot` (as `blocking.rs` does; no runtime needed, no new dependency).
  - The app starts one load at boot (a `Task` in the boot batch) and one per **Reload themes** press. At most one load is in flight: while one runs, Reload is disabled and the page says "Reading the themes folder…".
  - Until the first load finishes, the window shows Twicecat and no "could not be used" line (the list is "not read yet", not "empty").
  - So the residual race of thm.2 (a file swapped for a FIFO between the check and the `open`) can stall only that one thread: the window keeps working with its current theme, and Reload stays disabled until the read ends. No `libc` dependency is added. `read_theme_file`'s docs say so.
  - There is no hot reload and no file watcher.
- **Reports:** view data, not logs. Nothing is logged, so `no_secret_logging.rs` is untouched.
  - One line per problem file, at most 5 lines, then "and N more".
  - Only the checked theme name is shown; a file whose name fails the rules is counted as "1 file with a name that is not allowed".
  - Fixed reasons: "not valid JSON", "larger than 64 KiB", "not a regular file", "could not be read", "no colours", "status colours look too alike", "backgrounds mix light and dark", "selected rows would not show", "colours cannot be made readable", "says light but its background is dark" (and the reverse), "same name and mode as another file" (thm.2's text, Q16).
  - Notes: "3 colours made more readable", "2 colours not understood, Twicecat's used", "1 unknown name ignored", "1 older colour name ignored, its new name is set".
  - File contents are never echoed.
  - If the selected theme is missing or refused (after the first load), Twicecat is used and the page says "Theme "X" could not be used: <reason>. Showing Twicecat." The stored name is kept, so fixing the file and pressing Reload applies it. If one side of a selected pair is refused, the other side is shown as a single-look theme, the radios disappear, and the refused file's report line says why.
- **File permissions:** themes are only read, so no mode is required of them. They are untrusted however they got there.

#### 4. Safety: what any shown theme guarantees
`theme::resolve` runs every theme through these rules, the built-in one included. The built-in theme must pass with no adjustment. (Shipped in thm.2; only the token names change in Task 2b.)

- **V1 opaque:** a token whose alpha is not `ff` takes the built-in value (reported).
- **V2 one side:** `void`, `surface`, `surface-raised`, `surface-overlay`, `accent-wash`, the derived `danger_soft` and iced's derived code background must all have a luminance below 0.18 (dark) or all at least 0.18 (light). Otherwise the theme is **refused**. This makes V3 always solvable. The side found here is the theme's mode (1b).
- **V3 contrast, adjusted automatically:** a foreground below its least contrast is mixed toward white (dark side) or black (light side) until it meets the least contrast against every background it is drawn on. Non-text tokens are adjusted first, because `on-accent` depends on `accent`.
  - Non-text, 3:1: `focus` on all five backgrounds; `accent` on `void`, `surface`, `surface-raised` and the disabled checkbox box; `line-strong` on `surface` and `surface-raised`; the shield fills on every row.
  - Text, 4.5:1: `ink`, `ink-muted`, `accent-bright`, `live`, `ok` and `warn` on all five backgrounds; `ink`, `ink-muted` and `live` on `danger_soft`; `ink` also on each background mixed 15 % toward it (pressed/hovered states); `on-accent` on `accent` and `accent-bright`; the shield glyphs on their fills.
  - A last pass refuses what is still short ("colours cannot be made readable").
- **V4 distinct colours** (CIE76 ΔE ≥ 20), else **refused** ("status colours look too alike"): `live`, `ok`, `warn`, `accent-bright` pairwise and each against `ink` and `ink-muted`; `focus` against `live`, `ok`, `warn`, `ink`, `ink-muted` and `line-strong`; `accent` against `live` and `ink-muted`.
- **V5 visible states**, else **refused** ("selected rows would not show"): `accent-wash` at least ΔE 8 from `void`, `surface` and `surface-raised`; `surface-overlay` at least ΔE 5 from `surface`, `surface-raised` and `accent-wash`.
- **What is not covered:** iced's own widget colours not styled by the kit (radio, toggler, slider, scrollbar, text selection) come from `Extended::generate(palette)`. They are not validated and stay on the human checkpoint.

**The light example to ship and test: "Daylight"**, `docs/themes.md` and `crates/app/tests/fixtures/themes/Daylight.light.json`, neutral keys only. It starts from the user's draft (`scratchpad/Daylight.light.json`). A planning check with the same formulas found the draft **fails V5**: `surface-overlay #dfe3ea` is only ΔE 4.3 from `surface-raised #eceef2` (needs 5), so the draft would be refused with "selected rows would not show". The proposed fix is `surface-overlay #d8dde6` (ΔE 6.7; `accent-bright` then 4.71:1 on it). Task 2b confirms the final values with `resolve` itself (which also checks the shields and iced's derived code background, not covered by the planning check) and adjusts further if needed; the values that ship are the ones the test pins.

| Token | Value (planned) |
|---|---|
| `void` | `#ffffff` |
| `surface` | `#f6f7f9` |
| `surface-raised` | `#eceef2` |
| `surface-overlay` | `#d8dde6` (draft `#dfe3ea`, see above) |
| `accent-wash` | `#d3e8e3` |
| `line-strong` | `#5b6672` |
| `ink` | `#14181d` |
| `ink-muted` | `#4a5560` |
| `accent`, `accent-bright` and `focus` | `#0b6b5c` |
| `on-accent` | `#ffffff` |
| `live` | `#b3112f` |
| `ok` | `#17622f` |
| `warn` | `#8a4b00` |

Planning check with `#d8dde6`: all backgrounds and `danger_soft #e3cdd5` are light (luminance ≥ 0.648); tightest text pair `live` on `danger_soft` 4.60:1; `accent-bright`/`focus` ≥ 4.71:1; `on-accent` on the accent 6.42:1; `accent` on the disabled box 3.43:1; closest V4 pair ΔE 22.5 (`ok`/`accent-bright` and `ok`/`focus`); V5 wash ≥ 9.0, hover ≥ 6.7.

"Paper" stays as a test-only light fixture (keys renamed); it is no longer in the docs.

### Rules for every task (AGENTS.md)
- **Rule 1:** the human allowed this work on `feat/call-sounds`, after the call sounds (Q1), and approved the previous round's five decisions and this round's (pairs and the radios, Q12, Q13, Q14).
- **Rule 2:** every iced API used is already used in `crates/app`: `Theme::custom`, `pick_list` through `kit::menu`, `button`, `text`. `tokio::sync::oneshot` is already used by `blocking.rs`. `directories::ProjectDirs::config_dir` was checked in `directories-6.0.0` (cited above). Anything else is checked in the registry source of the pinned version and cited file:line in PROGRESS.md.
- **Rule 3:** no pinned version changes, and no new crate, font or other asset except the copied `tokens.json`, the neutral `Twicecat-copy.json` and the theme fixtures. The only manifest change of the whole plan is `serde_json = "1"` in `crates/app/Cargo.toml` (thm.2, Q4). No `libc`.
- **Rule 7:** the app still talks to the core only through `twicecat_core::api` and `twicecat_core::paths`; `ThemeName` lives in `api/rooms.rs`; `scripts/check-app-deps.sh` and `scripts/check-core-api.sh` pass after every task.
- **Rule 5:**
  - one commit per task on `feat/call-sounds`, type `feat` (with `docs` for plan, ledger and evidence commits), scope `app` or `core`, trailer `Step: thm.N`;
  - review fixes and Task 2b are folded in with `git commit --fixup=<sha>` and `git rebase --autosquash <base>`, only within the `thm.*` commits; `snd.*` commits are never touched; `git push --force-with-lease` only if the branch has been pushed;
  - every commit updates `PROGRESS.md` ("Custom theme steps", "Custom theme human checkpoint") and the ledger.
- **The gate before each commit** (and on each rewritten commit in Task 2b):
  - `cargo fmt --all`;
  - `cargo clippy --workspace --all-targets -- -D warnings`, and again with `--all-features`;
  - `cargo test --workspace`;
  - `cargo check --workspace --lib --bins --locked`;
  - `scripts/check-app-deps.sh`, `scripts/check-core-api.sh`, `bash scripts/check-font-assets.sh` (unchanged, must still pass), `bash scripts/check-no-webrtc-download.sh` and `scripts/check-commits.sh main`;
  - `cargo deny check`.
- **Lessons carried over from the UI rework audits:** V1–V5 enforce readable text and visible warnings, and the kit's readability tests run over every accepted test theme; every new menu goes through `kit::menu` inside `ui::guard::no_wheel` (`menus_ignore_the_wheel.rs`); trust and warning colours are tested by a distance, not only `assert_ne!`; the ledger says honestly which tests were red first, and each mutation is named and reverted; the gallery's PNGs are an aid, not proof.
- **Rules 6 and 9:** fixtures contain colours and made-up names only; nothing is logged; how a theme looks in a real window, and opening the folder on each OS, are human checkpoint items.

### Task 1: The built-in Twicecat colours replace the four families
Shipped as the `Step: thm.1` commit (evidence `docs/plans/custom-theme.evidence/task-1.md`). The text below is what was built and proven. Task 2b amends the commit: the token fields are renamed, the "dark look only" note goes (the radios, kept in the code, simply do not appear for a single-look theme; `both_modes` stays), and the docs lose the dark wording; the criteria marked (2b) are replaced there.
Language: rust
Files:
- `crates/app/src/theme.rs`: `Tokens`, `BUILT_IN`, the mapping table, `Resolved` and `built_in()` (named "Twicecat"), `semantic()`; the module docs with the two Q9 departures; `Family` and the eight sets removed; contrast tables for the built-in theme; `only_theme_rs_builds_themes`.
- `crates/app/src/state.rs`: `current_theme()` returns `theme::built_in()`; tests rewritten.
- `crates/app/src/settings.rs`: `SettingsMessage::Palette` removed.
- `crates/app/src/screens/settings.rs`: swatches and "Use the default" removed; the "Theme: Twicecat" line; (2b) the radios hidden with the note.
- `crates/app/src/ui/kit.rs` (also `checkbox_style`/`text_input_style`), `rich_text.rs`, `screens/call.rs`, `screens/timeline_pane.rs` tests, `crates/app/tests/timeline_layout.rs`, `crates/app/src/source_scan.rs` (test only).
- `crates/app/tests/ui_gallery.rs`: `THEMES`, swatch tests replaced, one index column per theme.
- `docs/accessibility.md` "Theme and contrast"; `PROGRESS.md`; `docs/plans/custom-theme.progress.md`.
Depends on: none
Acceptance criteria:
- A test checks `BUILT_IN` against the 15 hex values of the design table, and `danger_soft == #3b2136`.
- Built-in contrast tests: every V3 text pair ≥ 4.5:1, every non-text pair ≥ 3:1; V4 pairs ≥ ΔE 20, V5 pairs ≥ ΔE 8 and 5; `readable_on` keeps sender colours at 4.5:1 on every surface.
- The kit tests run over `theme::built_in()` and pass; severity colours are asserted ΔE ≥ 20 apart.
- `the_hover_background_differs_from_the_others` holds; `background ≠ surface ≠ surface_raised`, `sidebar ≠ surface`, `sidebar == background` documented.
- `grep -rnw "Family\|PaletteChoice\|DEFAULT_FAMILY\|Frost\|Ember\|HighContrast" crates/app/src crates/app/tests` finds no code use; a settings file with `"palette":"Ember"` shows Twicecat.
- (2b) The Appearance page shows "Theme: Twicecat", "This theme has a dark look only." and no radio.
- A mutation that puts a second `Theme::custom` in `state.rs` fails `only_theme_rs_builds_themes` (recorded and reverted).
- `ui/tokens.rs`, `ui/fonts.rs` and the font assets are unchanged.
- `cargo test -p twicecat-app --test ui_gallery` passes.
- The gate passes. The commit is `feat(app): one built-in Twicecat theme from the design colours` with `Step: thm.1`.
Evidence: `TWICECAT_GALLERY_DIR=target/ui-gallery TWICECAT_GALLERY_ONLY=settings-appearance cargo test -p twicecat-app --test ui_gallery -- --ignored` writes `settings-appearance-twicecat-comfortable.png`; `cargo run -p twicecat-app -- --ephemeral` opens in the Twicecat colours.

### Task 2: Theme files are parsed and validated (no UI yet)
Shipped as the `Step: thm.2` commit (evidence `docs/plans/custom-theme.evidence/task-2.md`). The text below is what was built and proven. Task 2b amends the commit: neutral token names with legacy aliases, Daylight, `Twicecat-copy.json`, fixture keys renamed, and the `load_dir` doc line. The light/dark pairing and the duplicate check by name and mode stay as built; the criteria marked (2b) are extended there.
Language: rust
Files:
- `crates/app/Cargo.toml`: `serde_json = "1"`.
- `crates/app/src/themes.rs` (new): `parse`, `load_dir`, `Loaded`, `ReportLine`, the limits.
- `crates/app/src/theme.rs`: `resolve()` with V1–V5, `readable_over`, CIE76 ΔE, the registry, `TOKEN_NAMES`, `test_themes()`.
- `crates/app/src/lib.rs`: `pub mod themes;`.
- `crates/app/assets/themes/twicecat.tokens.json` (the design file, unchanged) and `SOURCE.md`.
- `crates/app/tests/fixtures/themes/`: (2b) `Paper.light.json`, `Duo.light.json`, `Duo.dark.json`, `Faint.dark.json`, `hostile/*.json`.
- `PROGRESS.md` and the ledger.
Depends on: Task 1
Acceptance criteria:
- `parse` of `assets/themes/twicecat.tokens.json` gives exactly `BUILT_IN`, and `resolve` makes no adjustment; its SHA-256 is `0388d3cf…cf2b`.
- (2b) Paper and Duo resolve with no adjustment; Faint resolves with `ink` adjusted; every kit readability test passes over `test_themes()`.
- The hostile corpus gives the stated results, never panics, finishes in under 1 s, and its report lines contain no file values; (2b) `Nord.json` next to `Nord.dark.json` keeps the first by file name.
- After any accepted resolve, every V3 pair meets its least contrast, and V4 and V5 hold (2,000 seeded random token sets).
- Mutations recorded and reverted: removing V4, removing the `symlink_metadata` check, lowering V3 to 3:1.
- `only_theme_rs_builds_themes` still passes; `scripts/check-app-deps.sh` and `cargo check --workspace --lib --bins --locked` pass.
- The gate passes. The commit is `feat(app): read custom theme files safely and keep them readable` with `Step: thm.2`.
Evidence: `cargo test -p twicecat-app --lib themes::` passes, including the `hostile_` tests. Nothing on screen changes in this task.

### Task 2b: Neutral token names, no dark wording, no note for single-look themes, Daylight (folded into thm.1 and thm.2)
Language: rust
Files:
- **Folded into the `thm.1` commit** (`fixup! feat(app): one built-in Twicecat theme from the design colours`):
  - `crates/app/src/theme.rs`: `Tokens` fields `violet_wash`→`accent_wash`, `violet`→`accent`, `on_violet`→`on_accent`, `violet_bright`→`accent_bright` (with their doc comments), `BUILT_IN`, `map_tokens`, the shield and kit references; `Resolved.both_modes` **kept** (its doc comment already says a single-mode theme is shown in its own mode whatever the choice is); module docs rewritten: the mapping table with neutral names, the Q9 departures with `accent-bright`, and the mode rule of 1b (a single-look theme, Twicecat included, is shown in its own mode; a pair follows the System/Light/Dark choice) in place of "dark whatever the system or the stored `ThemeChoice` says"; no "dark" wording for the built-in theme.
  - `crates/app/src/ui/kit.rs`: the comment that names `violet`.
  - `crates/app/src/screens/settings.rs`: `appearance_page` shows a row with the label "Theme" and the value "Twicecat" (from `BUILT_IN_NAME`). The radio closure, the three radios and the `if resolved.both_modes` branch stay; only the `else` branch with the note "This theme has a dark look only." (and its comment) is removed, so a single-look theme shows nothing there.
  - `crates/app/src/settings.rs`: unchanged (`SettingsMessage::Theme(ThemeChoice)` and `ALL_NAMES: [&str; 24]` stay; the radios of a paired theme send it).
  - `crates/app/src/state.rs`: `current_theme()` doc without the dark wording ("the built-in Twicecat theme, which has one look; the OS mode and the stored `ThemeChoice` apply only to a paired theme"); the `SettingsMessage::Theme` tests (:11304-11311, :11401-11424) keep their behaviour (the choice is saved) and lose any dark wording; one test asserts that for every `ThemeChoice` × `Message::ThemeMode(Light/Dark/None)`, `current_theme()` stays Twicecat; the "system mode is still tracked" test (:3069) kept.
  - `crates/app/tests/ui_gallery.rs`: `THEMES` keeps its shape; the appearance test renamed `the_appearance_page_names_the_theme_and_has_no_swatches_or_mode_radios` and asserting "Theme" and "Twicecat", no radio, and no shown text containing "dark", for every stored `ThemeChoice`; comments at :37 and :3501 reworded (no "a dark look only").
  - `docs/accessibility.md` "Theme and contrast": neutral token names; no "dark look only"; light themes and light/dark pairs come as files (Daylight in Task 4); High contrast is gone and custom themes replace it.
  - `PROGRESS.md` "Custom theme steps" thm.1 section: neutral names, "radios shown only for a paired theme" in place of "radios hidden because `both_modes` is false … dark look only", the reworded "Deviation from ROADMAP.md:99" (1b); the "Custom theme human checkpoint" item about Light/High contrast users reworded to "users who had picked Light or High contrast now see Twicecat until they add a theme file such as Daylight".
- **Folded into the `thm.2` commit** (`fixup! feat(app): read custom theme files safely and keep them readable`):
  - `crates/app/src/theme.rs`: `TOKEN_NAMES` with `accent-wash`, `accent`, `on-accent`, `accent-bright`; `Tokens::set` matches the neutral names only; `make_readable`, `status_alike`, `selection_hidden`, `verify`, `resolve` docs and comments with neutral names; `test_themes()` = Twicecat, Daylight, Paper, Duo (both looks) and Faint (adjusted).
  - `crates/app/src/themes.rs`:
    - `LEGACY_ALIASES: [(&str, &str); 4]` (alias → neutral name) and its use in `parse`: an alias sets its token only when the neutral key is absent; otherwise it is counted in a new `ParseNotes.aliases_ignored`;
    - a new `Note::AliasIgnored(usize)` with the fixed text of §3, emitted by `load_dir`;
    - **pairing, `both_modes` and the duplicate check by name and mode are not touched** (`FileProblem::Duplicate` keeps "same name and mode as another file"); the `.light`/`.dark` suffix check is kept;
    - module and `load_dir` docs: neutral names, the pair rule of 1b stated once, wrapped to at most 100 columns (the over-long line `/// at, 32 \`*.json\` files with allowed names taken in byte order of their names, 64 KiB each, and`);
    - tests: alias cases, the pair and same-name-same-mode cases, the Daylight and starter cases below (the existing Duo and Nord tests keep their assertions, with renamed keys).
  - `crates/app/assets/themes/Twicecat-copy.json` (new): the 15 tokens in `TOKEN_NAMES` order with neutral keys and the built-in values, `"$type": "color"`, nothing else. `SOURCE.md` gains one paragraph: this file is the built-in colours under the neutral names, written for this repository (not copied from the design folder), the starting file for a custom theme. `twicecat.tokens.json` is not touched.
  - `crates/app/tests/fixtures/themes/`:
    - `Daylight.light.json` (new, neutral keys, values per §4, confirmed by `resolve`);
    - `Paper.light.json`, `Faint.dark.json`, `Duo.light.json` and `Duo.dark.json`: keys renamed to the neutral names, file names and values unchanged (Duo stays the pair "Duo");
    - `hostile/*.json`: keys renamed, plus `hostile/alias_and_neutral.json` (both `accent` and `violet` present, with different values; both `on-accent` and `on-violet`).
  - `PROGRESS.md` "Custom theme steps" thm.2 section: neutral names, the alias rule, pairs as in 1b (unchanged code, reworded text), Daylight, `Twicecat-copy.json`, the fixture key renames.
- **New docs commits** (not folded): the ledger (`docs/plans/custom-theme.progress.md`: "Task 2b: started/complete", old → new `thm.1`/`thm.2` hashes) and `docs/plans/custom-theme.evidence/task-2b.md`. The earlier evidence files are records of what was proven then and stay as they are.
- **How the history is rewritten:**
  - Preconditions: no other agent is rewriting the branch, and `git status --porcelain` is empty (the four modified call-sounds files at the time of this revision are committed or set aside by their owner first; stop and ask if not). Record `git rev-parse HEAD` and the `snd.*` hashes in the ledger.
  - In a scratch worktree at `thm.1` (`git worktree add <scratch>/wt <thm.1>`): make the thm.1 part, run the gate, `git commit --fixup=<thm.1>`; cherry-pick the commits after `thm.1` up to `thm.2`, resolving conflicts toward the new names; make the thm.2 part, run the gate, `git commit --fixup=<thm.2>`; cherry-pick the remaining docs commits; `git rebase --autosquash <thm.1>^` (with `GIT_SEQUENCE_EDITOR=:` if the installed git needs `-i` for `--autosquash`).
  - Check with `git rebase --exec '<gate>' <thm.1>^` (or the gate run by hand at each of the two rewritten commits), then move `feat/call-sounds` to the new tip in the main worktree (`git reset --keep <new tip>` with a clean tree) and remove the scratch worktree.
Depends on: Task 2
Acceptance criteria:
- History: `git log --format='%s%n%b' main..HEAD` still has exactly one `Step: thm.1` and one `Step: thm.2` commit, with their original subjects, and no `fixup!` commit. Every commit before `thm.1` keeps its hash (`git rev-parse <thm.1>^` equals the recorded value; the `snd.*` hashes are unchanged). `git range-diff` between the old and new ranges shows changes only in `thm.1`, `thm.2` and the docs commits that cherry-picking moved.
- At each rewritten `thm.1` and `thm.2` commit, the gate passes (the `--exec` log or the per-commit runs are in the ledger).
- Neutral names: `TOKEN_NAMES == ["void","surface","surface-raised","surface-overlay","accent-wash","line-strong","ink","ink-muted","accent","on-accent","accent-bright","focus","live","ok","warn"]` (a test). `git grep -n -i violet -- crates/app/src crates/app/tests docs/accessibility.md ':!crates/app/tests/fixtures/themes/hostile/alias_and_neutral.json'` matches only the `LEGACY_ALIASES` block and the known-unused `violet-deep` entry in `themes.rs`, and the alias tests in `themes.rs` (the ledger lists every match).
- No dark wording: `git grep -n -i -E 'dark look only|dark-only|dark only|the dark theme|dark violet' -- crates/app docs/accessibility.md` finds nothing, nor does the "Custom theme" part of `PROGRESS.md` (plain "dark look" stays allowed: it describes one side of a pair).
- Aliases:
  - `parse` of `assets/themes/twicecat.tokens.json` (legacy names only) still gives exactly `BUILT_IN` with `ParseNotes::default()`, and `sha256sum` of the file is still `0388d3cf…cf2b` (`git diff --exit-code <old thm.2> -- crates/app/assets/themes/twicecat.tokens.json`);
  - `parse` of `assets/themes/Twicecat-copy.json` gives exactly `BUILT_IN` with `ParseNotes::default()`; a test checks its keys are exactly `TOKEN_NAMES` in order and that it contains no `violet`;
  - `hostile/alias_and_neutral.json`: the neutral values win, `aliases_ignored == 2`, and the report line reads "alias_and_neutral: 2 older colour names ignored, their new names are set" (fixed text; no colour value in it);
  - a neutral key with an invalid value next to a valid alias: the token takes the built-in value, `not_understood == 1`, `aliases_ignored == 1` (presence decides);
  - an alias alone with an invalid value falls back and counts as not understood, like any token.
- Pairs and duplicates (thm.2 behaviour kept; tests generated at test time):
  - `git grep -n both_modes crates/app/src` still finds `Resolved.both_modes`, its use in `load_dir` and in `appearance_page`;
  - `Nord.dark.json` (dark) + `Nord.light.json` (light) load as one theme "Nord": two `Resolved` entries named "Nord", one with `dark == true` and one with `dark == false`, both with `both_modes == true`, and no report line;
  - `Nord.dark.json` (dark) + `Nord.json` (dark): one theme "Nord" from `Nord.dark.json` (it sorts first), `both_modes == false`, and the one line "Nord: same name and mode as another file" (the existing test, keys renamed);
  - `Nord.dark.json` + `Nord.json` (dark) + `Nord.light.json`: "Nord" is a pair (`Nord.dark.json` and `Nord.light.json`), plus the one duplicate line for `Nord.json`;
  - a lone `Nord.dark.json`, a lone `Nord.json` and `Daylight.light.json` each load with `both_modes == false`;
  - `Nord.dark.json` + a `Nord.light.json` that is refused (status colours alike): the dark look loads alone with `both_modes == false`, and the light file has its refusal line;
  - the Duo fixtures load as one pair "Duo", both looks with `both_modes == true` (the existing test at `themes.rs:674`, keys renamed);
  - `X.light.json` with a dark `void` and `X.dark.json` with a light `void` are still refused with the "says light/dark" reasons.
- Daylight:
  - `load_dir` of a folder holding only `Daylight.light.json` gives one theme named "Daylight", `dark == false`, `both_modes == false`, an empty `report` (no adjustment) and no report lines; a test pins its 15 values;
  - the ledger records whether the draft's `surface-overlay #dfe3ea` was refused by `resolve` (expected: `SelectionHidden`) and which value ships;
  - every kit, `rich_text` and `call` readability test passes over the new `test_themes()` (Twicecat, Daylight, Paper, Duo light, Duo dark, Faint).
- `grep -nE '^.{101,}' crates/app/src/themes.rs crates/app/src/theme.rs` prints nothing.
- The Appearance page (gallery test): the "Theme" row with "Twicecat", no radio, no swatch, no note, no text containing "dark", for every stored `ThemeChoice`. `SettingsMessage::Theme` is still there and `ALL_NAMES.len() == 24`.
- State: for each `ThemeChoice` and each `Message::ThemeMode(Light/Dark/None)`, `current_theme().palette()` equals Twicecat's (the built-in theme has one look).
- Mutations, each recorded and reverted: making the alias win over the neutral key fails the `alias_and_neutral` test; dropping the mode part of the duplicate check (name only) fails the `Nord` pair test; forcing `both_modes = false` after pairing fails the `Nord` pair and Duo tests; setting Daylight's `surface-overlay` back to `#dfe3ea` fails the Daylight test; putting the note back in the `else` branch fails the gallery appearance test.
- After the rewrite: security re-audit (aliases, pairs and duplicates), review and evidence are re-run for Tasks 1, 2 and 2b; the ledger records each verdict.
Evidence: `cargo test -p twicecat-app --lib themes:: theme::` passes, including the alias, `Nord` pair/duplicate and Daylight tests; `sha256sum crates/app/assets/themes/twicecat.tokens.json` prints `0388d3cf…cf2b`; `TWICECAT_GALLERY_DIR=target/ui-gallery TWICECAT_GALLERY_ONLY=settings-appearance cargo test -p twicecat-app --test ui_gallery -- --ignored` writes `settings-appearance-twicecat-comfortable.png` showing "Theme" / "Twicecat" with no radios and no note; `git log --oneline main..HEAD` shows one `thm.1` and one `thm.2` commit and no `fixup!`.

### Task 3: A theme name and a themes folder in the core
Language: rust
Files:
- `crates/core/src/api/rooms.rs`: `ThemeName` (`Copy`, `TryFrom<&str>`, `as_str`, the rules and reserved names, rustdoc with doctests); `UiSettings.theme_name: Option<ThemeName>` replaces `palette`; `PaletteChoice` removed. `ThemeChoice` and `UiSettings.theme` are not touched (1b).
- `crates/core/src/api/mod.rs`: re-export `ThemeName` instead of `PaletteChoice`.
- `crates/core/src/paths.rs`:
  - `settings_from_map` reads `theme_name` through `ThemeName::try_from` and ignores `palette`;
  - `write_settings` writes `theme_name` only when `Some`, and never writes `palette`; `theme` is written as before;
  - the `themes` field, set by `from_os()` from `ProjectDirs::config_dir()` and by `at(root)` to `root/themes`;
  - `themes_dir()` and `create_themes_dir()`;
  - tests.
- `crates/app/src/themes.rs`: `name_allowed` delegates to `ThemeName::try_from(..).is_ok()`, so the rules live in one place.
- `crates/app/src/settings.rs` and `state.rs`: compile against the new field. The app still shows Twicecat; the name is used in Task 4.
- `crates/app/tests/ui_gallery.rs`: the settings fields.
- `PROGRESS.md` (the breaking core API change, approved in Q3) and the ledger.
Depends on: Task 2b
Acceptance criteria:
- Round trips: `None` and `Some("Daylight")` × density × room list × text scale × every `ThemeChoice` round-trip through `write_settings`/`read_settings`. `None` writes no `theme_name` key; the stored `theme` value is kept.
- Legacy files:
  - `{"palette":"Frost"}`, `"Ember"`, `"HighContrast"`, `"Twicecat"` and `"Bogus"` read as `theme_name: None`;
  - the file is not renamed `.bad`;
  - after one `write_settings` the file has no `palette` key;
  - a file with both `palette` and a valid `theme_name` keeps the `theme_name`.
- Bad names read as `None`, with the rest of the file kept: `{"theme_name":"../x"}`, `"a/b"`, `"C:\\x"`, `""`, `" lead"`, 33 bytes, `"\u202e"`, `"TwiceCat"`, `"DEFAULT"`, `"Day.light"`; `{"theme_name":5}` and `{"theme_name":null}`.
- `ThemeName` doctests: valid and invalid examples; `UiSettings: Copy`.
- `themes::name_allowed` and `ThemeName::try_from` agree on every name in the two lists above and on `"Daylight"`, `"Nord Dark"`, `"a_b-c 1"` (a test).
- The themes folder:
  - `DataPaths::at(root).themes_dir()` is `root/themes`;
  - `DataPaths::from_os().themes_dir()` ends with `themes`, and its parent equals `ProjectDirs::from("io.github", "ga_lep", "Twicecat").config_dir()`;
  - on Linux, with `XDG_CONFIG_HOME` set to a temporary absolute path, it is `<tmp>/twicecat/themes`, tested in a child process (the test binary re-run with an env marker), or `#[ignore]` with a reason if no such helper exists; the ledger says which.
- `create_themes_dir()` makes the folder with mode 0o700 on Unix. `the_data_root_holds_only_the_marker_and_the_sessions_dir` still passes.
- The settings `Debug` output carries no secret: the `api` test at `api/mod.rs:1848` still passes.
- `scripts/check-core-api.sh` and `scripts/check-app-deps.sh` pass.
- The gate passes. The commit is `feat(core): keep a theme name and a themes folder in the local settings` with `Step: thm.3`.
Evidence: `cargo test -p twicecat-core --lib paths::` passes. With a scratch `--data-dir` holding `{"palette":"Frost","theme":"Light"}`, saving any setting in the app (for example the text size) leaves a `settings.json` without `palette` and with `"theme":"Light"` (the commands are in the ledger).

### Task 4: Choosing a theme on the Appearance page, loading off the UI thread, and docs/themes.md
Language: rust
Files:
- `crates/app/src/main.rs`: passes `data.themes_dir()` to `App::booter`; no theme file is read in `main`.
- `crates/app/src/theme.rs`: `pick_look` (1b) with unit tests.
- `crates/app/src/themes.rs`:
  - `pub async fn load_off_thread(dir: PathBuf) -> Option<Loaded>`: `load_dir` on a thread of its own named `twicecat-themes` (`std::thread::Builder`), the result through `tokio::sync::oneshot`; `None` if the thread could not start or panicked. It does not use `blocking::run`;
  - a test-only variant taking the loader function, for the stall tests;
  - `read_theme_file` docs: the swap-to-FIFO race now stalls only the loader thread (no `libc`).
- `crates/app/src/state.rs`:
  - `App.themes_dir: PathBuf`, `App.themes: Option<Loaded>` (`None` = not read yet), `App.themes_loading: bool`;
  - the boot batch gains the first load (`Task::perform(themes::load_off_thread(dir), …)`), with `themes_loading = true`;
  - `current_theme()` resolves `ui_settings.theme_name` against `themes`, falling back to Twicecat. If the name has one look, that look is shown and `ui_settings.theme` and `theme_mode` are not read. Only if the name is a pair (`both_modes`) is the look chosen: `Light`/`Dark` force it; `System` takes the dark look when `theme_mode` is `Dark` and the light look otherwise (including `Mode::None`), as phase 2.8's `resolve_mode` did. The choosing is one small pure function in `theme.rs` (`pick_look(looks, choice, system) -> &Resolved`) so that it is unit-tested apart from `App`;
  - `selected_is_paired()` for the page: true only when the stored name resolves to a loaded pair;
  - `theme_fallback()` returns the fallback reason once the list is read;
  - `set_themes()` for tests.
- `crates/app/src/settings.rs`:
  - `SettingsMessage::{ThemePicked(Option<ThemeName>), ReloadThemes, ThemesLoaded(Option<Loaded>), OpenThemesFolder, ThemesFolderOpened(bool)}` with names and pages (`ALL_NAMES` updated); `SettingsMessage::Theme(ThemeChoice)` is kept as it is (saves `UiSettings.theme`);
  - `ReloadThemes` returns the load task only when no load is in flight;
  - the folder is created through `DataPaths::create_themes_dir` and opened through a new `dialogs::open_folder` (`open::that_detached`), inside `Task::perform`, so `state.rs` never names `open::`.
- `crates/app/src/dialogs.rs`: `open_folder`.
- `crates/app/src/screens/settings.rs`:
  - the "Theme" row: `kit::menu(pick_list(…))` in `ui::guard::no_wheel`, "Twicecat (default)" first, then the custom names as text, sorted, each name once (a pair is one entry);
  - under it, the existing "Follow the system" / "Light" / "Dark" radios, shown only when `selected_is_paired()`; nothing in their place otherwise;
  - **Reload themes** (disabled while loading, with "Reading the themes folder…") and **Open themes folder** buttons;
  - up to 5 report lines and the fallback line, through `kit::banner(Severity::Warning)` or `note`.
- `crates/app/tests/ui_gallery.rs`:
  - `THEMES` gains `"daylight"`, loaded through `themes::load_dir("tests/fixtures/themes")` (synchronously; tests may);
  - new scenarios: `settings-appearance-themes` (Daylight selected, no radios; the menu with Daylight, Duo, Faint and Paper; a report with one refused and one adjusted file), `settings-appearance-pair` (Duo selected with `ThemeChoice::System`: the three radios shown, System selected; drawn once with the OS mode Light and once Dark, showing Duo's light and dark looks), `settings-appearance-fallback` (the selected theme refused, no radios) and `settings-appearance-loading` (Reload disabled, "Reading the themes folder…").
- `docs/themes.md` (new):
  - where the folder is on each OS, and `--data-dir`/`--ephemeral`;
  - **Try the light example** first: the `Daylight.light.json` block in full, then exactly where to save it: `~/.config/twicecat/themes/Daylight.light.json` (Linux; `$XDG_CONFIG_HOME/twicecat/themes/` if set), `~/Library/Application Support/io.github.ga_lep.Twicecat/themes/Daylight.light.json` (macOS), `%APPDATA%\ga_lep\Twicecat\config\themes\Daylight.light.json` (Windows), or **Open themes folder** to get there; then **Reload themes** and pick "Daylight";
  - file names: `<Name>.json`, optional `.light`/`.dark` that must match the colours; a single file is a theme with one look, shown in its own mode whatever the system's light/dark setting; `<Name>.light.json` + `<Name>.dark.json` make one theme with both looks, and for it the page offers "Follow the system", "Light" and "Dark"; two files of the same name and mode: the first by file name is used, the other reported; how to build a pair from `Twicecat-copy.json` and Daylight (save them as `<Name>.dark.json` and `<Name>.light.json`);
  - the 15 tokens with their meaning and the built-in values;
  - "start from `crates/app/assets/themes/Twicecat-copy.json`" (copy it under a new name);
  - files that use the design's original token names still load; if a file has both, the new name wins;
  - the mapping table in words, including the two Q9 departures;
  - the rules V1–V5 and the limits;
  - what is ignored; how to reload.
  No sentence calls the built-in theme dark.
- `docs/accessibility.md`: custom themes, the V rules, Daylight as the light option, and a link to `docs/themes.md`.
- `PROGRESS.md`: a link to `docs/themes.md`; the checkpoint list.
- The ledger.
Depends on: Task 2b, Task 3
Acceptance criteria:
- State tests (with `set_themes` over the fixture folder):
  - picking "Daylight" applies it at once (`current_theme().palette().background == #ffffff`) and saves `theme_name: Some("Daylight")` once; an unchanged pick writes nothing;
  - picking "Twicecat (default)" saves `None`;
  - single-look themes ignore the choice and the OS mode: with Daylight picked, every stored `ThemeChoice` × every `Message::ThemeMode(Light/Dark/None)` leaves Daylight's light palette shown; with Faint picked (dark, one look), likewise Faint's dark palette; with no name (Twicecat), likewise Twicecat; `selected_is_paired()` is false for all three;
  - paired themes use them: with Duo picked, `ThemeChoice::Light` shows Duo's light look and `Dark` its dark look whatever the OS mode; `System` shows the dark look after `ThemeMode(Dark)` and the light look after `ThemeMode(Light)` and with `Mode::None`; a `ThemeMode` change while `System` is stored switches the look at once, and while `Light`/`Dark` is stored changes nothing; `selected_is_paired()` is true;
  - picking a radio while Duo is selected saves `theme` once (the existing `SettingsMessage::Theme` save path) and does not touch `theme_name`;
  - `pick_look` unit tests cover the same table (3 choices × 3 OS modes) for a pair, and return the only look for a single-look input whatever the arguments;
  - a stored name that is missing or refused shows Twicecat, keeps the stored name, and `theme_fallback()` gives "Theme "X" could not be used: <reason>. Showing Twicecat."; after a `ThemesLoaded` that has the theme, it is applied;
  - before the first `ThemesLoaded`, a stored "Daylight" shows Twicecat and `theme_fallback()` is `None`.
- Off the UI thread:
  - `ReloadThemes` with no load in flight returns a task and sets `themes_loading`; a second `ReloadThemes` while loading returns no task (checked as the existing state tests check tasks); `ThemesLoaded` clears the flag and replaces the list and report; while loading, `current_theme()` does not change;
  - `load_off_thread` with a test loader that blocks on a channel: the calling thread keeps running (a `recv_timeout` of 100 ms on the result shows it pending), and releasing the channel yields the result;
  - a source-scan test: `state.rs`, `settings.rs` and `screens/settings.rs` contain no `load_dir(` and no `blocking::run` call for themes (only `load_off_thread`).
- `OpenThemesFolder` creates the folder (0o700) and calls `dialogs::open_folder` inside `Task::perform`. The existing `state.rs` source scans (no `open::`, `that_detached`) still pass. In tests the launcher is faked, as `links` is.
- `docs/themes.md` and the fixtures stay in sync: a test reads the doc's ```json block and asserts it equals `tests/fixtures/themes/Daylight.light.json` byte for byte; a second test checks that the token list in the doc equals `theme::TOKEN_NAMES`; a third checks the doc names the three OS paths of §3 and contains no `violet` and no "dark look only"/"dark-only"; a fourth checks it mentions both `.light.json` and `.dark.json` and the "Follow the system" choice.
- `menus_ignore_the_wheel.rs` passes with the new menu, and a Ctrl + wheel over it changes nothing.
- The gallery's layout and overlap checks pass for `{twicecat, daylight} × density × rail` and the four new scenarios, at 1280 and 720 wide. Report lines wrap inside the card. A gallery test asserts the three radios are present for Duo and absent for Twicecat, Daylight, Faint and the fallback scenario.
- Mutations, each recorded and reverted:
  - letting `current_theme` use a refused theme fails the fallback test;
  - making `current_theme` ignore `ThemeChoice` for a pair (always the first look) fails the pair state test;
  - making `System` ignore the OS mode fails the pair state test;
  - showing the radios for every theme (dropping the `selected_is_paired()` check) fails the gallery radio test;
  - removing the `no_wheel` wrapper fails the wheel test;
  - calling `themes::load_dir` directly in the `ReloadThemes` handler fails the source scan;
  - removing the in-flight check fails the second-reload test.
- The gate passes. The commit is `feat(app): pick a custom theme from the themes folder` with `Step: thm.4`.
Evidence: `cargo run -p twicecat-app -- --data-dir <scratch>`, after saving `docs/themes.md`'s Daylight as `<scratch>/themes/Daylight.light.json`.
- Settings → Appearance → Theme lists "Twicecat (default)" and "Daylight"; picking Daylight turns the window light, and `<scratch>/settings.json` has `"theme_name":"Daylight"`.
- **Open themes folder** opens `<scratch>/themes`.
- Editing `void` to `#000000` and pressing **Reload themes** shows "Theme "Daylight" could not be used: backgrounds mix light and dark. Showing Twicecat."
- A pair: saving `crates/app/assets/themes/Twicecat-copy.json` as `<scratch>/themes/Nord.dark.json` and the Daylight block as `<scratch>/themes/Nord.light.json`, then **Reload themes**: "Nord" is listed once; picking it shows the three radios; "Light" turns the window light, "Dark" dark, and `settings.json` has `"theme_name":"Nord"` and the chosen `"theme"`. Picking Daylight again hides the radios.
- The gallery writes `settings-appearance-themes-*.png`, `settings-appearance-pair-*.png`, `settings-appearance-fallback-*.png` and `settings-appearance-loading-*.png`.

## Human checkpoint (added to PROGRESS.md "Custom theme human checkpoint" by the task named)
- (Task 1, reworded by 2b) The Twicecat colours in a real window on Linux (Wayland and X11), Windows and macOS: every screen, both densities and the rail.
  - Look at what the kit does not style and the validation cannot cover: radios, togglers, sliders, scrollbars and text selection, which iced derives from the palette.
  - The `accent-bright` primary buttons and mention badge (accepted, Q9).
  - Is the error banner's tint visible?
- (Task 2b) Users who had picked Light or High contrast now see Twicecat until they add a theme file. Confirm that Daylight, as described in `docs/themes.md`, is enough for them.
- (Task 4) Save Daylight into the themes folder (the per-OS paths in `docs/themes.md`), press Reload, pick it, and check it in a real window, including the iced-derived controls on a light background. Then break it on purpose and press Reload: the note appears, and the window falls back to Twicecat.
- (Task 4) At start-up with Daylight stored: is a brief Twicecat frame visible before Daylight applies (the first load runs off the UI thread)? Acceptable or not.
- (Task 4) **Open themes folder** opens the right folder in the file manager on Linux, Windows and macOS. On Linux, also with `XDG_CONFIG_HOME` set.
- (Task 4) Switching the OS between light and dark while the app runs: with a single-look theme (Twicecat, Daylight) nothing changes; with a paired theme and "Follow the system" the window switches look at once; with a paired theme and "Light" or "Dark" nothing changes. On Linux, check a desktop that reports no mode too (the pair then shows its light look).
- (Task 4) A paired theme: the three radios appear under the menu, and disappear when a single-look theme is picked.

What cannot be verified headlessly:
- real rendering (the gallery uses tiny-skia, not the window's wgpu);
- overlays such as menus and tooltips, which the gallery does not draw;
- iced's derived widget colours under a custom theme;
- the OS file manager and the real per-OS config paths;
- the real OS light/dark signal reaching the app (tests send `Message::ThemeMode` by hand);
- the start-up frame before the first load finishes.

## Questions
- **Q1 (resolved):** the work goes on the current branch `feat/call-sounds`, after the call-sounds work, with `Step: thm.N`. No new branch.
- **Q2 (resolved, revised in the last two rounds):** no "dark only for now". A theme has one look (light or dark, shown in its own mode) or is a `<Name>.light.json` + `<Name>.dark.json` pair, for which the System/Light/Dark radios apply (1b). The built-in Twicecat has one look. The ROADMAP line is not edited; the PROGRESS.md deviation line says light/dark pairs are supported again as theme files. Daylight ships as the light example.
- **Q3 (resolved):** a breaking core change from `palette` to `theme_name: Option<ThemeName>`, with `ThemeName` a `Copy` type. The old `palette` key is ignored and never written again.
- **Q4 (resolved):** `serde_json` is a direct dependency of `crates/app`, already in `Cargo.lock`, with no version change.
- **Q5 (resolved):** keep Inter. No font is replaced, added or hashed.
- **Q6 (resolved):** colours only. `ui/tokens.rs` sizes, spacing and radii do not change.
- **Q7 (resolved):** themes live in the OS config directory through `ProjectDirs::config_dir()`; under `--data-dir`/`--ephemeral` they are read from `<dir>/themes`.
- **Q8 (resolved):** the built-in theme is called "Twicecat". The Appearance list labels it "Twicecat (default)".
- **Q9 (resolved):** the primary button fill and the mention badge are `accent-bright`. These departures are documented.
- **Q10 (resolved):** none of the design repository's tooling is adopted. Only `tokens.json` is copied, byte for byte.
- **Q11 (resolved):** the README is handled separately. `docs/themes.md` is linked from `PROGRESS.md` and `docs/accessibility.md`.
- **Q12 (resolved):** on macOS, `directories` gives the same folder for config and data, so `themes/` sits next to `settings.json` there. Accepted.
- **Q13 (resolved: "if unused, delete"):** with pairs, `ThemeChoice`/`UiSettings.theme` and the app's OS light/dark tracking are used (they choose the look of a paired theme), so they stay. They are deleted only if a later review finds them unused.
- **Q14 (resolved):** user docs do not list the old `violet*` names; they say files using the design's original names still load. The names appear only in the alias table in `themes.rs`, in tests and in `PROGRESS.md`.
- **Q15 (resolved by user decisions, recorded):** the token keys are neutral (`accent*`), with the design's names accepted as aliases; the loader runs on its own thread instead of adding `libc`.
- **Q16 (open, small):** the duplicate line. Your note quoted "same name as another file"; thm.2 already says "same name and mode as another file" (`themes.rs:80`), and since a same-name file of the *other* mode is now a valid pair, the longer text says exactly why the file was skipped. The plan keeps thm.2's text (no code change). Recommended: keep it; say if you want the shorter text instead (a one-line change folded into thm.2 by Task 2b).

PLAN READY — awaiting validation
