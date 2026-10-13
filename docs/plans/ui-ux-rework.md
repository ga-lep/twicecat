# UI/UX Rework Plan
Status: APPROVED

## Goal
Rework the look and feel of the desktop app (`crates/app`, iced =0.14.0) on the branch `feat/ui-ux-rework`. The branch was fast-forwarded to `origin/main` at `90b93eb`, which includes the call UI and push-to-talk.

The rework has three parts:
- **A shared design system:**
  - spacing, size and type tokens;
  - one icon module;
  - a component kit, which replaces about 200 one-off `.style(...)` closures across 20 screen files;
  - a bundled typeface, Inter 4.1 (SIL OFL 1.1).
- **Selectable designs:**
  - four colour sets ("theme families": Twicecat, Frost, Ember, High contrast), each with a light and a dark mode;
  - two densities (Comfortable, Compact);
  - a room list that is either expanded or collapsed to a rail;
  - a text scale (zoom).

  The message layout stays as it is today: grouped by sender (answer Q8). It is restyled only through the tokens, the kit and density.
- **A new default look** (answer Q7). The default colour set becomes the one the human picks at the gallery review (Task 5 checkpoint). The plan recommends **Frost** (see "Default look"). Until then the default stays Twicecat. Applying the choice is a one-line change of `theme::DEFAULT_FAMILY` (Task 17).
- **A redesign of every screen that exists today:**
  - login, browser sign-in and loading;
  - the main window: header, spaces sidebar, room list, timeline, composer, thread panel and status bar;
  - **calls:** the call button and People toggle in the room header, the call bar (dock), the participants panel, the incoming-call card, and the room list's call pill and Voice tag;
  - the **Voice & audio** card: devices, push-to-talk, microphone processing with GPU status, levels, mic test, ringtone, other people's voices, announce, and diagnostics;
  - Settings and Sessions;
  - room dialogs, room settings, members, upload, media viewer and switcher;
  - security banners, recovery and verification.

Nothing in the core's behaviour changes. The only core change is additive: the new appearance choices join `UiSettings` (Task 3; answer Q1). Rendering cannot be checked headlessly, so every task's look goes on the human checkpoint. The headless gallery (Task 5) writes PNGs of every screen for review, as an aid and not as proof.

The work runs before Phase 6 is signed off and before Phase 7 (answer Q6, 2026-10-13: the human allowed it, as an exception to AGENTS.md rule 1).

### What exists today (surveyed at `90b93eb`)
- **`crates/app/src/theme.rs`:**
  - one family in light and dark (`LIGHT`, `DARK`, `Semantic`), with WCAG contrast tests;
  - `ThemeChoice {System, Light, Dark}` from `twicecat_core::api::UiSettings` (`crates/core/src/api/rooms.rs:353-391`), written by hand in `DataPaths::write_settings` (`crates/core/src/paths.rs:197`).
- **`crates/app/src/screens/` (20 files):**
  - `login`, `loading` and `main_view` (header, panes, status bar);
  - `spaces_sidebar` (64 px);
  - `room_list_pane`: 300 px, windowed rows of `room_list::ROW_HEIGHT` 64 px, the call pill (`calls::CALL_MARK`) and the `VOICE_TAG`, with the **call bar** (`call::bar`) last in its column;
  - `timeline_pane`: rows measured by `sensor`, with `timeline::estimate_height`, and grouped by sender through `is_continuation`;
  - `thread_panel`;
  - `call`: `ring_layer` (incoming-call card, `RING_CARD_WIDTH` 380), `bar` (room, phase line, declined line, push-to-talk talk line, Mute/Deafen/Audio…/Leave) and `panel_slot` (participants: speaking ring, mute mark, volume slider with −/+, per-person cleaning `pick_list`; `calls::PANEL_WIDTH` 320 px);
  - `audio_settings` (the Voice & audio card): sections devices, talking (mode, key capture, gamepad button), processing (noise suppression, GPU status from `audio_settings::gpu_status`), levels (meters), mic test, ringtone, others (cleaning), announce, diagnostics;
  - `settings`, which links to Voice & audio (`SettingsMessage::OpenVoiceAudio`, `VOICE_AUDIO_LINE`), and `sessions`, `security`, `verification`;
  - `room_dialogs` (has `modal`), `room_settings`, `members`, `upload_dialog`, `media_viewer` and `switcher`.
- **Glyphs are scattered as constants:** `LOCK_MARK` (room_list_pane), `CALL_MARK` and `MUTED_MARK` (calls.rs), and inline `"\u{2715}"`, `"\u{2212}"` and so on.
- **Fonts:** the code uses `Font::DEFAULT`, `Font::MONOSPACE`, and `BOLD`/`ITALIC` built from `Font::DEFAULT` (`timeline_pane.rs:184-191`, `rich_text.rs:301-305`). There is no bundled font; iced's `fira-sans` feature is off.
- **Layout invariants**, documented in `main_view.rs`, which keep focus and scroll when parts come and go. Every task keeps them:
  - the main row is always `[sidebar, list, room pane, thread, call panel]`;
  - the main column is always `[banner, panes, status]`;
  - the room column is always `[title, timeline, bars, editor]`;
  - the room-list column is always `[actions, tabs, search, list, call bar]`.
- **Window layers** (`state.rs:2575`), bottom to top: room dialogs, Settings, Voice & audio, viewer, upload, switcher, verification, ring card. Esc closes one layer per press in the order given in `keys.rs`.
- **`docs/accessibility.md`:**
  - the keyboard map, driven by `keys::SHORTCUTS` (now including Ctrl+Shift+M and Ctrl+Shift+D for calls);
  - the 4.5:1 contrast rule;
  - no screen reader, because iced 0.14 has no AccessKit.

### Rules for every task (AGENTS.md)
- **Rule 1:** the human made an exception for this branch (Q6, see Goal).
- **Rule 2:** every iced API used is checked in `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/iced*-0.14.*`. The ones this plan relies on were checked while planning (list below). Any further API is checked before use and cited file:line in PROGRESS.md.
- **Rule 3:**
  - no pinned version changes, no new crates, and no new iced features: `svg` (`iced-0.14.0/Cargo.toml:99`) and `fira-sans` (:74) stay off;
  - icons are Unicode glyphs behind one icon module (answer Q3);
  - the font is a file in the repository, loaded through iced's existing API (answer Q4), not a dependency.
- **Rule 7:** `crates/app` talks to the core only through `twicecat_core::api`, plus `twicecat_ptt`, which is already allowed. `scripts/check-app-deps.sh` and `scripts/check-core-api.sh` pass after every task.
- **Rule 5:**
  - one commit per task, type `feat` (`fix` only for a defect in code already on `main`), scope `app` or `core`, with a `Step: ui.N` trailer where N is the task number (answer Q5);
  - review fixes are folded in with `--fixup` and `rebase --autosquash`;
  - `PROGRESS.md` (a new "UI/UX rework steps" section) and the ledger `docs/plans/ui-ux-rework.progress.md` are updated in the same commit.
- **Gate before each commit:**
  - `cargo fmt --all`;
  - `cargo clippy --workspace --all-targets -- -D warnings`, and again with `--all-features` (which includes the app's `gpu` feature);
  - `cargo test --workspace`;
  - `scripts/check-app-deps.sh`, `scripts/check-core-api.sh`, `scripts/check-commits.sh main` and `cargo deny check`, plus `scripts/check-font-assets.sh` from Task 2 on.
- **Rules 6 and 9:**
  - fixtures use made-up names on `example.org`, and no tokens, keys or real message bodies;
  - `tests/no_secret_logging.rs` keeps passing;
  - the agent never runs `hardware_*` or `keyring` tests, and never opens a microphone;
  - how screens look, how they feel under the mouse, sound, push-to-talk in a real session, and OS dialogs are human checkpoint items.
- **Invariants every task keeps:**
  - the four slot layouts and the layer and Esc order above;
  - Tab order;
  - every text and background pair in every family reaches 4.5:1 (7:1 for High contrast);
  - the windowing of the room list and the timeline: `crates/app/tests/timeline_layout.rs` and the 10,000-room test in `room_list.rs` still pass;
  - the call tests in `crates/app/tests/calls.rs` and `crates/app/tests/ptt.rs` still pass unchanged;
  - the timeline's grouping and row structure (`is_continuation`, header and continuation rows) stays as it is today.

### iced 0.14 APIs checked for this plan (registry source, file:line)
- **Themes:**
  - `Theme::custom` `iced_core-0.14.0/src/theme.rs:91`, `Theme::custom_with_fn` :100, `Theme::extended_palette` :138;
  - `palette::Palette` `theme/palette.rs:9`, `Extended` :288, `Extended::generate` :395, `palette::mix` :675, `readable` :692, `is_dark` :723.
- **Application:**
  - `Application::scale_factor` `iced-0.14.0/src/application.rs:410`;
  - `Application::font(impl Into<Cow<'static, [u8]>>)` :245, which pushes to `settings.fonts`, loaded by the compositor at start (`iced_winit-0.14.1/src/lib.rs:139,602-603`);
  - `Application::default_font(Font)` :234;
  - `Font::with_name(&'static str)` `iced_core-0.14.0/src/font.rs:34` (`family: Family::Name`), with `Font { weight, style, .. }` fields :6-10; `Family::Name` maps to `cosmic_text::Family::Name` (`iced_graphics-0.14.0/src/text.rs:267-270`).
- **Headless font loading:** `iced::advanced::graphics` (`iced-0.14.0/src/advanced.rs:25` → `iced_renderer-0.14.0/src/lib.rs:9`) → `text::font_system()` `iced_graphics-0.14.0/src/text.rs:116` → `FontSystem::load_font(Cow<'static, [u8]>)` :150. Behind the dev-only `advanced` feature, which `crates/app` already enables under `[dev-dependencies]`.
- **Window:** `window::resize_events` `iced_runtime-0.14.0/src/window.rs:249`.
- **Widgets** (`iced_widget-0.14.2/src/helpers.rs`):
  - `responsive` :2100, `tooltip` :1110 (with `Tooltip::delay` `tooltip.rs:126`), `float` :2084, `pick_list` :1656, `toggler` :1400, `radio` :1351;
  - `stack` :561, `opaque` :577, `hover` :738, `mouse_area` :2010, `sensor` :1014;
  - `rule::horizontal`/`vertical` `rule.rs:30,42`;
  - `container::Style` (with `shadow`) `container.rs:462`, `rounded_box`/`bordered_box` :583/:595;
  - `button::Style` `button.rs:487`, `button::subtle` :713.
- **Headless drawing:** `renderer::Headless::screenshot` `iced_core-0.14.0/src/renderer.rs:139`. `iced_runtime::UserInterface` is not re-exported by `iced`, so the gallery draws with `Widget::layout`/`Widget::draw` (`iced_core-0.14.0/src/widget.rs:64`), as `crates/app/tests/timeline_layout.rs` already lays out.

### Default look (answer Q7)
- **The rule:** the default colour set is whichever family the human picks at the gallery review (Task 5 checkpoint). Twicecat is the default only until then.
- **Recommended candidate: Frost**, in both light and dark: cool neutral greys with a teal accent.
  - It differs clearly from today's blue-violet accent.
  - It does not look like Element (green) or Discord (blurple).
  - Neutral greys suit long sessions in dark mode and give the call colours room to stand out: the speaking ring, and the success/warning/danger phase dots.
  - It reaches 4.5:1 for every pair in the same tests as the other families.
- **How a "new default" reaches existing users:** `UiSettings.palette` is an `Option<PaletteChoice>` (Task 3).
  - `None` means "the app's default". It is what every existing `settings.json` reads as, and the field is not written while it is `None`.
  - The app resolves `None` to `theme::DEFAULT_FAMILY` (Task 4).
  - So changing that one constant (Task 17) moves everyone who never picked a family, and keeps the choice of anyone who did.
- **The other defaults are unchanged:** light/dark follows the system, Comfortable, Expanded, 100 %. Inter is the new typeface for everyone.

### The bundled typeface (answer Q4)
- **Font:** Inter 4.1 by The Inter Project Authors (Rasmus Andersson).
  - Source: https://github.com/rsms/inter, release `v4.1` (2024-11-16), asset `Inter-4.1.zip`, SHA-256 `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`. Downloaded and checked while planning.
- **Licence:** SIL Open Font License 1.1 (`LICENSE.txt` in the zip, SHA-256 `262481e844521b326f5ecd053e59b98c8b2da78c8ee1bdbb6e8174305e54935a`).
  - The copyright line declares **no Reserved Font Name**, so the name "Inter" may stay.
  - OFL 1.1 lets the font be bundled and embedded with any software, provided the font is not sold on its own and its licence goes with it. The font stays under OFL; it is not relicensed under the app's GPL-3.0.
- **Files kept:** four static TTFs from `extras/ttf/`, about 1.67 MB in total. This is an **assumption**: Q9 is unanswered, and this is the recommended option. The variable `InterVariable.ttf` is not used, because weight matching of a variable face through cosmic-text is not something this plan relies on.

| File | Size | SHA-256 |
|---|---|---|
| `Inter-Regular.ttf` | 411,640 | `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` |
| `Inter-Italic.ttf` | 417,388 | `bbc051dd204b5019a1aa0bc0ae2aa8a05ab13e7a3f979fa357631dc7feb6833a` |
| `Inter-SemiBold.ttf` | 419,744 | `78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3` |
| `Inter-Bold.ttf` | 420,428 | `288316099b1e0a47a4716d159098005eef7c0066921f34e3200393dbdb01947f` |

- **Where it lives:** `crates/app/assets/fonts/inter/` holds the four TTFs, `OFL.txt` (the zip's `LICENSE.txt`, unchanged) and `SOURCE.md` (URL, tag, the zip's SHA-256, and each file's SHA-256).
- **How it is loaded:**
  - `crates/app/src/ui/fonts.rs` holds `include_bytes!` constants;
  - `main.rs` calls `.font(REGULAR).font(ITALIC).font(SEMIBOLD).font(BOLD).default_font(ui::fonts::BODY)`, where `BODY = Font::with_name("Inter")`;
  - `BOLD`/`ITALIC` and the rich-text fonts are derived from `ui::fonts::BODY`, not `Font::DEFAULT`, so they resolve to Inter;
  - monospace stays the system's `Font::MONOSPACE`;
  - glyphs Inter lacks (emoji, CJK, the icon glyphs) fall back to system fonts through cosmic-text.
- **cargo-deny:** no change. Fonts are not crates, and cargo-deny checks crate licence metadata. `twicecat-app` is `publish = false`, and `deny.toml` has `[licenses.private] ignore = true`. `cargo deny check` is still run in the gate to confirm.
- **Attribution:**
  - a new root `THIRD-PARTY-NOTICES.md` names Inter, its copyright, the OFL 1.1, and the path of `OFL.txt`;
  - Settings → About shows "Inter typeface © The Inter Project Authors, SIL Open Font License 1.1" (Task 14);
  - a human item for Phase 7 packaging: every installer ships `OFL.txt` (`packaging/` has only the `.desktop` file today).
- **Binary size:** about 1.7 MB larger (recorded in PROGRESS.md before and after).
- **Bold italic:** `Inter-BoldItalic.ttf` is left out (Q9 assumption). Bold italic text falls back to Inter Bold or a system face.

### Task 1: Design tokens, icon module and component kit
Language: rust
Files:
- `crates/app/src/ui/mod.rs`, `crates/app/src/ui/tokens.rs`, `crates/app/src/ui/icons.rs` and `crates/app/src/ui/kit.rs` (new);
- `crates/app/src/lib.rs`;
- `crates/app/src/theme.rs` (`Semantic` gains `surface`, `surface_raised`, `border`, `focus`, `sidebar`, `danger_soft` and `speaking`, in both existing palettes);
- `crates/app/src/screens/room_dialogs.rs` (`modal`, `muted`, `person_avatar` and `PERSON_SLOT` move to the kit; their callers in `settings.rs`, `switcher.rs`, `upload_dialog.rs`, `room_settings.rs`, `call.rs` and `audio_settings.rs` are updated);
- `crates/app/src/calls.rs` and `crates/app/src/screens/room_list_pane.rs` (`CALL_MARK`, `MUTED_MARK` and `LOCK_MARK` become re-exports of `ui::icons`).

Depends on: none
Acceptance criteria:
- `ui::tokens` has:
  - spacing `XS 4, S 8, M 12, L 16, XL 24`;
  - radii `SMALL 4, MEDIUM 8, LARGE 12, ROUND`;
  - type sizes `CAPTION 12, BODY 14, BODY_LARGE 16, TITLE 18, HEADLINE 24, DISPLAY 32`;
  - `Density {Comfortable, Compact}` with `pad`, `gap`, `room_row_height` (64 / 44 px) and `timeline_row_pad` (Compact trims the vertical padding of timeline rows only; the row structure is unchanged).
- `ui::icons` is the **only** place glyphs are defined:
  - an `Icon` enum (Lock, Call, Speaker, Muted, Deafened, Mic, MicOff, Leave, Close, Plus, Minus, Search, Settings, People, Thread, Reply, React, Edit, Delete, Copy, Attach, Emoji, Send, Shield, Warning, Check, ChevronLeft, ChevronRight, More, Gpu, Keyboard, Gamepad, Dot);
  - `Icon::glyph()`, which returns the Unicode text today, and `Icon::label()`, the words for its tooltip;
  - `icon(Icon) -> Element`, the single place that turns an icon into a widget, so a later switch to SVG or an icon font changes one file.

  A test reads every `.rs` file under `crates/app/src/` except `ui/icons.rs` and `ui/fonts.rs`, and fails on a non-ASCII symbol from the ranges U+2190–U+2BFF or U+1F300–U+1FAFF outside a comment or test. The ellipsis U+2026 and middle dot U+00B7 used in words are allowed.
- `ui::kit` has:
  - `primary_button`, `secondary_button`, `ghost_button` and `danger_button`;
  - `icon_button(Icon, msg)`, which always wraps a `tooltip` with `Icon::label()`, and `toggle_icon_button(Icon, active, msg)` for mute/deafen-style toggles, with the active state shown by both colour and glyph;
  - `card`, `modal(content, width)`, `section(title, body)`, `tag`, `count_badge`, `banner(Severity, text)`, `empty_state(title, hint)`, `field`, `divider`, `avatar(...)` (moved `person_avatar`), `speaking_ring(avatar, speaking)` and `page_nav(pages, current, msg)` (left list for paged cards).
- Every style function is a pure `fn(&Theme, Status) -> Style`. A unit test checks, for both palettes and every `button::Status`, that text against background reaches 4.5:1 (3:1 for `Disabled`).
- The contrast table covers every new `Semantic` field. `surface`, `surface_raised` and `sidebar` differ from each other and from `background`. `speaking` reaches 3:1 against `background`, because it is a non-text ring (WCAG 1.4.11).
- The moved items build the same widget trees: no slot changes, and existing tests pass unchanged. The only visible change is `modal`'s card, on `surface_raised` with the `LARGE` radius.
- The commit carries `Step: ui.1`.

Evidence: none — internal kit; first visible in Task 4 (Settings) and Task 5 (gallery PNGs).

### Task 2: Bundle the Inter typeface
Language: rust
Files:
- `crates/app/assets/fonts/inter/{Inter-Regular.ttf, Inter-Italic.ttf, Inter-SemiBold.ttf, Inter-Bold.ttf, OFL.txt, SOURCE.md}` (new);
- `crates/app/src/ui/fonts.rs` (new), `scripts/check-font-assets.sh` (new) and `crates/app/src/main.rs`;
- `crates/app/src/screens/timeline_pane.rs` and `crates/app/src/rich_text.rs` (fonts derived from `ui::fonts::BODY`);
- `THIRD-PARTY-NOTICES.md` (new) and `PROGRESS.md` (pinned assets, binary size).

Depends on: Task 1
Acceptance criteria:
- The four TTFs and `OFL.txt` match the SHA-256 values in "The bundled typeface" above. `scripts/check-font-assets.sh` (new; `set -euo pipefail`, shellcheck-clean, downloads nothing) checks them with `sha256sum -c` against `SOURCE.md`, and runs in the gate. A unit test checks each `include_bytes!` length and the TrueType magic (`00 01 00 00`); no hash crate is added. `SOURCE.md` records the URL, tag, zip SHA-256 and file SHA-256s.
- `ui::fonts` has `REGULAR`, `ITALIC`, `SEMIBOLD` and `BOLD` (`&'static [u8]`), `BODY = Font::with_name("Inter")`, `BODY_SEMIBOLD`, `BODY_BOLD`, `BODY_ITALIC` (the same family with `Weight`/`Style` set), and `MONO = Font::MONOSPACE`.
- `main.rs` loads the four faces with `.font(...)` and sets `.default_font(ui::fonts::BODY)`. A unit test checks that `ui::fonts::ALL` lists exactly the four byte slices that `main.rs` loads (one constant used by both).
- After the task, no `Font::DEFAULT` is left in `crates/app/src` outside tests (grep test). `rich_text`'s bold, italic and code fonts keep their family test (`rich_text.rs:927`), now against `ui::fonts`.
- A headless test (tiny-skia) loads the four faces through `iced::advanced::graphics::text::font_system().write()…load_font(...)`. It then checks that a `text("Twicecat")` in `ui::fonts::BODY` and one in `BODY_BOLD` lay out to different widths, so the bold face is really found, and that both differ from the system `Font::DEFAULT`.
- `THIRD-PARTY-NOTICES.md` names Inter, "Copyright (c) 2016 The Inter Project Authors", SIL OFL 1.1 and the path of `OFL.txt`. Settings → About gets its line in Task 14; until then the notices file covers it.
- `cargo deny check` passes with `deny.toml` unchanged. PROGRESS.md records the release binary size of `twicecat` before and after (`cargo build --release -p twicecat-app`) and the Q9 assumption (four static faces).
- The commit carries `Step: ui.2`.

Evidence: `scripts/check-font-assets.sh` prints OK for five files. `cargo test -p twicecat-app fonts` passes the byte and face-resolution tests. `cargo run -p twicecat-app -- --ephemeral --homeserver http://localhost:8008` shows the login in Inter (human checkpoint).

### Task 3: Store the appearance choices in the local settings (core)
Language: rust
Files:
- `crates/core/src/api/rooms.rs`: adds `PaletteChoice {Twicecat, Frost, Ember, HighContrast}`, `DensityChoice {Comfortable, Compact}`, `RoomListChoice {Expanded, Rail}` and `UiSettings::{palette: Option<PaletteChoice>, density, room_list, text_scale_percent}`. There is no message-layout setting (answer Q8);
- `crates/core/src/api/mod.rs` (re-exports);
- `crates/core/src/paths.rs` (`write_settings` writes the new fields, and omits `palette` while it is `None`; `read_settings` is lenient per field);
- `crates/app/src/settings.rs` (compiles against the new fields; no UI yet).

Depends on: none
Acceptance criteria:
- The defaults are `palette: None` ("the app's default"), Comfortable, Expanded and 100, so an old `settings.json` gives no visible change from this task. A doc test shows this.
- A file without the new fields reads as before, and the existing `paths.rs` tests pass unchanged. Writing settings with `palette: None` leaves no `palette` key in the file (test), so a later change of the app's default reaches that user.
- A field with an unknown value takes that field's default and keeps all other fields (`an_unknown_appearance_value_falls_back_per_field`); an unknown palette reads as `None`. The file is not renamed `.bad`.
- `text_scale_percent` is clamped to 80–200 and rounded to a step of 10 when read. A test covers 0, 79, 85, 200, 250 and `u16::MAX`.
- Writing and reading round-trips every value of every new enum, and `None`. The file stays 0o600 on Unix.
- `scripts/check-core-api.sh` passes, and `UiSettings` stays `Copy + Eq`. PROGRESS.md records the additive API as approved (answer Q1, 2026-10-13).
- The commit is `feat(core): …` with `Step: ui.3`.

Evidence: `cargo test -p twicecat-core paths::` passes the new tests. A test reads a `settings.json` with `"palette":"Nope","theme":"Dark"` and shows `theme == Dark`, `palette == None`.

### Task 4: Theme families, the Appearance settings and zoom
Language: rust
Files:
- `crates/app/src/theme.rs` (`Family`, `DEFAULT_FAMILY`);
- `crates/app/src/state.rs` (`current_theme`, `App::scale_factor`);
- `crates/app/src/main.rs` (`.scale_factor(App::scale_factor)`);
- `crates/app/src/settings.rs` and `crates/app/src/screens/settings.rs` (Appearance section);
- `crates/app/src/keys.rs` (Ctrl+= / Ctrl+- / Ctrl+0);
- `docs/accessibility.md`.

Depends on: Task 1, Task 3
Acceptance criteria:
- There are four families × light/dark = eight `Palette`s and eight `Semantic`s:
  - Twicecat: today's colours, refined;
  - Frost: cool greys with a teal accent (the recommended new default);
  - Ember: warm neutrals with an orange accent;
  - High contrast: black and white with saturated accents.

  Every family passes the contrast table at 4.5:1, and High contrast at 7:1. `readable_on` keeps sender colours readable on all eight backgrounds, and `speaking` reaches 3:1 on all eight.
- `pub const DEFAULT_FAMILY: Family = Family::Twicecat;` is the single place the default is decided. `palette: None` resolves to it, and a test checks that changing it is the only edit needed: no other code names a default family (grep test on `Family::Twicecat` outside `theme.rs` and tests).
- `theme_for_choice(system, choice, family)` combines family and mode, and System still follows `system::theme_changes()`. A test covers names and `is_dark` for all 24 combinations (four families × three choices × two system modes).
- The Appearance section has:
  - the mode radios;
  - four family swatch buttons (accent, surface and text samples, with the selected one outlined). The default family's swatch says "(default)". "Use the default" sets `palette: None`;
  - `pick_list`s for Density and Room list;
  - text scale as −, a percentage and +, plus Reset.

  Each change goes through `SettingsMessage`, applies at once and is saved by the existing `SettingsStore`. Update tests cover each message, including going back to `None`.
- A Density switch clears the timeline's measured heights (a new `TimelineView::reset_measurements`) and keeps `offset_from_bottom` (unit test). A resize with no density switch keeps them.
- `App::scale_factor` returns `text_scale_percent / 100`. Ctrl+= and Ctrl+- step by 10 within 80–200, and Ctrl+0 resets. They are in `SHORTCUTS` and do not collide with existing keys: `keys_are_listed_once`, `the_composer_keeps_its_own_keys` and the label tests pass. They are ignored while `audio_ui.capturing` is waiting for a push-to-talk key (test).
- `docs/accessibility.md` covers families, High contrast's 7:1 and the zoom keys.
- The commit carries `Step: ui.4`.

Evidence: `cargo run -p twicecat-app -- --ephemeral --homeserver http://localhost:8008`, log in as a `.env.test` user, press Ctrl+, and change the family, density and zoom; `settings.json` in the ephemeral directory shows the values, and has no `palette` key after "Use the default". The look is a human checkpoint item.

### Task 5: Headless UI gallery
Language: rust
Files:
- `crates/app/tests/ui_gallery.rs` (new);
- `crates/app/tests/common/fixture.rs` (new): a no-op `CommandSink`, and an `App` driven only through `App::new` and `App::update` with `CoreEvent`s from `twicecat_core::api`, reusing the call events `tests/calls.rs` already sends. The fixture covers:
  - logged in, with rooms, a timeline with every item kind, a thread, typing, a reply and an upload in progress;
  - a call in each `CallPhase`, with four participants (speaking, muted, local, declined), push-to-talk talking and not talking, and an incoming ring;
  - the Voice & audio card, with devices, levels, GPU status (found, none, looking) and diagnostics.

Depends on: Task 2, Task 4
Acceptance criteria:
- A non-ignored test lays out each screen headlessly at 1280×800 and 720×800 under every family × mode × density, and fails if any layout panics. The screens are login, waiting, loading, the main view with and without a room, the main view in a call (bar plus panel), the ring card, Voice & audio, settings, sessions, room settings, members, create/join, upload, viewer, switcher, verification (each phase), recovery and banners. It also checks that the room pane's node keeps its place in the layout tree when the thread panel, the call panel, the call bar or a banner come and go, which pins the slot invariants.
- The gallery loads the Inter faces into the headless font system first (Task 2).
- An `#[ignore = "writes PNGs for review"]` test writes `<screen>-<family>-<mode>-<density>.png` (plus `-rail` for room-list variants) through `Headless::screenshot` (tiny-skia, scale 1.0) into `$TWICECAT_GALLERY_DIR`, or `target/ui-gallery/` when that is not set. It also writes `index.html`, a plain `<img>` grid with no scripts and nothing remote. The grid has one column per family, so the four colour sets can be compared side by side for the default pick.
- The fixture holds no real names, tokens or keys, and `no_secret_logging` passes. No audio device is opened: the fixture never sends a command to a real core.
- Overlays from iced's overlay layer (open `pick_list` menus, tooltips) are not drawn headlessly. PROGRESS.md says so.
- PROGRESS.md adds the human item "Pick the default colour set from the gallery (recommended: Frost)".
- The commit carries `Step: ui.5`.

Evidence: `TWICECAT_GALLERY_DIR=target/ui-gallery cargo test -p twicecat-app --test ui_gallery -- --ignored` writes the PNGs and `index.html`; `ls target/ui-gallery | wc -l` shows the expected count.

### Task 6: Main window frame: header, panes and status bar
Language: rust
Files:
- `crates/app/src/screens/main_view.rs`, `crates/app/src/screens/spaces_sidebar.rs` and `crates/app/src/screens/thread_panel.rs`.

Depends on: Task 5
Acceptance criteria:
- Room header:
  - room avatar, name at `TITLE` size, and the topic on one capped line underneath;
  - lock, Voice and trust shown as `tag`s;
  - Members, room Settings and the notification mode as `icon_button`s;
  - the call button (`calls::CallButton`: Call / Join call (N) / Join voice / Leave call / Joining…) as a `primary_button`, or a `danger_button` for Leave, with its label kept;
  - People as a `toggle_icon_button`;
  - an invite keeps Accept and Decline.
- Panes are separated by 1 px rules in `Semantic::border`. The sidebar is on `sidebar` and the thread panel on `surface`, with a header and a close `icon_button`.
- Status bar:
  - a sync dot plus words (never colour alone);
  - the account;
  - Settings, Voice & audio, Sessions and Log out in an account menu.
- The slot invariants hold (gallery test), and the existing `main_view` and `state` tests pass unchanged. A unit test lists every `Message` the old header and status bar could send, including `CallMessage::TogglePanel` and each `CallButton::press`, and checks that each is still reachable.
- The commit carries `Step: ui.6`.

Evidence: the gallery `main-*.png`. In the app, the header in a room with a call shows Join call and People (human checkpoint).

### Task 7: Room list redesign, density and the rail
Language: rust
Files:
- `crates/app/src/screens/room_list_pane.rs`, `crates/app/src/room_list.rs` (`ROW_HEIGHT` becomes `row_height(density)`), `crates/app/src/state.rs`, `crates/app/src/keys.rs` (Ctrl+B) and `docs/accessibility.md`.

Depends on: Task 6
Acceptance criteria:
- Expanded rows:
  - avatar, name, last-message time, and a one-line preview;
  - `count_badge`;
  - the call pill as a `tag`, with `Icon::Speaker` and the count, in `highlight` while the room rings;
  - the Voice, Invite and Muted `tag`s;
  - the selected row on `selection` with a 3 px accent bar.
- The tabs become a segmented control, and search gets a search icon and a clear `icon_button`.
- Compact uses 44 px rows without a preview, and Comfortable keeps 64 px. The windowing helpers use `row_height(density)`, and the windowing and 10,000-room tests run for both densities.
- Rail (`RoomListChoice::Rail`):
  - 72 px wide;
  - avatars with an unread dot and the call pill reduced to a speaker dot, with a tooltip naming the room;
  - fixed 56 px rows;
  - the list slot and the room-list column slots are kept, and the call bar becomes the compact dock from Task 11.
- Ctrl+B toggles and saves the choice; it is in `SHORTCUTS`. Alt+Up/Down and Ctrl+Shift+Up/Down work the same in the rail (update tests).
- Empty states use `empty_state` with the existing `empty_label` text.
- The commit carries `Step: ui.7`.

Evidence: the gallery `main-*-{comfortable,compact}.png` and `main-*-rail.png`. In the app, Ctrl+B collapses and expands the list (human checkpoint).

### Task 8: Narrow windows
Language: rust
Files:
- `crates/app/src/state.rs` (`window_width` from `window::resize_events`), `crates/app/src/screens/main_view.rs`, `thread_panel.rs`, `call.rs` (`panel_slot`) and `room_list_pane.rs`.

Depends on: Task 7
Acceptance criteria:
- A pure `layout_for(width, choice, thread_open, call_panel_open) -> PaneLayout`, table-tested:
  - at 1100 px or more, the user's choice applies and the side panels are side panes;
  - from 800 to 1100 px, the list is forced to the rail while a thread or the call panel is open, and when both are open the call panel shows and the thread panel waits (the last one opened wins, test);
  - under 800 px, the sidebar slot is 0 wide, the list is a rail, and an open side panel takes the room pane's width. The room pane stays in the tree, and Esc returns to it in the existing Esc order.
- The saved `RoomListChoice` never changes because of the width; only the shown layout does.
- A resize updates `window_width` with no `Task`. The update tests cross both thresholds with no panel, the thread, the call panel and both.
- The gallery renders 720 and 1280 px for each layout, and the slot test passes at both.
- The commit carries `Step: ui.8`.

Evidence: the gallery `main-narrow-*.png`. In the app, dragging the window narrower during a call collapses the list, then the sidebar (human checkpoint).

### Task 9: Timeline restyle (same grouped layout)
Language: rust
Files:
- `crates/app/src/screens/timeline_pane.rs`, `crates/app/src/timeline.rs` (estimates per density) and `crates/app/src/rich_text.rs` (code and quote surfaces only).

Depends on: Task 6
Acceptance criteria:
- The layout is today's (answer Q8): the same header and continuation rows, the same grouping (`is_continuation` unchanged, with its tests), and the same order of parts within a row. Only colours, surfaces, spacing, type and icons change, through the tokens, the kit and `Density::timeline_row_pad`.
- Restyled parts:
  - sender names in `BODY_SEMIBOLD`, with `sender_label`'s ambiguity handling kept;
  - timestamps in `muted` `CAPTION`;
  - day dividers as centred `tag`s between rules;
  - reactions as rounded chips, with your own reaction outlined in the accent;
  - replies, code and quotes on `surface` with a 3 px bar;
  - call notices in the timeline (the existing call items or state lines) with `Icon::Call`.
- The action bar keeps its place and buttons. Its buttons become `icon_button`s with tooltips (Reply, React, Edit, Delete, Copy, Thread) on `surface_raised`, and every existing action message is still sent.
- Shields, UTD, echo states and receipts keep their words and colours, and their tests pass unchanged.
- `estimate_height(item, continuation, width, density)` stays within ±25 % of the measured height for the synthetic timeline in `tests/timeline_layout.rs`, for both densities (new assertion). The 10,000-item test passes for both.
- The commit carries `Step: ui.9`.

Evidence: the gallery `main-room-*.png`. In the app, a local-homeserver room with replies, reactions and a thread reads as before, restyled (human checkpoint).

### Task 10: Composer and the bars above it
Language: rust
Files:
- `crates/app/src/screens/main_view.rs` (`editor`, `mode_bar`, `suggestion_list`, `link_bar`, `command_bar`) and `crates/app/src/composer.rs` (view helpers only).

Depends on: Task 9
Acceptance criteria:
- The composer sits on a rounded `surface_raised` field, with an Attach `icon_button` on the left and Emoji and Send on the right. Send is enabled under the editor's existing `enabled` condition when the text is not blank. It sends the same `Command` as Enter (update test).
- The reply and edit bar is a `card` with the sender, one capped line and a close `icon_button`. Suggestions are a `card` with the highlight on `selection`.
- `send_error` and `actions.notice` use `banner(Danger, …)` with the same words.
- The editor keeps its slot and focus. The Enter, Shift+Enter, Up and Tab tests pass unchanged, and so do Ctrl+Shift+M and Ctrl+Shift+D while the composer has the focus.
- The placeholder reads "Message <room name>", sanitised with `name_or_placeholder`.
- The commit carries `Step: ui.10`.

Evidence: the gallery `main-room-*` with a reply and suggestions open. In the app, typing `@al` shows the card and Enter sends (human checkpoint).

### Task 11: Call screens: dock, participants panel, ring card
Language: rust
Files:
- `crates/app/src/screens/call.rs`, `crates/app/src/screens/room_list_pane.rs` (dock placement only) and `crates/app/src/calls.rs` (view helpers only; no change to `CallMessage` or call state).

Depends on: Task 7
Acceptance criteria:
- **Call dock** (replaces `call::bar`, in the same slot, last in the room-list column):
  - a `card` on `surface_raised`;
  - line 1: a phase dot (Joining in warning, Connected in success, Reconnecting in danger) plus `CallPhase::label()` words, the room name as a ghost button that sends `ShowCall`, and the lock when encrypted;
  - line 2, when set: `declined_line`;
  - line 3, in push-to-talk mode only: `talk_line` as a pill, on the accent with `Icon::Mic` while talking, muted with `Icon::Keyboard` or `Icon::Gamepad` otherwise;
  - controls row: `toggle_icon_button`s for Mute (`Mic`/`MicOff`) and Deafen (`Speaker`/`Deafened`), with the tooltips carrying `mute_labels`' words; an Audio settings `icon_button` (opens Voice & audio); and Leave as a `danger_button` with words, off while `pending`.

  In the rail (Task 7), the dock is a 72 px column: dot, Mute, Deafen and Leave, with tooltips. The call notice is a `banner(Danger)` with a close `icon_button` (`DismissNotice`).
- **Participants panel** (`panel_slot`, same slot and `PANEL_WIDTH`):
  - header "In the call (N)" with a close `icon_button`;
  - rows: `speaking_ring(avatar, speaking)` in `Semantic::speaking`; the name, "(you)" and the mute icon; a status line ("Speaking" / "Muted", words kept); and the cleaning line;
  - a row's controls fold behind a "More" `icon_button`, which shows the volume slider with −/+ and percentage (same `CallMessage::Volume`, `VolumeStep` and `VolumeReleased`) and the cleaning `pick_list`.

  Which rows are expanded is view-only state in `calls.rs`, and an update test covers expanding and collapsing.
- **Ring card:**
  - a `modal` at `RING_CARD_WIDTH` with a large avatar (64 px), `ring_lines` in `HEADLINE` and `BODY`, and Decline (`danger_button`) and Accept (`primary_button`), each with an icon and words;
  - the "Esc hides this; the room keeps ringing." note is kept;
  - the buttons are off when `answered`.
- No `CallMessage`, call state or core command changes. `tests/calls.rs`, `tests/ptt.rs` and the unit tests in `call.rs` and `calls.rs` (`mute_labels`, `declined_line`, `ring_lines`, `talk_line`) pass unchanged. The Esc order (ring card first, then the participants panel last) is unchanged (keys tests).
- Speaking is shown by a ring and the word "Speaking", never by colour alone.
- The commit carries `Step: ui.11`.

Evidence: the gallery `call-dock-<phase>-*.png`, `call-panel-*.png`, `call-ring-*.png` and `main-call-*-rail.png`. Against the local stack (`scripts/stack.sh`, two `.env.test` users), starting a call in a room shows the dock and the panel. How it sounds, and speaking rings that follow real voices, are human checkpoint items.

### Task 12: Voice & audio card as pages, with push-to-talk and GPU status
Language: rust
Files:
- `crates/app/src/screens/audio_settings.rs` and `crates/app/src/audio_settings.rs` (an `AudioPage` enum in `AudioUi`; no change to `AudioMessage` semantics or to the `AudioSettings` sent to the core).

Depends on: Task 1, Task 5
Acceptance criteria:
- The card is a `modal` (760 × 640) with `page_nav` on the left and the page on the right. The pages are:
  - **Devices:** microphone and speakers;
  - **Talking:** input mode (voice activity, push-to-talk, toggle), the key row with its capture state, the gamepad button, release delay, and the PTT source lines with their states from `source_line`/`keyboard_note`;
  - **Microphone:** noise suppression, echo, gain, the gate and its threshold, plus the GPU block;
  - **Levels:** the meters;
  - **Mic test;**
  - **Ringtone;**
  - **Other voices:** cleaning, its limit and skip policy;
  - **Announce;**
  - **Diagnostics.**

  Each existing section moves whole to one page.
- **GPU block:**
  - a status `card` with `Icon::Gpu`, the line from `gpu_status`, the backend in use as a `tag` (`gpu_name`, for example "NVIDIA · TensorRT"), and the other backends that were found as muted `tag`s;
  - the "moved to the CPU" states as a `banner(Warning)`, and "none found" as `empty_state` words;
  - in a build without `--features gpu`, the block shows the existing note, unchanged.
- The key-capture state reads "Press a key…" on the `focus` surface while `audio_ui.capturing` is set. Esc during capture cancels the capture before it closes the card, as today (keys tests unchanged).
- Meters are `progress_bar`s in `success`, `warning` above −12 dBFS and `danger` above −3 dBFS, with the dB value as text beside them (`meter`'s label kept). Diagnostics lines use `ui::fonts::MONO` in a two-column table (label, value) built from the same `diagnostics()` lines.
- The call notice still shows at the top of every page.
- Opening the card from the dock or the account menu shows Devices. Opening it from the talk line shows Talking: a new `AudioMessage::OpenAt(AudioPage)`, with update tests.
- A test lists the `AudioMessage`s each page can send, and together they cover all of them. Every existing test in `audio_settings.rs` passes unchanged.
- The commit carries `Step: ui.12`.

Evidence: the gallery `audio-<page>-*.png`, including `audio-microphone-gpu-{found,none,looking}-*.png`. In the app, Voice & audio → Talking → "Set key…" and pressing a key shows its label (human checkpoint; the portal dialog on Wayland is human-only).

### Task 13: Login, browser sign-in and loading screens
Language: rust
Files:
- `crates/app/src/screens/login.rs` and `crates/app/src/screens/loading.rs`.

Depends on: Task 5
Acceptance criteria:
- Login is a centred `card` (420 px) containing, in order:
  - "Twicecat" in `DISPLAY` (Inter SemiBold) and a tagline;
  - the user and password fields;
  - "Log in" (primary), an "or" divider, and "Sign in with browser" (secondary);
  - the server field behind a "Use another server" disclosure, which opens on its own when the field is not empty or the notice is about the server.
- The Tab order is user → password → Log in → browser → disclosure (→ server), tested on a pure focus-order function. Enter submits from every field.
- The notice is a `banner`. While a login is in flight the fields are off and the button reads "Signing in…".
- Waiting for the browser is a `card` with the status line, Copy link (only when the browser failed) and Cancel. The URL is still never shown (existing test).
- Loading is a `card` with the sync label and a `progress_bar` driven by the sync events that already arrive. Starting, Closing and "The core stopped" use `empty_state`.
- The commit carries `Step: ui.13`.

Evidence: the gallery `login-*.png`, `waiting-*.png` and `loading-*.png`. Running the app with `--ephemeral` against the local homeserver and logging in as a `.env.test` user reaches the main view (human checkpoint for the look).

### Task 14: Settings and Sessions as pages
Language: rust
Files:
- `crates/app/src/screens/settings.rs`, `crates/app/src/settings.rs` (`SettingsPage`), `crates/app/src/screens/sessions.rs` and `docs/accessibility.md`.

Depends on: Task 4, Task 12
Acceptance criteria:
- Settings is a modal (760 × 620) with `page_nav`. The pages are:
  - Profile;
  - Appearance;
  - Notifications;
  - Voice & audio: `VOICE_AUDIO_LINE` and a button that sends `SettingsMessage::OpenVoiceAudio` (unchanged);
  - Keyboard;
  - Sessions;
  - About: the drag-and-drop note, the version, and the Inter attribution line (Task 2).
- Ctrl+, opens Profile. A test lists the `SettingsMessage`s each page can send, and together they cover all of them.
- Esc closes Settings as before, and Tab cycles within the page, then to the page list. The accessibility document says so.
- Sessions: the current session is first; rows show name, trust `tag`, last seen and `ghost_button` actions; reauth and the logout warning use `modal` and `banner(Warning)`. The existing tests pass unchanged.
- The commit carries `Step: ui.14`.

Evidence: the gallery `settings-<page>-*.png` and `sessions-*.png`. In the app, Ctrl+, and then each page (human checkpoint).

### Task 15: Room dialogs, room settings, members, upload, viewer and switcher
Language: rust
Files:
- `crates/app/src/screens/room_dialogs.rs`, `room_settings.rs`, `members.rs`, `upload_dialog.rs`, `media_viewer.rs` and `switcher.rs`.

Depends on: Task 6
Acceptance criteria:
- Every dialog is a `modal` with a header (title plus a close `icon_button`), a body, and a footer with the primary action last and destructive actions as `danger_button`. Confirmation flows are unchanged.
- Members has search, sections for Admins, Moderators, Members and Invited (from `RoomMemberView::role` and `membership`), avatar, name and trust `tag` rows, an "in the call" speaker `tag` when `calls` lists the member, and an `empty_state`.
- The switcher (Ctrl+K) is a command-palette card near the top of the window, with a large field and rows of avatar, name, unread badge and call pill. The Up/Down/Enter/Esc tests pass unchanged.
- The viewer has a dimmed backdrop, the image centred, and a bottom bar with name, size, Save and Close. Upload shows the preview, caption, size and type.
- No messages or update logic change; the module tests pass unchanged.
- The commit carries `Step: ui.15`.

Evidence: the gallery `dialog-*.png`, `members-*.png`, `switcher-*.png`, `viewer-*.png` and `upload-*.png`. In the app: Ctrl+K, Members and Attach (human checkpoint).

### Task 16: Security banners, recovery and verification
Language: rust
Files:
- `crates/app/src/screens/security.rs` and `crates/app/src/screens/verification.rs`.

Depends on: Task 6
Acceptance criteria:
- Banners use `banner` by severity (Info: backup; Warning: verify this session or set up recovery; Danger: identity changed), each with one primary action and a dismiss where dismissing is allowed today.
- Recovery and change-key are a `modal`: the key is in `ui::fonts::MONO`, in groups of four on `surface`, with Copy and Save buttons. The "never in Debug or logs" tests pass.
- Verification is a `modal` with a three-step indicator (Request → Compare → Done). The emoji phase shows a grid of 7 large emoji with their names, Match (primary) and Doesn't match (danger), and explicit Cancel and Done. Every phase has a gallery state.
- No flow or command changes; the existing tests pass unchanged.
- The commit carries `Step: ui.16`.

Evidence: the gallery `verify-<phase>-*.png`, `recovery-*.png` and `banner-*.png`. A verification between two `.env.test` accounts on the local homeserver shows the steps (human checkpoint).

### Task 17: Apply the default colour set the human picked
Language: rust
Files:
- `crates/app/src/theme.rs` (the `DEFAULT_FAMILY` line), `PROGRESS.md` and `docs/accessibility.md` (which family is the default).

Depends on: Task 5 (and the human's pick at its checkpoint)
Acceptance criteria:
- This task starts only after the human has named a family in the "Pick the default colour set" checkpoint item. If none has been named when Tasks 1–16 are done, the agent stops and asks (AGENTS.md rule 8); it does not pick.
- The code change is one line: `pub const DEFAULT_FAMILY: Family = Family::<picked>;`. The diff of `crates/` is that line plus the test expectation that names the default.
- Tests:
  - `palette: None` resolves to the picked family;
  - a `settings.json` with an explicit `"palette":"Twicecat"` still shows Twicecat;
  - the swatch marked "(default)" is the picked one.
- PROGRESS.md records the pick, its date, and that the checkpoint item is closed by the human.
- The commit is `feat(app): make <family> the default colour set` with `Step: ui.17`.

Evidence: `cargo run -p twicecat-app -- --ephemeral --homeserver http://localhost:8008`, with a fresh data directory, opens in the picked family. Settings → Appearance marks it "(default)".

### Task 18: Wrap-up: accessibility document, checkpoint list and review
Language: rust
Files:
- `docs/accessibility.md`, `PROGRESS.md` ("UI/UX rework steps" and the checkpoint), `THIRD-PARTY-NOTICES.md` (final check) and `docs/plans/ui-ux-rework.progress.md`.

Depends on: Tasks 1–17
Acceptance criteria:
- `docs/accessibility.md` has:
  - the final keyboard map, with a test that checks it lists every `SHORTCUTS` row;
  - contrast per family and the 3:1 non-text rule for the speaking ring;
  - zoom, icon tooltips and narrow windows;
  - how calls show state in words as well as colour.
- PROGRESS.md lists every human checkpoint item below. The steps are ticked for their agent part only.
- A full gallery run is recorded (count and path). The gate passes on the finished branch, and `scripts/check-commits.sh main` passes: one commit per task (18) plus one `docs(plan)` commit.
- The commit is `docs(app): …` with `Step: ui.18`.

Evidence: `scripts/check-commits.sh main` exits 0, and `cargo test -p twicecat-app accessibility_doc_lists_every_shortcut` passes.

## Human checkpoint (not agent tasks; added to PROGRESS.md "UI/UX rework human checkpoint" by the task named)
- (Task 2) Inter in the real window on Linux (Wayland and X11), Windows and macOS: regular, italic, semibold and bold; and fallback for emoji, CJK and the icon glyphs. For Phase 7: every installer ships `crates/app/assets/fonts/inter/OFL.txt` and `THIRD-PARTY-NOTICES.md`.
- (Task 4) Each family in light and dark on a real window, High contrast in particular. System still follows the OS mode. Zoom at 80 % and 200 % on a HiDPI screen. A density switch keeps your place in the timeline.
- (Task 5) **Pick the default colour set** (the recommendation is Frost) from `target/ui-gallery/index.html`, comparing the four family columns in light and dark. Then say which other designs to keep. The PNGs are from tiny-skia, not the wgpu renderer used in the window, so confirm the pick in the real window (Task 4 item) before Task 17.
- (Tasks 6, 7, 8) The main window, Ctrl+B, and dragging narrower than 1100 and 800 px with and without a call. Focus stays in the composer, and the timeline does not jump when panels, the dock or banners come and go.
- (Tasks 9, 10) The restyled timeline in a busy room (replies, reactions, media, threads, edits, a UTD, local echo) still reads with the same layout as before. The composer, the suggestion card and the reply and edit bar. IME is still deferred from Phase 3.
- (Task 11) A real call (local stack, then with an Element Call user):
  - the dock's phase changes, including Reconnecting when the SFU is stopped;
  - Mute and Deafen from the dock and with Ctrl+Shift+M/D;
  - speaking rings follow real voices;
  - per-person volume and cleaning from the folded controls;
  - the ring card for an incoming DM call, and Esc hides it while the room keeps ringing;
  - the push-to-talk pill while holding the key on X11, Wayland (portal), Windows and macOS, and with a gamepad.
- (Task 12) Voice & audio pages with real devices:
  - device switching;
  - key capture, including the Wayland portal dialog;
  - meters moving with your voice, and the mic test on headphones;
  - the GPU block on your RTX 5090 (TensorRT or CUDA) and on Galep's Radeon (WebGPU), and the CPU fallback banner while a game loads the GPU.
- (Tasks 13–16) Login and loading against the local homeserver and once against your own account. Settings pages, Sessions, every dialog, the switcher, viewer and upload, and a verification between your own devices.
- (Task 17) A fresh profile opens in the picked default family.
- (All) Hover, pressed states and tooltip delays on each OS you use.

## Questions
Resolved (2026-10-13; the answers are folded into the plan):
- **Q1:** appearance choices live in `UiSettings` in the core, and the API addition is approved (Task 3).
- **Q2:** calls are in scope (Tasks 6, 7, 8, 11, 12, 15).
- **Q3:** Unicode icons for now, behind the single `ui::icons` module (Task 1).
- **Q4:** bundle a permissively licensed font: Inter 4.1, OFL 1.1 (Task 2).
- **Q5:** the trailer is `Step: ui.N`.
- **Q6:** go ahead before Phase 6 sign-off and before Phase 7 (a rule 1 exception granted by the human).
- **Q7:** a new default look. Twicecat stays the default only until the gallery review. The human picks the default colour set at the Task 5 checkpoint, the recommendation is Frost, and Task 17 applies it as a one-line change.
- **Q8:** keep the message layout as it is. The IRC and Bubbles task is dropped, there is no message-layout setting in `UiSettings` or in the gallery, and the timeline is restyled through tokens and density only (Task 9). Narrow windows (Task 8) stay.

Assumed (not answered):
- **Q9 (font faces):** four static faces (Regular, Italic, SemiBold, Bold, about 1.67 MB), the recommended option. The alternatives were adding `Inter-BoldItalic.ttf` (+425 KB) or the variable `InterVariable.ttf` (880 KB, plus the italic variable font). Say so before Task 2 if you want one of those instead.
